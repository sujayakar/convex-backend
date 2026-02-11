//! Elle-style serializability test using real JavaScript UDFs through V8.
//!
//! Deploys the simulation package's elle.ts UDFs (initializeRegister,
//! appendRegister, getRegister) and runs concurrent mutations/queries
//! through the Application API. Verifies serializability via a dependency
//! graph cycle check on the event log.

use std::sync::{
    atomic::{
        AtomicU32,
        Ordering,
    },
    Arc,
    Mutex,
};

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

use super::{
    elle::verify_serializability_for_test,
    helpers::{
        call_mutation,
        call_query,
        deploy_js,
    },
};
use crate::framework::scenario::{
    Scenario,
    TestRun,
};

type WriteId = u32;
type TxId = usize;

#[derive(Clone)]
pub struct ElleJsScenario;

#[async_trait]
impl Scenario for ElleJsScenario {
    type TestRun = ElleJsRun;

    fn name(&self) -> &'static str {
        "elle_js"
    }

    async fn start_run(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<ElleJsRun> {
        deploy_js(application).await?;
        tracing::info!("[elle_js] Deployed JS");

        let result = call_mutation(application, "elle:initializeRegister", vec![json!({})]).await?;
        let register_id: String = serde_json::from_value(result)?;
        tracing::info!("[elle_js] Initialized register: {register_id}");

        Ok(ElleJsRun {
            register_id,
            next_write_id: Arc::new(AtomicU32::new(0)),
            next_tx_id: Arc::new(AtomicU32::new(0)),
            events: Arc::new(Mutex::new(vec![])),
        })
    }
}

pub struct ElleJsRun {
    register_id: String,
    next_write_id: Arc<AtomicU32>,
    next_tx_id: Arc<AtomicU32>,
    events: Arc<Mutex<Vec<(String, TxId, WriteId, Vec<WriteId>)>>>,
}

#[async_trait]
impl TestRun for ElleJsRun {
    type Output = usize;

    fn run_transaction(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let register_id = self.register_id.clone();
        let next_write_id = self.next_write_id.clone();
        let next_tx_id = self.next_tx_id.clone();
        let events = self.events.clone();
        let application = application.clone();
        let do_write = rt.rng().random_bool(0.6);

        async move {
            let tx_id = next_tx_id.fetch_add(1, Ordering::SeqCst) as TxId;

            if do_write {
                let write_id = next_write_id.fetch_add(1, Ordering::SeqCst);
                let result = call_mutation(
                    &application,
                    "elle:appendRegister",
                    vec![json!({"id": register_id, "value": write_id as f64})],
                )
                .await?;
                let write_ids: Vec<f64> = serde_json::from_value(result)?;
                let write_ids: Vec<WriteId> = write_ids.iter().map(|w| *w as WriteId).collect();
                events
                    .lock()
                    .unwrap()
                    .push(("write".into(), tx_id, write_id, write_ids));
            } else {
                let result = call_query(
                    &application,
                    "elle:getRegister",
                    vec![json!({"id": register_id})],
                )
                .await?;
                let write_ids: Vec<f64> = serde_json::from_value(result)?;
                let write_ids: Vec<WriteId> = write_ids.iter().map(|w| *w as WriteId).collect();
                events
                    .lock()
                    .unwrap()
                    .push(("read".into(), tx_id, 0, write_ids));
            }
            Ok(())
        }
        .boxed()
    }

    async fn validate(&self, _application: &Application<TestRuntime>) -> anyhow::Result<()> {
        Ok(())
    }

    async fn finalize(&self, _application: &Application<TestRuntime>) -> anyhow::Result<usize> {
        let events = self.events.lock().unwrap();
        let test_events: Vec<(&str, usize, u32, Vec<u32>)> = events
            .iter()
            .map(|(kind, tx_id, write_id, values)| {
                (kind.as_str(), *tx_id, *write_id, values.clone())
            })
            .collect();
        let num_events = test_events.len();
        tracing::info!(
            "[elle_js] Finalizing with {num_events} events ({} writes, {} reads)",
            events.iter().filter(|(k, ..)| k == "write").count(),
            events.iter().filter(|(k, ..)| k == "read").count(),
        );
        verify_serializability_for_test(&test_events)?;
        tracing::info!("[elle_js] Serializability check passed");
        Ok(num_events)
    }
}
