//! SDK Client proof-of-concept scenario.
//!
//! Demonstrates that the real ConvexClient JS SDK runs correctly inside a
//! deterministic V8 isolate, connected to the ServerThread via simulated
//! WebSocket channels.
//!
//! The scenario:
//! 1. Deploys JS UDFs (subscription module with counter queries/mutations)
//! 2. Creates an SdkClientThread with ConvexClient connected to ServerThread
//! 3. Subscribes to a counter query via ConvexClient.onUpdate
//! 4. Runs mutations that increment the counter
//! 5. Verifies the subscription receives the updates

use std::{
    sync::Arc,
    time::Duration,
};

use application::Application;
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
        sdk_client::SdkClientThread,
        server::ServerThread,
    },
};

#[derive(Clone)]
pub struct SdkClientPocScenario;

#[async_trait]
impl Scenario for SdkClientPocScenario {
    type TestRun = SdkClientPocRun;

    fn name(&self) -> &'static str {
        "sdk_client_poc"
    }

    async fn start_run(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<SdkClientPocRun> {
        deploy_js(application).await?;
        tracing::info!("[sdk_client_poc] Deployed JS");

        // Initialize a counter document.
        let counter_id_value =
            call_mutation(application, "subscription:initCounter", vec![json!({})])
                .await?;
        let counter_id: String = serde_json::from_value(counter_id_value)?;
        tracing::info!("[sdk_client_poc] Initialized counter: {counter_id}");

        // Create server + SDK client threads.
        let app: Arc<dyn application::api::ApplicationApi> =
            Arc::new(application.clone());
        let mut handles: Vec<Box<dyn SpawnHandle>> = vec![];

        let (server, handle) = ServerThread::new(
            rt.clone(),
            app,
            Some(Duration::from_secs(1)),
        );
        handles.push(handle);

        let (sdk_client, handle) = SdkClientThread::new(rt.clone(), server.clone());
        handles.push(handle);

        // Subscribe to the counter query via the ConvexClient.
        let subscription_id = sdk_client
            .subscribe(
                "subscription:getCounter",
                assert_obj!("counterId" => counter_id.clone()),
            )
            .await?;
        tracing::info!("[sdk_client_poc] Subscribed via ConvexClient (id={subscription_id})");

        // Wait a bit for the initial subscription to resolve.
        rt.wait(Duration::from_secs(2)).await;

        Ok(SdkClientPocRun {
            counter_id,
            server,
            sdk_client,
            subscription_id,
            handles,
        })
    }
}

pub struct SdkClientPocRun {
    counter_id: String,
    server: ServerThread,
    sdk_client: SdkClientThread,
    subscription_id: u32,
    #[allow(dead_code)]
    handles: Vec<Box<dyn SpawnHandle>>,
}

#[async_trait]
impl TestRun for SdkClientPocRun {
    type Output = usize;

    fn run_transaction(
        &self,
        _rt: TestRuntime,
        _application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
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
        // The subscription should eventually converge.
        Ok(())
    }

    async fn finalize(
        &self,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<usize> {
        // Wait for the SDK client to observe the latest server state.
        let server_ts = self.server.latest_timestamp().await?;
        self.sdk_client.wait_for_server_ts(server_ts).await?;

        // Get the subscription result via ConvexClient's callback-based API.
        let sub_value = self
            .sdk_client
            .get_subscription_result(self.subscription_id)
            .await?;
        let counter_from_sub = match sub_value {
            Some(Ok(common::value::ConvexValue::Float64(f))) => f as usize,
            Some(Ok(common::value::ConvexValue::Int64(i))) => i as usize,
            other => anyhow::bail!("Unexpected subscription value: {other:?}"),
        };

        // Verify against a direct query.
        let direct_value = call_query(
            application,
            "subscription:getCounter",
            vec![json!({"counterId": self.counter_id})],
        )
        .await?;
        let counter_direct = direct_value.as_f64().unwrap_or(0.0) as usize;

        tracing::info!(
            "[sdk_client_poc] Final: subscription={counter_from_sub}, direct={counter_direct}"
        );

        anyhow::ensure!(
            counter_from_sub == counter_direct,
            "Subscription ({counter_from_sub}) != direct read ({counter_direct})"
        );

        Ok(counter_from_sub)
    }
}
