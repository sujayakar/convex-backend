//! Simple counter scenario: contended increment/decrement of a document value.
//!
//! Two documents: `linear` (incremented each transaction) and `triangle`
//! (triangle += linear each transaction).
//!
//! Invariant: `triangle == (linear + 1) * linear / 2`
//!
//! This implies transactions are serialized -- no two transactions can read
//! the same starting snapshot and both commit successfully.

use std::sync::LazyLock;

use anyhow::bail;
use application::Application;
use async_trait::async_trait;
use common::{
    assert_obj,
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
use runtime::testing::TestRuntime;

use crate::framework::scenario::{
    Scenario,
    TestRun,
};

static TABLE: LazyLock<TableName> = LazyLock::new(|| "counter".parse().unwrap());

#[derive(Clone)]
pub struct CounterScenario;

#[async_trait]
impl Scenario for CounterScenario {
    type TestRun = CounterRun;

    fn name(&self) -> &'static str {
        "counter"
    }

    async fn start_run(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<CounterRun> {
        let mut tx = application.begin(Identity::system()).await?;
        TableModel::new(&mut tx)
            .insert_table_metadata(TableNamespace::test_user(), &TABLE)
            .await?;
        let linear_id = TestFacingModel::new(&mut tx)
            .insert(&TABLE, object_from_count(0))
            .await?;
        let triangle_id = TestFacingModel::new(&mut tx)
            .insert(&TABLE, object_from_count(0))
            .await?;
        application.commit_test(tx).await?;
        tracing::info!(
            "[counter] Initialized: linear={linear_id}, triangle={triangle_id}"
        );
        Ok(CounterRun {
            linear_id,
            triangle_id,
        })
    }
}

pub struct CounterRun {
    linear_id: ResolvedDocumentId,
    triangle_id: ResolvedDocumentId,
}

#[async_trait]
impl TestRun for CounterRun {
    type Output = i64;

    fn run_transaction(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let linear_id = self.linear_id;
        let triangle_id = self.triangle_id;
        let application = application.clone();
        let future = async move {
            let mut tx = application.begin(Identity::system()).await?;

            let linear_doc = tx
                .get(linear_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Linear document not found"))?;
            let linear = count_from_object(linear_doc.value())?;

            let triangle_doc = tx
                .get(triangle_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Triangle document not found"))?;
            let triangle = count_from_object(triangle_doc.value())?;

            UserFacingModel::new_root_for_test(&mut tx)
                .replace(linear_id.into(), object_from_count(linear + 1))
                .await?;

            let linear_doc = tx
                .get(linear_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Linear document not found after replace"))?;
            let new_linear = count_from_object(linear_doc.value())?;
            anyhow::ensure!(
                new_linear == linear + 1,
                "Within-transaction read inconsistency"
            );

            UserFacingModel::new_root_for_test(&mut tx)
                .patch(
                    triangle_id.into(),
                    object_from_count(triangle + new_linear).into(),
                )
                .await?;

            application.commit_test(tx).await?;
            Ok(())
        };
        future.boxed()
    }

    async fn validate(
        &self,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<()> {
        let mut tx = application.begin(Identity::system()).await?;
        let linear_doc = tx
            .get(self.linear_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Linear document not found"))?;
        let triangle_doc = tx
            .get(self.triangle_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Triangle document not found"))?;
        let linear = count_from_object(linear_doc.value())?;
        let triangle = count_from_object(triangle_doc.value())?;
        let expected_triangle = (linear + 1) * linear / 2;
        anyhow::ensure!(
            expected_triangle == triangle,
            "Invariant violated: linear={linear}, triangle={triangle}, expected={expected_triangle}"
        );
        Ok(())
    }

    async fn finalize(
        &self,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<i64> {
        let mut tx = application.begin(Identity::system()).await?;
        let triangle_doc = tx
            .get(self.triangle_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Triangle document not found"))?;
        let triangle = count_from_object(triangle_doc.value())?;
        tracing::info!("[counter] Final triangle value: {triangle}");
        Ok(triangle)
    }
}

fn object_from_count(i: i64) -> ConvexObject {
    assert_obj!("count" => i)
}

fn count_from_object(o: &ConvexObject) -> anyhow::Result<i64> {
    let count = o
        .get("count")
        .ok_or_else(|| anyhow::anyhow!("count field not found"))?;
    if let ConvexValue::Int64(m) = count {
        if *m < 0 {
            bail!("Negative counter: {m}")
        }
        Ok(*m)
    } else {
        bail!("count field is not Int64: {count:?}")
    }
}
