//! Counter scenario using real JavaScript UDFs through V8.
//!
//! Two documents (linear, triangle counter). Each JS mutation increments
//! linear and adds to triangle. Invariant: `triangle == (linear+1)*linear/2`.
//! Validation is also done via a JS query.

use application::Application;
use async_trait::async_trait;
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
use crate::framework::scenario::{
    Scenario,
    TestRun,
};

#[derive(Clone)]
pub struct CounterJsScenario;

#[async_trait]
impl Scenario for CounterJsScenario {
    type TestRun = CounterJsRun;

    fn name(&self) -> &'static str {
        "counter_js"
    }

    async fn start_run(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<CounterJsRun> {
        deploy_js(application).await?;
        tracing::info!("[counter_js] Deployed JS");

        let ids = call_mutation(application, "counter:setup", vec![json!({})]).await?;
        let linear_id = ids["linearId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing linearId"))?
            .to_string();
        let triangle_id = ids["triangleId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing triangleId"))?
            .to_string();
        tracing::info!("[counter_js] Initialized: linear={linear_id}, triangle={triangle_id}");

        Ok(CounterJsRun {
            linear_id,
            triangle_id,
        })
    }
}

pub struct CounterJsRun {
    linear_id: String,
    triangle_id: String,
}

#[async_trait]
impl TestRun for CounterJsRun {
    type Output = i64;

    fn run_transaction(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let linear_id = self.linear_id.clone();
        let triangle_id = self.triangle_id.clone();
        let application = application.clone();
        async move {
            call_mutation(
                &application,
                "counter:increment",
                vec![json!({"linearId": linear_id, "triangleId": triangle_id})],
            )
            .await?;
            Ok(())
        }
        .boxed()
    }

    async fn validate(&self, application: &Application<TestRuntime>) -> anyhow::Result<()> {
        call_query(
            application,
            "counter:validate",
            vec![json!({"linearId": self.linear_id, "triangleId": self.triangle_id})],
        )
        .await?;
        Ok(())
    }

    async fn finalize(&self, application: &Application<TestRuntime>) -> anyhow::Result<i64> {
        let value = call_query(
            application,
            "counter:validate",
            vec![json!({"linearId": self.linear_id, "triangleId": self.triangle_id})],
        )
        .await?;
        let triangle = value["triangle"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("Missing triangle"))? as i64;
        tracing::info!("[counter_js] Final triangle value: {triangle}");
        Ok(triangle)
    }
}
