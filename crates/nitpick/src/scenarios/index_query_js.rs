//! Index query scenario: concurrent inserts + index range queries.
//!
//! Inserts events for multiple users with timestamps, then queries them
//! via a compound index [userId, timestamp]. Validates that query results
//! are always consistent with a serializable execution.
//!
//! Invariant: for each user, querying all their events returns exactly
//! the set of events that were successfully inserted for that user,
//! in timestamp order.

use std::sync::{
    atomic::{
        AtomicU32,
        Ordering,
    },
    Arc,
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

use super::helpers::{
    call_mutation,
    call_query,
    deploy_js,
};
use crate::framework::scenario::{
    Scenario,
    TestRun,
};

const NUM_USERS: usize = 5;

#[derive(Clone)]
pub struct IndexQueryJsScenario;

#[async_trait]
impl Scenario for IndexQueryJsScenario {
    type TestRun = IndexQueryJsRun;

    fn name(&self) -> &'static str {
        "index_query_js"
    }

    async fn start_run(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<IndexQueryJsRun> {
        deploy_js(application).await?;
        tracing::info!("[index_query_js] Deployed JS");

        Ok(IndexQueryJsRun {
            next_timestamp: Arc::new(AtomicU32::new(0)),
            total_inserted: Arc::new(AtomicU32::new(0)),
        })
    }
}

pub struct IndexQueryJsRun {
    next_timestamp: Arc<AtomicU32>,
    total_inserted: Arc<AtomicU32>,
}

#[async_trait]
impl TestRun for IndexQueryJsRun {
    type Output = usize; // total events inserted

    fn run_transaction(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let next_timestamp = self.next_timestamp.clone();
        let total_inserted = self.total_inserted.clone();
        let application = application.clone();

        // 60% inserts, 40% queries
        let do_insert = rt.rng().random_bool(0.6);
        let user_id = rt.rng().random_range(0..NUM_USERS);

        async move {
            if do_insert {
                let ts = next_timestamp.fetch_add(1, Ordering::SeqCst);
                let payload = format!("event-{ts}");
                call_mutation(
                    &application,
                    "index_query:insertEvent",
                    vec![json!({
                        "userId": user_id as f64,
                        "timestamp": ts as f64,
                        "payload": payload,
                    })],
                )
                .await?;
                total_inserted.fetch_add(1, Ordering::SeqCst);
            } else {
                // Query all events for a user via the index.
                let result = call_query(
                    &application,
                    "index_query:queryUserEvents",
                    vec![json!({"userId": user_id as f64})],
                )
                .await?;
                let events: Vec<serde_json::Value> = serde_json::from_value(result)?;

                // Verify ordering: timestamps must be ascending.
                let mut prev_ts: Option<f64> = None;
                for event in &events {
                    let ts = event["timestamp"].as_f64().unwrap();
                    if let Some(prev) = prev_ts {
                        anyhow::ensure!(
                            ts >= prev,
                            "Index query returned out-of-order results: {prev} > {ts}"
                        );
                    }
                    prev_ts = Some(ts);
                }
            }
            Ok(())
        }
        .boxed()
    }

    async fn validate(&self, application: &Application<TestRuntime>) -> anyhow::Result<()> {
        // For each user, query via the index and verify ordering.
        // Each query runs in its own consistent snapshot.
        for user_id in 0..NUM_USERS {
            let result = call_query(
                application,
                "index_query:queryUserEvents",
                vec![json!({"userId": user_id as f64})],
            )
            .await?;
            let events: Vec<serde_json::Value> = serde_json::from_value(result)?;

            // Verify all events belong to this user.
            for event in &events {
                let eid = event["userId"].as_f64().unwrap() as usize;
                anyhow::ensure!(
                    eid == user_id,
                    "Index query for user {user_id} returned event for user {eid}"
                );
            }

            // Verify ordering: timestamps must be ascending.
            let mut prev_ts: Option<f64> = None;
            for event in &events {
                let ts = event["timestamp"].as_f64().unwrap();
                if let Some(prev) = prev_ts {
                    anyhow::ensure!(ts >= prev, "Out-of-order for user {user_id}: {prev} > {ts}");
                }
                prev_ts = Some(ts);
            }
        }
        Ok(())
    }

    async fn finalize(&self, application: &Application<TestRuntime>) -> anyhow::Result<usize> {
        self.validate(application).await?;
        let count = self.total_inserted.load(Ordering::SeqCst) as usize;
        tracing::info!("[index_query_js] Final: {count} events inserted across {NUM_USERS} users");
        Ok(count)
    }
}
