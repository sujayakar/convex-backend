//! Link Ring scenario: a circular ring of linked documents.
//!
//! M documents form a ring where each document stores a `next` index pointing
//! to the next element. Each transaction picks a random position and reverses
//! a sub-chain of length N within the ring.
//!
//! Invariant: the ring is always a single cycle of exactly M elements.
//! This implies all concurrent sub-chain reversals are properly serialized.

use std::sync::LazyLock;

use application::Application;
use async_trait::async_trait;
use common::{
    assert_obj,
    runtime::Runtime,
    types::TableName,
    value::{
        ConvexObject,
        ConvexValue,
        ResolvedDocumentId,
        TableNamespace,
    },
};
use database::{
    TableModel,
    TestFacingModel,
    UserFacingModel,
};
use futures::{
    future::BoxFuture,
    FutureExt,
};
use keybroker::Identity;
use rand::Rng;
use runtime::testing::TestRuntime;

use crate::framework::scenario::{
    Scenario,
    TestRun,
};

static TABLE: LazyLock<TableName> = LazyLock::new(|| "ring".parse().unwrap());

#[derive(Clone)]
pub struct LinkRingScenario {
    /// Total number of links in the ring.
    pub size: usize,
    /// Length of the sub-chain to reverse in each transaction.
    pub rotate_length: usize,
}

impl Default for LinkRingScenario {
    fn default() -> Self {
        Self {
            size: 20,
            rotate_length: 5,
        }
    }
}

#[async_trait]
impl Scenario for LinkRingScenario {
    type TestRun = LinkRingRun;

    fn name(&self) -> &'static str {
        "link_ring"
    }

    async fn start_run(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<LinkRingRun> {
        let mut tx = application.begin(Identity::system()).await?;
        TableModel::new(&mut tx)
            .insert_table_metadata(TableNamespace::test_user(), &TABLE)
            .await?;

        // Create M documents. Each has an "idx" field (0..M) and a "next"
        // field pointing to the next index in the ring.
        let mut doc_ids = vec![];
        for i in 0..self.size {
            let next = (i + 1) % self.size;
            let doc_id = TestFacingModel::new(&mut tx)
                .insert(&TABLE, ring_obj(i as i64, next as i64))
                .await?;
            doc_ids.push(doc_id);
        }
        application.commit_test(tx).await?;

        tracing::info!(
            "[link_ring] Initialized ring of {} elements, rotate_length={}",
            self.size,
            self.rotate_length
        );

        Ok(LinkRingRun {
            size: self.size,
            rotate_length: self.rotate_length,
            doc_ids,
        })
    }
}

pub struct LinkRingRun {
    size: usize,
    rotate_length: usize,
    /// Document IDs indexed by their ring index (0..size).
    doc_ids: Vec<ResolvedDocumentId>,
}

#[async_trait]
impl TestRun for LinkRingRun {
    type Output = ();

    fn run_transaction(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let size = self.size;
        let rotate_length = self.rotate_length;
        let doc_ids = self.doc_ids.clone();
        let application = application.clone();

        let future = async move {
            let mut tx = application.begin(Identity::system()).await?;

            // Pick a random starting index.
            let start_idx = rt.rng().random_range(0..size);

            // Walk rotate_length + 1 steps from start, collecting the indices.
            let mut chain_indices = vec![start_idx];
            let mut current_idx = start_idx;
            for _ in 0..(rotate_length + 1) {
                let doc = tx
                    .get(doc_ids[current_idx])
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Missing doc at index {current_idx}"))?;
                let next_idx = get_next(doc.value())?;
                chain_indices.push(next_idx);
                current_idx = next_idx;
            }

            // chain_indices = [a, b, c, d, e, f] with rotate_length=4 transforms:
            //   a→e, b→f, c→b, d→c
            // Effectively reverses [b, c, d, e] so a→e→d→c→b→f.

            let first = chain_indices[0];
            let second = chain_indices[1];
            let second_to_last = chain_indices[chain_indices.len() - 2];
            let last = chain_indices[chain_indices.len() - 1];

            // First element now points to second-to-last.
            UserFacingModel::new_root_for_test(&mut tx)
                .replace(
                    doc_ids[first].into(),
                    ring_obj(first as i64, second_to_last as i64),
                )
                .await?;

            // Second element now points to last (end of chain).
            UserFacingModel::new_root_for_test(&mut tx)
                .replace(doc_ids[second].into(), ring_obj(second as i64, last as i64))
                .await?;

            // Reverse the middle section: each middle element points to its
            // predecessor in the original chain.
            let mut target = second;
            for &key in chain_indices.iter().take(rotate_length + 1).skip(2) {
                UserFacingModel::new_root_for_test(&mut tx)
                    .replace(doc_ids[key].into(), ring_obj(key as i64, target as i64))
                    .await?;
                target = key;
            }

            application.commit_test(tx).await?;
            Ok(())
        };
        future.boxed()
    }

    async fn validate(&self, application: &Application<TestRuntime>) -> anyhow::Result<()> {
        validate_ring(application, &self.doc_ids, self.size).await
    }

    async fn finalize(&self, application: &Application<TestRuntime>) -> anyhow::Result<()> {
        validate_ring(application, &self.doc_ids, self.size).await
    }
}

async fn validate_ring(
    application: &Application<TestRuntime>,
    doc_ids: &[ResolvedDocumentId],
    expected_size: usize,
) -> anyhow::Result<()> {
    let mut tx = application.begin(Identity::system()).await?;

    // Walk the ring starting from index 0.
    let mut visited = std::collections::BTreeSet::new();
    visited.insert(0usize);
    let mut current = 0;

    for hop in 0..(expected_size - 1) {
        let doc = tx
            .get(doc_ids[current])
            .await?
            .ok_or_else(|| anyhow::anyhow!("Missing doc at index {current}"))?;
        let next = get_next(doc.value())?;
        anyhow::ensure!(
            next < expected_size,
            "Next index out of range: {next} >= {expected_size}"
        );
        anyhow::ensure!(
            visited.insert(next),
            "Duplicate in ring! Visited index {next} twice after {} hops",
            hop + 1
        );
        current = next;
    }

    // After size-1 hops, next should be 0.
    let doc = tx
        .get(doc_ids[current])
        .await?
        .ok_or_else(|| anyhow::anyhow!("Missing doc at end of ring: {current}"))?;
    let final_next = get_next(doc.value())?;
    anyhow::ensure!(
        final_next == 0,
        "Ring didn't loop back to 0: last link points to {final_next}"
    );

    Ok(())
}

fn ring_obj(idx: i64, next: i64) -> ConvexObject {
    assert_obj!("idx" => idx, "next" => next)
}

fn get_next(obj: &ConvexObject) -> anyhow::Result<usize> {
    let next = obj
        .get("next")
        .ok_or_else(|| anyhow::anyhow!("Missing 'next' field"))?;
    if let ConvexValue::Int64(n) = next {
        Ok(*n as usize)
    } else {
        anyhow::bail!("'next' field is not Int64: {next:?}")
    }
}
