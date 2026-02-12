//! LinkRing scenario using real JavaScript UDFs through V8.
//!
//! M documents form a ring. Each JS mutation reverses a sub-chain.
//! Invariant (checked by JS query): ring is a single cycle of M elements.

use application::Application;
use async_trait::async_trait;
use common::runtime::Runtime;
use futures::{
    future::BoxFuture,
    FutureExt,
};
use rand::Rng;
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
pub struct LinkRingJsScenario {
    pub size: usize,
    pub rotate_length: usize,
}

impl Default for LinkRingJsScenario {
    fn default() -> Self {
        Self {
            size: 20,
            rotate_length: 5,
        }
    }
}

#[async_trait]
impl Scenario for LinkRingJsScenario {
    type TestRun = LinkRingJsRun;

    fn name(&self) -> &'static str {
        "link_ring_js"
    }

    async fn start_run(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<LinkRingJsRun> {
        deploy_js(application).await?;
        tracing::info!("[link_ring_js] Deployed JS");

        let result = call_mutation(
            application,
            "link_ring:setup",
            vec![json!({"size": self.size as f64})],
        )
        .await?;
        let ids: Vec<String> = serde_json::from_value(result)?;
        tracing::info!(
            "[link_ring_js] Initialized ring of {} elements, rotate_length={}",
            ids.len(),
            self.rotate_length
        );

        Ok(LinkRingJsRun {
            ids,
            size: self.size,
            rotate_length: self.rotate_length,
        })
    }
}

pub struct LinkRingJsRun {
    ids: Vec<String>,
    size: usize,
    rotate_length: usize,
}

#[async_trait]
impl TestRun for LinkRingJsRun {
    type Output = ();

    fn run_transaction(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let ids = self.ids.clone();
        let size = self.size;
        let rotate_length = self.rotate_length;
        let application = application.clone();
        let start_idx = rt.rng().random_range(0..size);

        async move {
            call_mutation(
                &application,
                "link_ring:rotate",
                vec![json!({
                    "ids": ids,
                    "startIdx": start_idx as f64,
                    "rotateLength": rotate_length as f64,
                })],
            )
            .await?;
            Ok(())
        }
        .boxed()
    }

    async fn validate(&self, application: &Application<TestRuntime>) -> anyhow::Result<()> {
        call_query(
            application,
            "link_ring:validate",
            vec![json!({"ids": self.ids, "expectedSize": self.size as f64})],
        )
        .await?;
        Ok(())
    }

    async fn finalize(&self, application: &Application<TestRuntime>) -> anyhow::Result<()> {
        self.validate(application).await
    }
}
