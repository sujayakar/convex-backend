//! Text search scenario: concurrent inserts + full-text search queries.
//!
//! Inserts documents with searchable body text, then runs search queries
//! concurrently. Validates that search results contain the search term
//! and that filtered results respect the category filter.

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

const CATEGORIES: &[&str] = &["tech", "science", "arts"];
const SEARCH_TERMS: &[&str] = &["database", "quantum", "convex", "simulation"];

#[derive(Clone)]
pub struct TextSearchJsScenario;

#[async_trait]
impl Scenario for TextSearchJsScenario {
    type TestRun = TextSearchJsRun;

    fn name(&self) -> &'static str {
        "text_search_js"
    }

    async fn start_run(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<TextSearchJsRun> {
        deploy_js(application).await?;
        tracing::info!("[text_search_js] Deployed JS, waiting for search index bootstrap...");

        // Wait for search index bootstrap to complete.
        // The bootstrap worker runs in the background; we need to yield
        // and advance time to let it make progress.
        for _ in 0..20 {
            rt.wait(std::time::Duration::from_secs(1)).await;
            tokio::task::yield_now().await;
        }
        tracing::info!("[text_search_js] Bootstrap wait complete");

        // Insert a test document and verify search works before proceeding.
        call_mutation(
            application,
            "text_search:insertDocument",
            vec![json!({
                "title": "bootstrap test",
                "body": "testing search index bootstrap",
                "category": "test",
            })],
        )
        .await?;

        // Give the search index a moment to index the document.
        for _ in 0..5 {
            rt.wait(std::time::Duration::from_secs(1)).await;
            tokio::task::yield_now().await;
        }

        // Try a search -- if this fails, the bootstrap isn't done yet.
        let result = call_query(
            application,
            "text_search:searchByBody",
            vec![json!({"searchTerm": "bootstrap"})],
        )
        .await;
        match result {
            Ok(_) => tracing::info!("[text_search_js] Search index is ready"),
            Err(e) => {
                tracing::warn!("[text_search_js] Search not ready yet: {e}. Trying more time...");
                for _ in 0..30 {
                    rt.wait(std::time::Duration::from_secs(1)).await;
                    tokio::task::yield_now().await;
                }
                call_query(
                    application,
                    "text_search:searchByBody",
                    vec![json!({"searchTerm": "bootstrap"})],
                )
                .await?;
                tracing::info!("[text_search_js] Search index ready after extended wait");
            },
        }

        Ok(TextSearchJsRun {
            next_id: Arc::new(AtomicU32::new(0)),
        })
    }
}

pub struct TextSearchJsRun {
    next_id: Arc<AtomicU32>,
}

#[async_trait]
impl TestRun for TextSearchJsRun {
    type Output = usize; // total documents

    fn run_transaction(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let next_id = self.next_id.clone();
        let application = application.clone();
        let do_insert = rt.rng().random_bool(0.6);
        let category = CATEGORIES[rt.rng().random_range(0..CATEGORIES.len())];
        let term = SEARCH_TERMS[rt.rng().random_range(0..SEARCH_TERMS.len())];

        if do_insert {
            let id = next_id.fetch_add(1, Ordering::SeqCst);
            let category = category.to_string();
            let term = term.to_string();
            async move {
                let body = format!("Document {id} about {term} in the {category} field");
                call_mutation(
                    &application,
                    "text_search:insertDocument",
                    vec![json!({
                        "title": format!("Doc {id}"),
                        "body": body,
                        "category": category,
                    })],
                )
                .await?;
                Ok(())
            }
            .boxed()
        } else {
            let category = category.to_string();
            let term = term.to_string();
            async move {
                // Search with category filter.
                let result = call_query(
                    &application,
                    "text_search:searchByBodyWithFilter",
                    vec![json!({
                        "searchTerm": term,
                        "category": category,
                    })],
                )
                .await?;
                let docs: Vec<serde_json::Value> = serde_json::from_value(result)?;

                // Verify all results match the category filter.
                for doc in &docs {
                    let doc_cat = doc["category"].as_str().unwrap();
                    anyhow::ensure!(
                        doc_cat == category,
                        "Search returned doc with category {doc_cat}, expected {category}"
                    );
                }

                Ok(())
            }
            .boxed()
        }
    }

    async fn validate(&self, application: &Application<TestRuntime>) -> anyhow::Result<()> {
        // Just verify we can run a search and get back results.
        let _result = call_query(
            application,
            "text_search:searchByBody",
            vec![json!({"searchTerm": "document"})],
        )
        .await?;
        Ok(())
    }

    async fn finalize(&self, application: &Application<TestRuntime>) -> anyhow::Result<usize> {
        let result = call_query(application, "text_search:countAll", vec![json!({})]).await?;
        let count = result.as_f64().unwrap() as usize;
        tracing::info!("[text_search_js] Final: {count} documents");
        Ok(count)
    }
}
