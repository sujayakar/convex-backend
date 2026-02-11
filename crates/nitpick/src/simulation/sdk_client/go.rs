//! Main event loop for the SDK client thread.
//!
//! Runs a V8 isolate with the ConvexClient JS SDK, processing:
//! - Rust requests (subscribe, mutation, etc.)
//! - WebSocket messages from the server
//! - Timer completions (setTimeout)

use std::sync::Arc;

use deno_core::{
    serde_v8,
    v8::{
        self,
        scope,
    },
    ModuleSpecifier,
};
use isolate::{
    helpers::pump_message_loop,
    isolate::Isolate,
    ConcurrencyLimiter,
    RequestScope,
};
use runtime::testing::TestRuntime;
use tokio::sync::mpsc;

use super::{
    environment::SdkClientEnvironment,
    state::{
        SdkClientState,
        extract_error,
    },
    SdkClientRequest,
    SdkClientThread,
};
use crate::simulation::server::ServerThread;

const SDK_CLIENT_SPECIFIER: &str = "convex:/sdk_client.js";

impl SdkClientThread {
    pub(super) async fn go(
        rt: TestRuntime,
        server: ServerThread,
        mut rx: mpsc::UnboundedReceiver<SdkClientRequest>,
    ) -> anyhow::Result<()> {
        let mut isolate = Isolate::new(rt.clone(), None, ConcurrencyLimiter::unlimited());
        let client_id = Arc::new(String::new());
        let environment = SdkClientEnvironment::new(rt);
        let (handle, state, mut timeout) = isolate.start_request(client_id, environment).await?;
        scope!(let handle_scope, isolate.isolate());
        let v8_context = v8::Context::new(handle_scope, v8::ContextOptions::default());
        let context_scope = &mut v8::ContextScope::new(handle_scope, v8_context);
        let mut isolate_context =
            RequestScope::new(context_scope, handle.clone(), state, false).await?;

        {
            scope!(let v8_scope, isolate_context.scope());
            let mut scope =
                RequestScope::<TestRuntime, SdkClientEnvironment>::enter(v8_scope);
            let specifier = ModuleSpecifier::parse(SDK_CLIENT_SPECIFIER)?;
            let module = scope.eval_module(&specifier, &mut timeout).await?;

            let mut state = SdkClientState::new(&mut scope, server, module)?;

            'main: loop {
                tracing::debug!("SDK client: processing inbox");
                state.process_js_inbox(&mut scope)?;

                tracing::debug!("SDK client: processing outbox");
                state.process_js_outbox(&mut scope)?;

                tracing::debug!("SDK client: microtask checkpoint");
                scope.perform_microtask_checkpoint();
                pump_message_loop(&scope);

                let rejections = scope.pending_unhandled_promise_rejections_mut();
                if let Some(promise) = rejections.exceptions.keys().next().cloned() {
                    let err = rejections.exceptions.remove(&promise).unwrap();
                    let err = v8::Local::new(&scope, err);
                    let err = extract_error(&scope, err)?;
                    anyhow::bail!(err);
                }

                // Don't block if we have something to do.
                if !state.is_outbox_empty() || !state.is_inbox_empty() {
                    continue;
                }

                // Block, inject something into JS, and restart the loop.
                let isolate_state = scope.state_mut()?;
                let environment = &mut isolate_state.environment;
                tokio::select! {
                    maybe_req = rx.recv() => {
                        let Some(req) = maybe_req else {
                            break 'main;
                        };
                        state.handle_request(&mut scope, req)?;
                    }
                    (web_socket_id, maybe_msg) = state.next_message() => {
                        state.handle_websocket_message(web_socket_id, maybe_msg)?;
                    }
                    resolver = environment.next_timer() => {
                        let resolver = resolver?;
                        let resolver = resolver.open(&mut scope);
                        let result = serde_v8::to_v8(&mut scope, ())?;
                        resolver.resolve(&scope, result);
                    }
                }
            }
        }
        tracing::info!("SdkClientThread shutting down...");
        drop(isolate_context);
        handle.take_termination_error(None, "sdk_client")??;
        Ok(())
    }
}
