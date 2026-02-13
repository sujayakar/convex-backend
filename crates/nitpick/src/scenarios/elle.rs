//! Elle-style append-register serializability test.
//!
//! Uses an append-only register pattern where multiple concurrent transactions
//! append values. After execution, builds a dependency graph (direct write
//! deps, direct read deps, anti-read deps) and checks it for cycles.
//!
//! A cycle in the dependency graph indicates a serializability violation.
//! This is based on the Elle framework: https://github.com/jepsen-io/elle

use std::{
    collections::{
        BTreeMap,
        BTreeSet,
    },
    sync::{
        atomic::{
            AtomicU32,
            Ordering,
        },
        Arc,
        Mutex,
    },
};

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

static TABLE: std::sync::LazyLock<TableName> =
    std::sync::LazyLock::new(|| "elle_register".parse().unwrap());

type WriteId = u32;
type TxId = usize;

/// Events recorded during execution for dependency graph construction.
#[derive(Clone, Debug)]
enum ElleEvent {
    Write {
        tx_id: TxId,
        write_id: WriteId,
        /// All write_ids seen after this write (the register state).
        register_after: Vec<WriteId>,
    },
    Read {
        tx_id: TxId,
        /// All write_ids seen at read time.
        register_value: Vec<WriteId>,
    },
}

#[derive(Clone)]
pub struct ElleScenario {
    pub num_transactions: usize,
}

impl Default for ElleScenario {
    fn default() -> Self {
        Self {
            num_transactions: 32,
        }
    }
}

#[async_trait]
impl Scenario for ElleScenario {
    type TestRun = ElleRun;

    fn name(&self) -> &'static str {
        "elle"
    }

    async fn start_run(
        &self,
        _rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<ElleRun> {
        let mut tx = application.begin(Identity::system()).await?;
        TableModel::new(&mut tx)
            .insert_table_metadata(TableNamespace::test_user(), &TABLE)
            .await?;
        let register_id = TestFacingModel::new(&mut tx)
            .insert(&TABLE, assert_obj!())
            .await?;
        application.commit_test(tx).await?;

        tracing::info!("[elle] Initialized register: {register_id}");

        Ok(ElleRun {
            register_id,
            next_write_id: Arc::new(AtomicU32::new(0)),
            next_tx_id: Arc::new(AtomicU32::new(0)),
            events: Arc::new(Mutex::new(vec![])),
        })
    }
}

pub struct ElleRun {
    register_id: ResolvedDocumentId,
    next_write_id: Arc<AtomicU32>,
    next_tx_id: Arc<AtomicU32>,
    events: Arc<Mutex<Vec<ElleEvent>>>,
}

#[async_trait]
impl TestRun for ElleRun {
    type Output = usize;

    // number of events

    fn run_transaction(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let register_id = self.register_id;
        let next_write_id = self.next_write_id.clone();
        let next_tx_id = self.next_tx_id.clone();
        let events = self.events.clone();
        let application = application.clone();

        let do_write = rt.rng().random_bool(0.5);
        let future = async move {
            let tx_id = next_tx_id.fetch_add(1, Ordering::SeqCst) as TxId;

            if do_write {
                let write_id = next_write_id.fetch_add(1, Ordering::SeqCst);

                let mut tx = application.begin(Identity::system()).await?;
                let doc = tx
                    .get(register_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Register not found"))?;
                let current = read_register(doc.value())?;
                let mut new_values = current;
                new_values.push(write_id);
                UserFacingModel::new_root_for_test(&mut tx)
                    .replace(register_id.into(), write_register(&new_values))
                    .await?;
                application.commit_test(tx).await?;

                events.lock().unwrap().push(ElleEvent::Write {
                    tx_id,
                    write_id,
                    register_after: new_values,
                });
            } else {
                let mut tx = application.begin(Identity::system()).await?;
                let doc = tx
                    .get(register_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Register not found"))?;
                let values = read_register(doc.value())?;
                drop(tx);

                events.lock().unwrap().push(ElleEvent::Read {
                    tx_id,
                    register_value: values,
                });
            }
            Ok(())
        };
        future.boxed()
    }

    async fn validate(&self, _application: &Application<TestRuntime>) -> anyhow::Result<()> {
        // Validation happens in finalize via the dependency graph.
        Ok(())
    }

    async fn finalize(&self, _application: &Application<TestRuntime>) -> anyhow::Result<usize> {
        let events = self.events.lock().unwrap().clone();
        let num_events = events.len();
        tracing::info!(
            "[elle] Finalizing with {} events ({} writes, {} reads)",
            num_events,
            events
                .iter()
                .filter(|e| matches!(e, ElleEvent::Write { .. }))
                .count(),
            events
                .iter()
                .filter(|e| matches!(e, ElleEvent::Read { .. }))
                .count(),
        );

        verify_serializability(&events)?;
        tracing::info!("[elle] Serializability check passed");
        Ok(num_events)
    }
}

/// Verify serializability by building a dependency graph and checking for
/// cycles.
fn verify_serializability(events: &[ElleEvent]) -> anyhow::Result<()> {
    let mut write_tx: BTreeMap<WriteId, TxId> = BTreeMap::new();
    // Register length -> tx_ids. Duplicates indicate a lost-update anomaly.
    let mut len_txs: BTreeMap<usize, Vec<TxId>> = BTreeMap::new();

    // Length 0 is the "initial" state (empty register).
    len_txs.insert(0, vec![]);

    for event in events {
        if let ElleEvent::Write {
            tx_id,
            write_id,
            register_after,
        } = event
        {
            write_tx.insert(*write_id, *tx_id);
            len_txs
                .entry(register_after.len())
                .or_default()
                .push(*tx_id);
        }
    }

    // If any length was produced by more than one transaction, that's a
    // lost-update anomaly (two writers read the same state and both committed).
    for (len, txs) in &len_txs {
        if txs.len() > 1 {
            anyhow::bail!(
                "Serializability violation: register length {len} produced by multiple \
                 transactions: {txs:?} (lost-update anomaly)"
            );
        }
    }

    let mut edges: BTreeSet<(TxId, TxId, &str)> = BTreeSet::new();
    let all_tx_ids: BTreeSet<TxId> = events
        .iter()
        .map(|e| match e {
            ElleEvent::Write { tx_id, .. } => *tx_id,
            ElleEvent::Read { tx_id, .. } => *tx_id,
        })
        .collect();

    for event in events {
        match event {
            ElleEvent::Write {
                tx_id,
                register_after,
                ..
            } => {
                // Direct write dependency: the previous write (at len-1)
                // must happen before this one.
                if !register_after.is_empty()
                    && let Some(prev_txs) = len_txs.get(&(register_after.len() - 1))
                {
                    for &prev_tx in prev_txs {
                        edges.insert((prev_tx, *tx_id, "WW"));
                    }
                }
            },
            ElleEvent::Read {
                tx_id,
                register_value,
            } => {
                // Direct read dependency: this read depends on the last write.
                if let Some(last_write_id) = register_value.last()
                    && let Some(&write_tx_id) = write_tx.get(last_write_id)
                {
                    edges.insert((write_tx_id, *tx_id, "WR"));
                }
                // Anti-read dependency: the next write after what we read
                // must come after this read.
                if let Some(next_txs) = len_txs.get(&(register_value.len() + 1)) {
                    for &next_tx in next_txs {
                        edges.insert((*tx_id, next_tx, "RW"));
                    }
                }
            },
        }
    }

    tracing::info!(
        "[elle] Dependency graph: {} nodes, {} edges",
        all_tx_ids.len(),
        edges.len()
    );

    // Check for cycles using DFS.
    check_acyclic(&all_tx_ids, &edges)
}

/// Test helper: verify serializability from a simplified event format.
/// Each tuple is (kind, tx_id, write_id, register_values).
/// kind is "write" or "read". write_id is ignored for reads.
pub fn verify_serializability_for_test(
    events: &[(&str, usize, u32, Vec<u32>)],
) -> anyhow::Result<()> {
    let elle_events: Vec<ElleEvent> = events
        .iter()
        .map(|(kind, tx_id, write_id, values)| match *kind {
            "write" => ElleEvent::Write {
                tx_id: *tx_id,
                write_id: *write_id,
                register_after: values.clone(),
            },
            "read" => ElleEvent::Read {
                tx_id: *tx_id,
                register_value: values.clone(),
            },
            _ => panic!("Unknown event kind: {kind}"),
        })
        .collect();
    verify_serializability(&elle_events)
}

fn check_acyclic(
    nodes: &BTreeSet<TxId>,
    edges: &BTreeSet<(TxId, TxId, &str)>,
) -> anyhow::Result<()> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Unvisited,
        InProgress,
        Done,
    }

    let mut state: BTreeMap<TxId, State> = nodes.iter().map(|n| (*n, State::Unvisited)).collect();

    for &start in nodes {
        if state[&start] != State::Unvisited {
            continue;
        }
        let mut stack = vec![(start, true)];
        while let Some((node, entering)) = stack.pop() {
            if entering {
                if state[&node] == State::InProgress {
                    anyhow::bail!("Serializability violation: cycle detected involving tx {node}");
                }
                if state[&node] == State::Done {
                    continue;
                }
                state.insert(node, State::InProgress);
                stack.push((node, false));
                // Push neighbors.
                for (from, to, _label) in edges.range((node, 0, "")..) {
                    if *from != node {
                        break;
                    }
                    stack.push((*to, true));
                }
            } else {
                state.insert(node, State::Done);
            }
        }
    }

    Ok(())
}

fn read_register(obj: &ConvexObject) -> anyhow::Result<Vec<WriteId>> {
    match obj.get("values") {
        Some(ConvexValue::Array(arr)) => {
            let mut result = vec![];
            for v in arr.iter() {
                if let ConvexValue::Int64(n) = v {
                    result.push(*n as WriteId);
                } else {
                    anyhow::bail!("Register contains non-integer: {v:?}");
                }
            }
            Ok(result)
        },
        None => Ok(vec![]), // empty register
        Some(other) => anyhow::bail!("Register values is not an array: {other:?}"),
    }
}

fn write_register(values: &[WriteId]) -> ConvexObject {
    let arr: Vec<ConvexValue> = values
        .iter()
        .map(|v| ConvexValue::Int64(*v as i64))
        .collect();
    assert_obj!("values" => ConvexValue::Array(arr.try_into().unwrap()))
}
