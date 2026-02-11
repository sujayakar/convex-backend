//! V8 state management for the SDK client thread.
//!
//! Manages the JS function handles, WebSocket bridging (inbox/outbox),
//! and network state. The WebSocket protocol is shared with js_client.

use std::{
    collections::BTreeMap,
    mem,
};

use anyhow::Context;
use common::{
    errors::JsError,
    value::ConvexValue,
};
use deno_core::{
    serde_v8,
    v8::{
        self,
        tc_scope,
    },
};
use futures::future;
use isolate::error::extract_source_mapped_error;
use serde::{
    Deserialize,
    Serialize,
};
use sync::ServerMessage;
use sync_types::{
    ClientMessage,
    Timestamp,
};
use tokio::{
    sync::{
        mpsc,
        oneshot,
    },
    time::Instant,
};

use super::{
    environment::SDK_CLIENT_SOURCE_MAP,
    protocol::{
        CompletedMutation,
        MutationResult,
        RunMutationArgs,
        SubscribeArgs,
        SubscriptionResult,
    },
    SdkClientRequest,
};
use crate::simulation::{
    js_client::{
        js_protocol::{
            JsIncomingMessage,
            JsOutgoingMessage,
        },
        state::get_function,
    },
    server::ServerThread,
};

pub type WebsocketId = u32;

pub struct SdkClientState<'a> {
    // SDK client JS functions
    subscribe: v8::Local<'a, v8::Function>,
    get_subscription_result: v8::Local<'a, v8::Function>,
    unsubscribe: v8::Local<'a, v8::Function>,
    run_mutation: v8::Local<'a, v8::Function>,
    get_completed_mutations: v8::Local<'a, v8::Function>,
    get_sdk_max_observed_timestamp: v8::Local<'a, v8::Function>,

    // WebSocket bridging (shared protocol with js_client)
    get_outgoing_messages: v8::Local<'a, v8::Function>,
    receive_incoming_messages: v8::Local<'a, v8::Function>,

    server: ServerThread,

    next_mutation_id: u32,
    in_progress_mutations: BTreeMap<u32, oneshot::Sender<Result<ConvexValue, JsError>>>,

    js_inbox: Vec<JsOutgoingMessage>,
    js_outbox: Vec<JsIncomingMessage>,

    network: NetworkState,
}

impl<'a> SdkClientState<'a> {
    pub fn new(
        scope: &mut v8::PinScope<'a, '_>,
        server: ServerThread,
        module: v8::Local<v8::Module>,
    ) -> anyhow::Result<Self> {
        let namespace: v8::Local<v8::Object> = module.get_module_namespace().try_into()?;

        let subscribe = get_function(scope, namespace, "subscribe")?;
        let get_subscription_result =
            get_function(scope, namespace, "getSubscriptionResult")?;
        let unsubscribe = get_function(scope, namespace, "unsubscribe")?;
        let run_mutation = get_function(scope, namespace, "runMutation")?;
        let get_completed_mutations =
            get_function(scope, namespace, "getCompletedMutations")?;
        let get_sdk_max_observed_timestamp =
            get_function(scope, namespace, "getSdkMaxObservedTimestamp")?;
        let get_outgoing_messages = get_function(scope, namespace, "getOutgoingMessages")?;
        let receive_incoming_messages =
            get_function(scope, namespace, "receiveIncomingMessages")?;

        Ok(SdkClientState {
            subscribe,
            get_subscription_result,
            unsubscribe,
            run_mutation,
            get_completed_mutations,
            get_sdk_max_observed_timestamp,
            get_outgoing_messages,
            receive_incoming_messages,
            js_inbox: vec![],
            js_outbox: vec![],
            network: NetworkState::Enabled {
                websockets: BTreeMap::new(),
            },
            server,
            next_mutation_id: 0,
            in_progress_mutations: BTreeMap::new(),
        })
    }

    pub fn get_max_observed_timestamp(
        &self,
        scope: &mut v8::PinScope<'a, '_>,
    ) -> anyhow::Result<Option<Timestamp>> {
        let Some(ts_str) =
            self.call::<_, Option<String>>(scope, self.get_sdk_max_observed_timestamp, ())?
        else {
            return Ok(None);
        };
        let ts_u64: u64 = ts_str.parse()?;
        Ok(Some(ts_u64.try_into()?))
    }

    pub fn is_outbox_empty(&self) -> bool {
        self.js_outbox.is_empty()
    }

    pub fn is_inbox_empty(&self) -> bool {
        self.js_inbox.is_empty()
    }

    /// Process outgoing WebSocket messages from JS and completed mutations.
    pub fn process_js_inbox(&mut self, scope: &mut v8::PinScope<'a, '_>) -> anyhow::Result<()> {
        // Drain WebSocket outgoing messages (connect/send/close).
        let messages =
            self.call::<(), Vec<JsOutgoingMessage>>(scope, self.get_outgoing_messages, ())?;
        self.js_inbox.extend(messages);

        // Drain completed mutation results.
        let completed_mutations =
            self.call::<(), Vec<CompletedMutation>>(scope, self.get_completed_mutations, ())?;
        for completed in completed_mutations {
            let sender = self
                .in_progress_mutations
                .remove(&completed.mutation_id)
                .context("Missing completion for mutation ID")?;
            let result = match completed.result {
                MutationResult::Success { value } => Ok(value.try_into()?),
                MutationResult::Failure { error } => Err(JsError::from_message(error)),
            };
            let _ = sender.send(result);
        }

        for msg in self.js_inbox.drain(..) {
            match msg {
                JsOutgoingMessage::Connect { web_socket_id } => {
                    let pair = self.server.connect()?;
                    match self.network {
                        NetworkState::Enabled { ref mut websockets } => {
                            anyhow::ensure!(websockets.insert(web_socket_id, pair).is_none());
                            self.js_outbox
                                .push(JsIncomingMessage::Connected { web_socket_id });
                        },
                        NetworkState::Disabled {
                            ref mut waiting_connects,
                        } => {
                            waiting_connects.push(web_socket_id);
                        },
                    }
                },
                JsOutgoingMessage::Send {
                    web_socket_id,
                    data,
                } => {
                    let tx = self.network.get_sender(web_socket_id)?;
                    let msg: serde_json::Value = serde_json::from_str(&data)?;
                    tx.send((msg.try_into()?, Instant::now()))?;
                },
                JsOutgoingMessage::Close { web_socket_id } => {
                    self.network.close(web_socket_id);
                    self.js_outbox
                        .push(JsIncomingMessage::Closed { web_socket_id });
                },
                JsOutgoingMessage::PersistMutation {
                    persist_id, ..
                } => {
                    self.js_outbox.push(JsIncomingMessage::PersistenceDone {
                        persist_id,
                        error: None,
                    });
                },
                JsOutgoingMessage::PersistPages {
                    persist_id, ..
                } => {
                    self.js_outbox.push(JsIncomingMessage::PersistenceDone {
                        persist_id,
                        error: None,
                    });
                },
                JsOutgoingMessage::MutationDone { .. } => {
                    // SDK client mutations are handled via getCompletedMutations above.
                    // This shouldn't fire since we use ConvexClient's Promise-based API.
                    tracing::warn!("Unexpected MutationDone in SDK client path");
                },
            }
        }
        Ok(())
    }

    /// Push incoming WebSocket messages into JS.
    pub fn process_js_outbox(&mut self, scope: &mut v8::PinScope<'a, '_>) -> anyhow::Result<()> {
        if self.js_outbox.is_empty() {
            return Ok(());
        }
        let messages = mem::take(&mut self.js_outbox);
        for message in &messages {
            tracing::debug!("sdk_client outbox: {message:?}");
        }
        self.call::<_, ()>(scope, self.receive_incoming_messages, messages)?;
        Ok(())
    }

    /// Handle a request from the Rust side.
    pub fn handle_request(
        &mut self,
        scope: &mut v8::PinScope<'a, '_>,
        req: SdkClientRequest,
    ) -> anyhow::Result<()> {
        match req {
            SdkClientRequest::Subscribe {
                query_name,
                args,
                sender,
            } => {
                let subscribe_args = SubscribeArgs {
                    query_name,
                    args_json: serde_json::to_string(&serde_json::Value::from(args))?,
                };
                let subscription_id: u32 = self.call(scope, self.subscribe, subscribe_args)?;
                let _ = sender.send(subscription_id);
            },
            SdkClientRequest::GetSubscriptionResult {
                subscription_id,
                sender,
            } => {
                let result_json: Option<String> =
                    self.call(scope, self.get_subscription_result, subscription_id)?;
                let result = match result_json {
                    Some(json_str) => {
                        let parsed: SubscriptionResult = serde_json::from_str(&json_str)?;
                        if let Some(error) = parsed.error {
                            Some(Err(JsError::from_message(error)))
                        } else if let Some(value) = parsed.value {
                            Some(Ok(value.try_into()?))
                        } else {
                            None
                        }
                    },
                    None => None,
                };
                let _ = sender.send(result);
            },
            SdkClientRequest::Unsubscribe {
                subscription_id,
                sender,
            } => {
                self.call::<_, ()>(scope, self.unsubscribe, subscription_id)?;
                let _ = sender.send(());
            },
            SdkClientRequest::RunMutation {
                udf_path,
                args,
                sender,
            } => {
                let mutation_id = self.next_mutation_id;
                self.next_mutation_id += 1;
                let mutation_args = RunMutationArgs {
                    mutation_id,
                    name: String::from(udf_path),
                    args_json: serde_json::to_string(&serde_json::Value::from(args))?,
                };
                self.call::<_, ()>(scope, self.run_mutation, mutation_args)?;
                self.in_progress_mutations.insert(mutation_id, sender);
            },
            SdkClientRequest::MaxObservedTimestamp { sender } => {
                let ts = self.get_max_observed_timestamp(scope)?;
                let _ = sender.send(ts);
            },
            SdkClientRequest::DisconnectNetwork { sender } => {
                let result = match self.network {
                    NetworkState::Enabled { ref mut websockets } => {
                        for (websocket_id, _) in mem::take(websockets) {
                            self.js_outbox.push(JsIncomingMessage::Closed {
                                web_socket_id: websocket_id,
                            });
                        }
                        self.network = NetworkState::Disabled {
                            waiting_connects: vec![],
                        };
                        true
                    },
                    NetworkState::Disabled { .. } => false,
                };
                let _ = sender.send(result);
            },
            SdkClientRequest::ReconnectNetwork { sender } => {
                let result = match self.network {
                    NetworkState::Disabled {
                        ref mut waiting_connects,
                    } => {
                        let mut websockets = BTreeMap::new();
                        for websocket_id in waiting_connects.drain(..) {
                            let pair = self.server.connect()?;
                            websockets.insert(websocket_id, pair);
                            self.js_outbox.push(JsIncomingMessage::Connected {
                                web_socket_id: websocket_id,
                            });
                        }
                        self.network = NetworkState::Enabled { websockets };
                        true
                    },
                    NetworkState::Enabled { .. } => false,
                };
                let _ = sender.send(result);
            },
        }
        Ok(())
    }

    /// Handle a WebSocket message from the server.
    pub fn handle_websocket_message(
        &mut self,
        websocket_id: WebsocketId,
        msg: Option<ServerMessage>,
    ) -> anyhow::Result<()> {
        match msg {
            Some(msg) => {
                let msg_string = serde_json::to_string(&serde_json::Value::from(msg))?;
                self.js_outbox.push(JsIncomingMessage::Message {
                    web_socket_id: websocket_id,
                    data: msg_string,
                });
            },
            None => {
                self.network.close(websocket_id);
                self.js_outbox.push(JsIncomingMessage::Closed {
                    web_socket_id: websocket_id,
                });
            },
        }
        Ok(())
    }

    /// Wait for the next WebSocket message from the server.
    pub async fn next_message(&mut self) -> (WebsocketId, Option<ServerMessage>) {
        match self.network {
            NetworkState::Disabled { .. } => future::pending().await,
            NetworkState::Enabled { ref mut websockets } => {
                // ConvexClient should only have one active WebSocket at a time,
                // but during reconnection there may briefly be zero.
                match websockets.iter_mut().next() {
                    Some((web_socket_id, (_, rx))) => {
                        let maybe_msg = rx.recv().await;
                        (*web_socket_id, maybe_msg.map(|(msg, _)| msg))
                    },
                    None => futures::future::pending().await,
                }
            },
        }
    }

    fn call<Args, Returns>(
        &self,
        scope: &mut v8::PinScope<'a, '_>,
        f: v8::Local<'a, v8::Function>,
        args: Args,
    ) -> anyhow::Result<Returns>
    where
        Args: Serialize,
        Returns: Deserialize<'static>,
    {
        let args_v8 = serde_v8::to_v8(scope, args)?;
        let (result, exception);
        {
            tc_scope!(let tc_scope, scope);
            result = f.call(tc_scope, f.into(), &[args_v8]);
            exception = tc_scope.exception();
        }
        if let Some(e) = exception {
            let err = extract_error(scope, e)?;
            anyhow::bail!(err);
        }
        let result = result.ok_or_else(|| anyhow::anyhow!("No result"))?;
        let result: Returns = serde_v8::from_v8(scope, result)?;
        Ok(result)
    }
}

enum NetworkState {
    Disabled {
        waiting_connects: Vec<WebsocketId>,
    },
    Enabled {
        websockets: BTreeMap<
            WebsocketId,
            (
                mpsc::UnboundedSender<(ClientMessage, Instant)>,
                mpsc::UnboundedReceiver<(ServerMessage, Instant)>,
            ),
        >,
    },
}

impl NetworkState {
    pub fn get_sender(
        &self,
        websocket_id: WebsocketId,
    ) -> anyhow::Result<&mpsc::UnboundedSender<(ClientMessage, Instant)>> {
        match self {
            NetworkState::Enabled { websockets } => {
                let (tx, _) = websockets
                    .get(&websocket_id)
                    .ok_or_else(|| anyhow::anyhow!("Unknown websocket id: {websocket_id}"))?;
                Ok(tx)
            },
            NetworkState::Disabled { .. } => anyhow::bail!("Network is disabled"),
        }
    }

    pub fn close(&mut self, websocket_id: WebsocketId) {
        match self {
            NetworkState::Enabled { websockets } => {
                websockets.remove(&websocket_id);
            },
            NetworkState::Disabled { .. } => (),
        }
    }
}

pub fn extract_error<'a>(
    scope: &v8::PinScope<'a, '_>,
    err: v8::Local<'a, v8::Value>,
) -> anyhow::Result<JsError> {
    let (message, frame_data, custom_data) = extract_source_mapped_error(scope, err)?;
    let err = JsError::from_frames(message, frame_data, custom_data, |url| {
        if url.as_str() == "convex:/sdk_client.js" {
            return Ok(Some(SDK_CLIENT_SOURCE_MAP.clone()));
        }
        tracing::error!("Unknown source URL: {url}");
        Ok(None)
    });
    Ok(err)
}
