//! Pagination scenario: concurrent inserts while paginating.
//!
//! Inserts items into a table with an index, then paginates through results
//! while concurrent inserts are happening. Validates that pagination produces
//! a consistent snapshot and that all items in a category are correctly
//! ordered by value.

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

const CATEGORIES: &[&str] = &["alpha", "beta", "gamma"];
const PAGE_SIZE: usize = 5;

#[derive(Clone)]
pub struct PaginationJsScenario;

#[async_trait]
impl Scenario for PaginationJsScenario {
    type TestRun = PaginationJsRun;

    fn name(&self) -> &'static str {
        "pagination_js"
    }

    async fn start_run(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<PaginationJsRun> {
        deploy_js(application).await?;
        tracing::info!("[pagination_js] Deployed JS");
        Ok(PaginationJsRun {
            next_value: Arc::new(AtomicU32::new(0)),
        })
    }
}

pub struct PaginationJsRun {
    next_value: Arc<AtomicU32>,
}

#[async_trait]
impl TestRun for PaginationJsRun {
    type Output = usize; // total items

    fn run_transaction(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let next_value = self.next_value.clone();
        let application = application.clone();
        let do_insert = rt.rng().random_bool(0.5);
        let category = CATEGORIES[rt.rng().random_range(0..CATEGORIES.len())];

        if do_insert {
            let value = next_value.fetch_add(1, Ordering::SeqCst);
            async move {
                call_mutation(
                    &application,
                    "pagination:insertItem",
                    vec![json!({
                        "value": value as f64,
                        "category": category,
                    })],
                )
                .await?;
                Ok(())
            }
            .boxed()
        } else {
            // Paginate through all items in this category.
            let category = category.to_string();
            async move {
                let mut all_ids = Vec::new();
                let mut cursor = serde_json::Value::Null;
                let mut pages = 0;

                loop {
                    let pagination_opts = if cursor.is_null() {
                        json!({ "numItems": PAGE_SIZE, "cursor": null })
                    } else {
                        json!({ "numItems": PAGE_SIZE, "cursor": cursor })
                    };

                    let result = call_query(
                        &application,
                        "pagination:paginateByCategory",
                        vec![json!({
                            "category": category,
                            "paginationOpts": pagination_opts,
                        })],
                    )
                    .await?;

                    let page = result["page"]
                        .as_array()
                        .ok_or_else(|| anyhow::anyhow!("Missing page in pagination result"))?;

                    // Verify ordering within the page.
                    let mut prev_value: Option<f64> = None;
                    for item in page {
                        let v = item["value"].as_f64().unwrap();
                        if let Some(prev) = prev_value {
                            anyhow::ensure!(
                                v >= prev,
                                "Pagination order violation: {prev} > {v}"
                            );
                        }
                        prev_value = Some(v);
                        all_ids.push(item["_id"].as_str().unwrap().to_string());
                    }

                    pages += 1;
                    let is_done = result["isDone"].as_bool().unwrap_or(true);
                    if is_done {
                        break;
                    }
                    cursor = result["continueCursor"].clone();
                    anyhow::ensure!(pages < 100, "Pagination loop exceeded 100 pages");
                }

                // Verify no duplicate IDs across pages.
                let unique_count = {
                    let mut set = std::collections::HashSet::new();
                    for id in &all_ids {
                        set.insert(id.clone());
                    }
                    set.len()
                };
                anyhow::ensure!(
                    unique_count == all_ids.len(),
                    "Pagination returned duplicates: {} unique out of {} total",
                    unique_count,
                    all_ids.len()
                );

                Ok(())
            }
            .boxed()
        }
    }

    async fn validate(&self, application: &Application<TestRuntime>) -> anyhow::Result<()> {
        // For each category, verify collect() results are ordered.
        for cat in CATEGORIES {
            let result = call_query(
                application,
                "pagination:allByCategory",
                vec![json!({"category": cat})],
            )
            .await?;
            let items: Vec<serde_json::Value> = serde_json::from_value(result)?;
            let mut prev: Option<f64> = None;
            for item in &items {
                let v = item["value"].as_f64().unwrap();
                if let Some(p) = prev {
                    anyhow::ensure!(v >= p, "Category {cat}: order violation {p} > {v}");
                }
                prev = Some(v);
            }
        }
        Ok(())
    }

    async fn finalize(&self, application: &Application<TestRuntime>) -> anyhow::Result<usize> {
        self.validate(application).await?;
        let mut total = 0;
        for cat in CATEGORIES {
            let result = call_query(
                application,
                "pagination:countByCategory",
                vec![json!({"category": cat})],
            )
            .await?;
            let count = result.as_f64().unwrap() as usize;
            total += count;
        }
        tracing::info!("[pagination_js] Final: {total} items across {} categories", CATEGORIES.len());
        Ok(total)
    }
}
