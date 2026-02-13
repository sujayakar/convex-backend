//! Subscription scenario: reactive queries via the sync protocol.
//!
//! Sets up a JsClientThread connected to a ServerThread (sync worker).
//! Subscribes to a counter query, then runs mutations that increment the
//! counter. Verifies that the subscription eventually reflects each
//! mutation.
//!
//! This tests the full sync pipeline: SyncWorker, subscription
//! invalidation, query re-execution, and client-side state convergence.

use std::{
    sync::Arc,
    time::Duration,
};

use application::{
    Application,
};
use async_trait::async_trait;
use common::{
    assert_obj,
    runtime::{
        Runtime,
        SpawnHandle,
    },
};
use futures::{
    future::BoxFuture,
    FutureExt,
};
use runtime::testing::TestRuntime;
use serde_json::json;

use super::helpers::{
    call_mutation,
    call_query,
    deploy_js,
};
use crate::{
    framework::scenario::{
        Scenario,
        TestRun,
    },
    simulation::{
        js_client::JsClientThread,
        server::ServerThread,
    },
};

#[derive(Clone)]
pub struct SubscriptionJsScenario;

#[async_trait]
impl Scenario for SubscriptionJsScenario {
    type TestRun = SubscriptionJsRun;

    fn name(&self) -> &'static str {
        "subscription_js"
    }

    async fn start_run(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<SubscriptionJsRun> {
        deploy_js(application).await?;
        tracing::info!("[subscription_js] Deployed JS");

        let counter_id_value =
            call_mutation(application, "subscription:initCounter", vec![json!({})])
                .await?;
        let counter_id: String = serde_json::from_value(counter_id_value)?;
        tracing::info!("[subscription_js] Initialized counter: {counter_id}");

        let app: Arc<dyn application::api::ApplicationApi> =
            Arc::new(application.clone());
        let mut handles: Vec<Box<dyn SpawnHandle>> = vec![];

        let (server, handle) = ServerThread::new(
            rt.clone(),
            app,
            Some(Duration::from_secs(1)), // network delay (mean, geometric dist)
        );
        handles.push(handle);

        let (js_client, handle) = JsClientThread::new(rt.clone(), server.clone());
        handles.push(handle);

        let query_token = js_client
            .add_query(
                "subscription:getCounter".parse()?,
                assert_obj!("counterId" => counter_id.clone()),
            )
            .await?;
        tracing::info!("[subscription_js] Subscribed to counter query");

        // Wait a bit for the initial subscription to resolve.
        rt.wait(Duration::from_secs(2)).await;

        Ok(SubscriptionJsRun {
            counter_id,
            server,
            js_client,
            query_token,
            _handles: handles,
        })
    }
}

pub struct SubscriptionJsRun {
    counter_id: String,
    server: ServerThread,
    js_client: JsClientThread,
    query_token: String,
    // Keep thread handles alive for the scenario lifetime.
    _handles: Vec<Box<dyn SpawnHandle>>,
}

#[async_trait]
impl TestRun for SubscriptionJsRun {
    type Output = usize; // final counter value from subscription

    fn run_transaction(
        &self,
        _rt: TestRuntime,
        _application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        // Run mutations via the ServerThread (which goes through the sync
        // protocol) rather than directly through the Application API.
        let server = self.server.clone();
        let counter_id = self.counter_id.clone();
        async move {
            server
                .mutation(
                    "subscription:increment".parse()?,
                    assert_obj!("counterId" => counter_id),
                )
                .await??;
            Ok(())
        }
        .boxed()
    }

    async fn validate(
        &self,
        _application: &Application<TestRuntime>,
    ) -> anyhow::Result<()> {
        // The subscription should eventually converge. We don't check
        // an exact value here since mutations may still be in flight.
        Ok(())
    }

    async fn finalize(
        &self,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<usize> {
        let server_ts = self.server.latest_timestamp().await?;
        self.js_client.wait_for_server_ts(server_ts).await?;

        let sub_value = self.js_client.query_result(self.query_token.clone()).await?;
        let counter_from_sub = match sub_value {
            Some(common::value::ConvexValue::Float64(f)) => f as usize,
            Some(common::value::ConvexValue::Int64(i)) => i as usize,
            other => anyhow::bail!("Unexpected subscription value: {other:?}"),
        };

        let direct_value = call_query(
            application,
            "subscription:getCounter",
            vec![json!({"counterId": self.counter_id})],
        )
        .await?;
        let counter_direct = direct_value.as_f64().unwrap_or(0.0) as usize;

        tracing::info!(
            "[subscription_js] Final: subscription={counter_from_sub}, direct={counter_direct}"
        );

        // The subscription value should match the direct read.
        anyhow::ensure!(
            counter_from_sub == counter_direct,
            "Subscription ({counter_from_sub}) != direct read ({counter_direct})"
        );

        Ok(counter_from_sub)
    }
}
