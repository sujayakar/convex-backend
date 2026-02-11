use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use cmd_util::env::env_config;
use common::{
    assert_obj,
    document::CreationTime,
    schemas::{
        DatabaseSchema,
        DocumentSchema,
        TableDefinition,
    },
};
use keybroker::Identity;
use maplit::btreemap;
use proptest::prelude::*;
use proptest_derive::Arbitrary;
use runtime::testing::TestRuntime;
use value::{
    DeveloperDocumentId,
    TableName,
    TableNamespace,
};

use super::randomized_framework::{
    run_randomized_actions,
    RandomizedScenario,
};
use crate::{
    test_helpers::{
        DbFixtures,
        DbFixturesArgs,
    },
    Database,
    IndexModel,
    SchemaModel,
    UserFacingModel,
};

struct MonotonicScenario {
    rt: TestRuntime,
    db: Database<TestRuntime>,
    tp: Arc<dyn common::persistence::Persistence>,
    namespace: TableNamespace,
    table_name: TableName,
    monotonic_enabled: bool,
    lower_bound: Option<CreationTime>,
    ids: Vec<DeveloperDocumentId>,
}

impl MonotonicScenario {
    async fn apply_schema(&mut self, monotonic: bool) -> anyhow::Result<()> {
        let schema = DatabaseSchema {
            tables: btreemap! {
                self.table_name.clone() => TableDefinition {
                    table_name: self.table_name.clone(),
                    indexes: Default::default(),
                    staged_db_indexes: Default::default(),
                    text_indexes: Default::default(),
                    staged_text_indexes: Default::default(),
                    vector_indexes: Default::default(),
                    staged_vector_indexes: Default::default(),
                    document_type: Some(DocumentSchema::Any),
                    monotonic_creation_time: monotonic,
                }
            },
            schema_validation: true,
        };

        let mut tx = self.db.begin(Identity::system()).await?;
        let (schema_id, _) = SchemaModel::new(&mut tx, self.namespace)
            .submit_pending(schema)
            .await?;
        self.db.commit(tx).await?;

        let mut tx = self.db.begin(Identity::system()).await?;
        SchemaModel::new(&mut tx, self.namespace)
            .mark_validated(schema_id)
            .await?;
        self.db.commit(tx).await?;

        let mut tx = self.db.begin(Identity::system()).await?;
        let (_, next_schema) = SchemaModel::new(&mut tx, self.namespace)
            .apply(Some(schema_id))
            .await?;
        IndexModel::new(&mut tx)
            .apply(self.namespace, &next_schema)
            .await?;
        self.db.commit(tx).await?;

        self.monotonic_enabled = monotonic;
        if monotonic {
            // Start a fresh observed lower bound window for randomized checks.
            // The committer may carry a stronger internal bound than this.
            self.lower_bound = None;
        }
        Ok(())
    }

    async fn fetch_creation_time(
        &self,
        id: &DeveloperDocumentId,
    ) -> anyhow::Result<Option<CreationTime>> {
        let mut tx = self.db.begin(Identity::system()).await?;
        let resolved_id = tx.resolve_developer_id(id, self.namespace)?;
        Ok(tx.get(resolved_id).await?.map(|doc| doc.creation_time()))
    }

    async fn insert_many_and_check(&mut self, count: usize) -> anyhow::Result<()> {
        let mut tx = self.db.begin(Identity::system()).await?;
        let mut inserted = Vec::with_capacity(count);
        for i in 0..count {
            let doc = assert_obj!("value" => i as i64);
            let id = UserFacingModel::new(&mut tx, self.namespace)
                .insert(self.table_name.clone(), doc)
                .await?;
            inserted.push(id);
        }
        self.db.commit(tx).await?;

        let initial_lb = self.lower_bound;
        let mut max_ct_in_batch = initial_lb;
        for id in &inserted {
            let creation_time = self
                .fetch_creation_time(id)
                .await?
                .context("Inserted document should exist")?;
            if self.monotonic_enabled {
                if let Some(lb) = initial_lb {
                    assert!(
                        creation_time > lb,
                        "creation time {:?} must be > lower bound {:?}",
                        creation_time,
                        lb
                    );
                }
                max_ct_in_batch = Some(match max_ct_in_batch {
                    Some(existing) if existing > creation_time => existing,
                    _ => creation_time,
                });
            }
        }
        if self.monotonic_enabled {
            self.lower_bound = max_ct_in_batch;
        }
        self.ids.extend(inserted);
        Ok(())
    }

    async fn delete_random(&mut self, n: u8) -> anyhow::Result<()> {
        if self.ids.is_empty() {
            return Ok(());
        }
        let idx = (n as usize) % self.ids.len();
        let id = self.ids.remove(idx);
        let mut tx = self.db.begin(Identity::system()).await?;
        UserFacingModel::new(&mut tx, self.namespace)
            .delete(id)
            .await?;
        self.db.commit(tx).await?;
        Ok(())
    }

    async fn restart(&mut self) -> anyhow::Result<()> {
        let DbFixtures { db, tp, .. } = DbFixtures::new_with_args(
            &self.rt,
            DbFixturesArgs {
                tp: Some(self.tp.clone()),
                ..Default::default()
            },
        )
        .await?;
        self.db = db;
        self.tp = tp;
        if self.monotonic_enabled {
            // Reset observed lower bound after restart; subsequent enabled
            // inserts must still be strictly increasing from here.
            self.lower_bound = None;
        }
        Ok(())
    }
}

#[derive(Debug, Arbitrary)]
enum MonotonicAction {
    Enable,
    Disable,
    InsertOne(i64),
    InsertBatch(#[proptest(strategy = "1u8..6")] u8),
    DeleteRandom(u8),
    Restart,
}

#[async_trait]
impl RandomizedScenario for MonotonicScenario {
    type Action = MonotonicAction;

    async fn init(rt: TestRuntime) -> anyhow::Result<Self> {
        let DbFixtures { db, tp, .. } = DbFixtures::new(&rt).await?;
        let mut scenario = Self {
            rt,
            db,
            tp,
            namespace: TableNamespace::test_user(),
            table_name: "randomized_monotonic".parse()?,
            monotonic_enabled: false,
            lower_bound: None,
            ids: Vec::new(),
        };
        scenario.apply_schema(false).await?;
        Ok(scenario)
    }

    async fn apply_action(&mut self, action: Self::Action) -> anyhow::Result<()> {
        match action {
            MonotonicAction::Enable => {
                if !self.monotonic_enabled {
                    self.apply_schema(true).await?;
                }
            },
            MonotonicAction::Disable => {
                if self.monotonic_enabled {
                    self.apply_schema(false).await?;
                }
            },
            MonotonicAction::InsertOne(v) => {
                let mut tx = self.db.begin(Identity::system()).await?;
                let id = UserFacingModel::new(&mut tx, self.namespace)
                    .insert(self.table_name.clone(), assert_obj!("value" => v))
                    .await?;
                self.db.commit(tx).await?;

                let creation_time = self
                    .fetch_creation_time(&id)
                    .await?
                    .context("Inserted document should exist")?;
                if self.monotonic_enabled {
                    if let Some(lb) = self.lower_bound {
                        assert!(
                            creation_time > lb,
                            "creation time {:?} must be > lower bound {:?}",
                            creation_time,
                            lb
                        );
                    }
                    self.lower_bound = Some(creation_time);
                }
                self.ids.push(id);
            },
            MonotonicAction::InsertBatch(count) => {
                self.insert_many_and_check(count as usize).await?;
            },
            MonotonicAction::DeleteRandom(n) => {
                self.delete_random(n).await?;
            },
            MonotonicAction::Restart => {
                self.restart().await?;
            },
        }
        Ok(())
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32 * env_config("CONVEX_PROPTEST_MULTIPLIER", 1),
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn proptest_monotonic_creation_time_randomized(
        actions in prop::collection::vec(any::<MonotonicAction>(), 1..32)
    ) {
        run_randomized_actions::<MonotonicScenario>(actions);
    }
}
