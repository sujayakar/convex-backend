//! SDK Client Simulation - runs the real ConvexClient JS SDK in a V8 isolate.
//!
//! This module wraps the higher-level `ConvexClient` (from `convex/browser`)
//! instead of `BaseConvexClient`, exercising:
//! - Callback-based subscription model (`onUpdate`)
//! - Promise-based mutation execution
//! - `setTimeout` scheduling for deferred callbacks
//! - Connection state tracking
//!
//! The isolate loads a bundled JS module (`sdk_client.js`) that creates a
//! `ConvexClient` with a simulated `TestingWebSocket`. WebSocket traffic is
//! routed through channels to the `ServerThread` (sync protocol).

use std::time::Duration;

use common::{
    errors::JsError,
    runtime::{
        Runtime,
        SpawnHandle,
    },
    value::{
        ConvexObject,
        ConvexValue,
    },
};
use runtime::testing::TestRuntime;
use sync_types::{
    Timestamp,
    UdfPath,
};
use tokio::sync::{
    mpsc,
    oneshot,
};

use super::server::ServerThread;

mod environment;
mod go;
mod protocol;
mod state;

pub type SubscriptionId = u32;

/// Requests that can be sent to the SDK client thread.
pub enum SdkClientRequest {
    /// Subscribe to a query via ConvexClient.onUpdate.
    Subscribe {
        query_name: String,
        args: ConvexObject,
        sender: oneshot::Sender<SubscriptionId>,
    },
    /// Get the latest result for a subscription.
    GetSubscriptionResult {
        subscription_id: SubscriptionId,
        sender: oneshot::Sender<Option<Result<ConvexValue, JsError>>>,
    },
    /// Unsubscribe from a query.
    Unsubscribe {
        subscription_id: SubscriptionId,
        sender: oneshot::Sender<()>,
    },
    /// Run a mutation via ConvexClient.mutation (Promise-based).
    RunMutation {
        udf_path: UdfPath,
        args: ConvexObject,
        sender: oneshot::Sender<Result<ConvexValue, JsError>>,
    },
    /// Get the max observed timestamp.
    MaxObservedTimestamp {
        sender: oneshot::Sender<Option<Timestamp>>,
    },
    /// Disconnect the simulated network.
    DisconnectNetwork {
        sender: oneshot::Sender<bool>,
    },
    /// Reconnect the simulated network.
    ReconnectNetwork {
        sender: oneshot::Sender<bool>,
    },
}

/// Handle to a running SDK client thread.
///
/// The thread runs a V8 isolate with the ConvexClient JS SDK, connected
/// to a `ServerThread` via simulated WebSocket channels.
#[derive(Clone)]
pub struct SdkClientThread {
    rt: TestRuntime,
    tx: mpsc::UnboundedSender<SdkClientRequest>,
}

impl SdkClientThread {
    /// Spawn a new SDK client thread connected to the given server.
    pub fn new(rt: TestRuntime, server: ServerThread) -> (Self, Box<dyn SpawnHandle>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let rt_ = rt.clone();
        let handle = rt.spawn_thread("sdk_client_thread", move || async move {
            Self::go(rt_, server, rx)
                .await
                .expect("SdkClientThread failed");
        });
        (Self { rt, tx }, handle)
    }

    /// Subscribe to a query. Returns a subscription ID.
    pub async fn subscribe(
        &self,
        query_name: &str,
        args: ConvexObject,
    ) -> anyhow::Result<SubscriptionId> {
        let (sender, receiver) = oneshot::channel();
        self.tx.send(SdkClientRequest::Subscribe {
            query_name: query_name.to_string(),
            args,
            sender,
        })?;
        Ok(receiver.await?)
    }

    /// Get the latest result for a subscription.
    pub async fn get_subscription_result(
        &self,
        subscription_id: SubscriptionId,
    ) -> anyhow::Result<Option<Result<ConvexValue, JsError>>> {
        let (sender, receiver) = oneshot::channel();
        self.tx.send(SdkClientRequest::GetSubscriptionResult {
            subscription_id,
            sender,
        })?;
        Ok(receiver.await?)
    }

    /// Unsubscribe from a query.
    pub async fn unsubscribe(&self, subscription_id: SubscriptionId) -> anyhow::Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.tx.send(SdkClientRequest::Unsubscribe {
            subscription_id,
            sender,
        })?;
        Ok(receiver.await?)
    }

    /// Run a mutation. Returns the result when the mutation completes.
    pub async fn run_mutation(
        &self,
        udf_path: UdfPath,
        args: ConvexObject,
    ) -> anyhow::Result<Result<ConvexValue, JsError>> {
        let (sender, receiver) = oneshot::channel();
        self.tx.send(SdkClientRequest::RunMutation {
            udf_path,
            args,
            sender,
        })?;
        Ok(receiver.await?)
    }

    /// Get the max observed timestamp from the SDK client.
    pub async fn max_observed_timestamp(&self) -> anyhow::Result<Option<Timestamp>> {
        let (sender, receiver) = oneshot::channel();
        self.tx
            .send(SdkClientRequest::MaxObservedTimestamp { sender })?;
        Ok(receiver.await?)
    }

    /// Wait until the SDK client has observed the given server timestamp.
    pub async fn wait_for_server_ts(&self, ts: Timestamp) -> anyhow::Result<()> {
        while self.max_observed_timestamp().await? < Some(ts) {
            self.rt.wait(Duration::from_secs(1)).await;
        }
        Ok(())
    }

    /// Disconnect the simulated network (drop all WebSocket connections).
    pub async fn disconnect_network(&self) -> anyhow::Result<bool> {
        let (sender, receiver) = oneshot::channel();
        self.tx
            .send(SdkClientRequest::DisconnectNetwork { sender })?;
        Ok(receiver.await?)
    }

    /// Reconnect the simulated network.
    pub async fn reconnect_network(&self) -> anyhow::Result<bool> {
        let (sender, receiver) = oneshot::channel();
        self.tx
            .send(SdkClientRequest::ReconnectNetwork { sender })?;
        Ok(receiver.await?)
    }
}
