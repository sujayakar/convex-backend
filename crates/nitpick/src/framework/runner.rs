use std::{
    task::Poll,
    time::{
        Duration,
        Instant,
    },
};

use application::{
    test_helpers::ApplicationTestExt,
    Application,
};
use common::{
    event_recorder::{
        Event,
        EventRecorder,
        TraceEvent,
    },
    knobs::RUNTIME_STACK_SIZE,
    runtime::Runtime,
};
use errors::ErrorMetadataAnyhowExt;
use futures::{
    future::LocalBoxFuture,
    stream::FuturesUnordered,
    FutureExt,
    StreamExt,
};
use rand::Rng;
use runtime::testing::{
    TestDriver,
    TestRuntime,
};

use super::scenario::{
    Scenario,
    TestRun,
};

const VERIFY_PROBABILITY: f64 = 0.01;

const TEST_TIMEOUT: Duration = Duration::from_secs(120);
const OUTPUT_DEBUG_PREVIEW_CHARS: usize = 256;

/// Configuration for a single simulation run.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// How many transactions should the runner issue from the scenario?
    pub transactions: usize,

    /// How many transactions should run at once?
    pub concurrency: usize,

    /// Seed the test's randomness with the given seed.
    pub seed: u64,
}

/// Result of a single simulation run.
#[derive(Debug)]
pub struct TestResult<T> {
    /// How many times was the test runtime polled?
    ///
    /// This is useful diagnostic context on failures, but it is not a
    /// deterministic equality signal (see `PartialEq` below).
    pub num_polls: usize,

    /// What was the next u64 sampled from the runtime's RNG?
    pub rng_next_u64: u64,

    /// What was the output from the test run?
    pub output: T,

    /// Structured execution trace (if event recording was enabled).
    pub trace: Vec<TraceEvent>,
}

#[derive(Debug, Eq, PartialEq)]
struct DeterminismDiff {
    rng_mismatch: bool,
    output_mismatch: bool,
    trace_len_mismatch: bool,
    trace_len_mismatch_side: Option<&'static str>,
    trace_len_delta: i128,
    paired_trace_event_count: usize,
    trace_mismatch_kind: &'static str,
    trace_event_mismatch_index: Option<usize>,
    trace_event_mismatch_run1_kind: Option<&'static str>,
    trace_event_mismatch_run2_kind: Option<&'static str>,
}

impl DeterminismDiff {
    fn is_match(&self) -> bool {
        !self.rng_mismatch
            && !self.output_mismatch
            && !self.trace_len_mismatch
            && self.trace_event_mismatch_index.is_none()
    }
}

fn determinism_diff<T: PartialEq>(run1: &TestResult<T>, run2: &TestResult<T>) -> DeterminismDiff {
    let rng_mismatch = run1.rng_next_u64 != run2.rng_next_u64;
    let output_mismatch = run1.output != run2.output;
    let trace_len_mismatch = run1.trace.len() != run2.trace.len();
    let trace_len_delta = signed_delta_usize(run1.trace.len(), run2.trace.len());
    let paired_trace_event_count = run1.trace.len().min(run2.trace.len());
    let trace_len_mismatch_side = if run1.trace.len() > run2.trace.len() {
        Some("run1_longer")
    } else if run1.trace.len() < run2.trace.len() {
        Some("run2_longer")
    } else {
        None
    };
    let paired_trace_event_mismatch_index = run1
        .trace
        .iter()
        .zip(run2.trace.iter())
        .position(|(a, b)| a.event != b.event);
    let trace_mismatch_kind = if paired_trace_event_mismatch_index.is_some() {
        "event_payload_mismatch"
    } else if trace_len_mismatch {
        "length_boundary"
    } else {
        "none"
    };
    let trace_event_mismatch_index = paired_trace_event_mismatch_index
        .or_else(|| trace_len_mismatch.then_some(run1.trace.len().min(run2.trace.len())));
    let trace_event_mismatch_run1_kind = trace_event_mismatch_index
        .and_then(|i| run1.trace.get(i))
        .map(|e| event_kind(&e.event));
    let trace_event_mismatch_run2_kind = trace_event_mismatch_index
        .and_then(|i| run2.trace.get(i))
        .map(|e| event_kind(&e.event));

    DeterminismDiff {
        rng_mismatch,
        output_mismatch,
        trace_len_mismatch,
        trace_len_mismatch_side,
        trace_len_delta,
        paired_trace_event_count,
        trace_mismatch_kind,
        trace_event_mismatch_index,
        trace_event_mismatch_run1_kind,
        trace_event_mismatch_run2_kind,
    }
}

/// Compute a signed difference between two `usize` values.
///
/// Uses saturating semantics if the unsigned magnitude cannot fit in `i128`
/// (theoretical on common targets, but keeps behavior well-defined on wider
/// architectures).
fn signed_delta_usize(lhs: usize, rhs: usize) -> i128 {
    use std::cmp::Ordering;

    match lhs.cmp(&rhs) {
        Ordering::Equal => 0,
        Ordering::Greater => i128::try_from(lhs - rhs).unwrap_or(i128::MAX),
        Ordering::Less => i128::try_from(rhs - lhs).map_or(i128::MIN, |delta| -delta),
    }
}

impl<T: PartialEq> PartialEq for TestResult<T> {
    fn eq(&self, other: &Self) -> bool {
        // `num_polls` is excluded from determinism equality. Tokio's
        // `worker_poll_count` includes scheduler-internal/background task polls
        // and is only flushed at scheduler submit points, so it can differ under
        // host CPU contention even when logical execution is identical.
        //
        // We still compare trace *event payloads* (excluding seq/elapsed wall
        // clock fields) to avoid weakening determinism checks after removing
        // `num_polls` from equality.
        determinism_diff(self, other).is_match()
    }
}
impl<T: Eq> Eq for TestResult<T> {}

fn run_once<S: Scenario>(
    scenario: &S,
    td: &TestDriver,
    config: Config,
) -> anyhow::Result<TestResult<<S::TestRun as TestRun>::Output>> {
    let test_start = Instant::now();

    let recorder = EventRecorder::active();
    let rt = td.rt_with_event_recorder(recorder.clone());
    let application = td.run_until(async {
        let application = Application::new_for_tests(&rt).await?;
        tracing::info!(
            "[nitpick] Created application in {:?}",
            test_start.elapsed()
        );
        anyhow::Ok(application)
    })?;

    let test_run = td.run_until(async {
        let run = scenario.start_run(td.rt(), &application).await?;
        tracing::info!(
            "[nitpick] Initialized scenario {:?} in {:?}",
            scenario.name(),
            test_start.elapsed()
        );
        anyhow::Ok(run)
    })?;

    let num_polls = td.run_until(run_transactions(td.rt(), &application, &test_run, config))?;

    // Always run validation at the end.
    let start = Instant::now();
    td.run_until(test_run.validate(&application))?;
    tracing::info!(
        "[nitpick] Final verification passed in {:?}",
        start.elapsed()
    );

    let output = td.run_until(test_run.finalize(&application))?;

    let trace = recorder.drain();
    let result = TestResult {
        num_polls,
        rng_next_u64: td.rt().rng().random(),
        output,
        trace,
    };
    tracing::info!(
        "[nitpick] Scenario {:?} completed in {:?} ({} polls, {} trace events, output: {:?})",
        scenario.name(),
        test_start.elapsed(),
        result.num_polls,
        result.trace.len(),
        result.output,
    );

    Ok(result)
}

async fn run_transactions<TR: TestRun>(
    rt: TestRuntime,
    application: &Application<TestRuntime>,
    test_run: &TR,
    config: Config,
) -> anyhow::Result<usize> {
    let test_start = Instant::now();
    let mut transactions = FuturesUnordered::<LocalBoxFuture<anyhow::Result<()>>>::new();
    let mut next_tx_id: usize = 0;
    let mut num_completed: usize = 0;
    // Allow extra transaction attempts to account for OCC retries.
    let max_tx_attempts = config.transactions * 10;

    loop {
        if test_start.elapsed() > TEST_TIMEOUT {
            anyhow::bail!(
                "Test timed out after {:?} ({num_completed} completed, {} in flight)",
                TEST_TIMEOUT,
                transactions.len()
            );
        }

        // Drain completed transactions.
        let mut results = vec![];
        if !transactions.is_empty() && transactions.len() >= config.concurrency {
            let r = transactions.select_next_some().await;
            results.push(r);
        }
        while !transactions.is_empty() {
            if let Poll::Ready(r) = futures::poll!(transactions.select_next_some()) {
                results.push(r);
            } else {
                break;
            }
        }
        for r in results {
            if let Err(e) = r {
                if e.is_occ() {
                    tracing::debug!("[nitpick] Transaction OCC'd");
                    continue;
                }
                return Err(e);
            }
            num_completed += 1;
        }
        if num_completed >= config.transactions {
            break;
        }

        // Refill transactions.
        while transactions.len() < config.concurrency && next_tx_id < max_tx_attempts {
            let tx_id = next_tx_id;
            next_tx_id += 1;
            let future = test_run.run_transaction(rt.clone(), application);
            let future = async move {
                tracing::debug!("[nitpick] tx {tx_id} starting");
                let r = future.await;
                match &r {
                    Ok(()) => tracing::debug!("[nitpick] tx {tx_id} ok"),
                    Err(e) if e.is_occ() => {
                        tracing::debug!("[nitpick] tx {tx_id} OCC")
                    },
                    Err(e) => tracing::warn!("[nitpick] tx {tx_id} failed: {e:?}"),
                }
                r
            }
            .boxed_local();
            transactions.push(future);
        }

        // Probabilistically validate.
        if rt.rng().random_bool(VERIFY_PROBABILITY) {
            test_run.validate(application).await?;
        }
    }

    // Diagnostic only: Tokio's poll-count metric can drift under host CPU
    // contention even for logically deterministic runs, so it is not used for
    // determinism equality.
    let num_polls = tokio::runtime::Handle::current()
        .metrics()
        .worker_poll_count(0);
    Ok(num_polls as usize)
}

/// How likely we are to run a determinism check (re-run with same seed).
const DETERMINISM_CHECK_PROBABILITY: f64 = 0.1;

/// Run a scenario with the given config on a dedicated thread with a large
/// stack. Probabilistically checks determinism by re-running with the same seed.
pub fn run_scenario<S: Scenario>(scenario: S, config: Config) -> anyhow::Result<()> {
    let thread_handle = std::thread::Builder::new()
        .stack_size(*RUNTIME_STACK_SIZE)
        .spawn(move || {
            let (run1, should_check) = {
                let td = TestDriver::new_with_seed(config.seed);
                let run1 = run_once(&scenario, &td, config)?;
                let should_check = td.rt().rng().random_bool(DETERMINISM_CHECK_PROBABILITY);
                (run1, should_check)
            };

            if should_check {
                tracing::info!(
                    "[nitpick] Running determinism check for seed {}",
                    config.seed
                );
                check_determinism(&scenario, config, &run1)?;
            }

            anyhow::Ok(())
        })?;
    thread_handle.join().expect("nitpick thread panicked")?;
    Ok(())
}

/// Run a scenario and always verify determinism (run twice with the same seed).
pub fn run_scenario_deterministic<S: Scenario>(scenario: S, config: Config) -> anyhow::Result<()> {
    let thread_handle = std::thread::Builder::new()
        .stack_size(*RUNTIME_STACK_SIZE)
        .spawn(move || {
            let run1 = {
                let td = TestDriver::new_with_seed(config.seed);
                run_once(&scenario, &td, config)?
            };
            check_determinism(&scenario, config, &run1)?;
            anyhow::Ok(())
        })?;
    thread_handle.join().expect("nitpick thread panicked")?;
    Ok(())
}

fn check_determinism<S: Scenario>(
    scenario: &S,
    config: Config,
    run1: &TestResult<<S::TestRun as TestRun>::Output>,
) -> anyhow::Result<()> {
    tracing::info!(
        "[nitpick] Running determinism check for seed {}",
        config.seed
    );
    let td = TestDriver::new_with_seed(config.seed);
    let run2 = run_once(scenario, &td, config)?;
    let diff = determinism_diff(run1, &run2);
    if !diff.is_match() {
        anyhow::bail!(
            "{}",
            determinism_failure_message(config.seed, run1, &run2, &diff)
        );
    }
    if run1.num_polls != run2.num_polls {
        tracing::debug!(
            "[nitpick] Determinism diagnostics for seed {}: num_polls differed (run1: {}, run2: {}), but deterministic fields matched",
            config.seed,
            run1.num_polls,
            run2.num_polls
        );
    }
    tracing::info!("[nitpick] Determinism check passed");
    Ok(())
}

fn determinism_failure_message<T: std::fmt::Debug>(
    seed: u64,
    run1: &TestResult<T>,
    run2: &TestResult<T>,
    diff: &DeterminismDiff,
) -> String {
    let run1_output_debug = debug_string_preview(&run1.output, OUTPUT_DEBUG_PREVIEW_CHARS);
    let run2_output_debug = debug_string_preview(&run2.output, OUTPUT_DEBUG_PREVIEW_CHARS);
    let num_polls_delta = signed_delta_usize(run1.num_polls, run2.num_polls);
    format!(
        "Determinism failure for seed {}:\n  run1: {{ rng_next_u64: {}, output_debug_preview: {} }}\n  run2: {{ rng_next_u64: {}, output_debug_preview: {} }}\n  diagnostics: {{ rng_mismatch: {}, output_mismatch: {}, run1_output_debug_len: {}, run2_output_debug_len: {}, run1_output_debug_truncated: {}, run2_output_debug_truncated: {}, output_debug_preview_chars_limit: {}, run1_num_polls: {}, run2_num_polls: {}, num_polls_delta: {}, run1_trace_len: {}, run2_trace_len: {}, trace_len_mismatch: {}, trace_len_mismatch_side: {:?}, trace_len_delta: {}, paired_trace_event_count: {}, trace_mismatch_kind: {:?}, trace_event_mismatch_index: {:?}, trace_event_mismatch_run1_kind: {:?}, trace_event_mismatch_run2_kind: {:?} }}",
        seed,
        run1.rng_next_u64,
        run1_output_debug.preview,
        run2.rng_next_u64,
        run2_output_debug.preview,
        diff.rng_mismatch,
        diff.output_mismatch,
        run1_output_debug.full_len,
        run2_output_debug.full_len,
        run1_output_debug.truncated,
        run2_output_debug.truncated,
        OUTPUT_DEBUG_PREVIEW_CHARS,
        run1.num_polls,
        run2.num_polls,
        num_polls_delta,
        run1.trace.len(),
        run2.trace.len(),
        diff.trace_len_mismatch,
        diff.trace_len_mismatch_side,
        diff.trace_len_delta,
        diff.paired_trace_event_count,
        diff.trace_mismatch_kind,
        diff.trace_event_mismatch_index,
        diff.trace_event_mismatch_run1_kind,
        diff.trace_event_mismatch_run2_kind,
    )
}

struct DebugStringPreview {
    full_len: usize,
    preview: String,
    truncated: bool,
}

fn debug_string_preview<T: std::fmt::Debug>(value: &T, max_chars: usize) -> DebugStringPreview {
    let debug_string = format!("{value:?}");
    let debug_char_count = debug_string.chars().count();
    if debug_char_count <= max_chars {
        return DebugStringPreview {
            full_len: debug_char_count,
            preview: debug_string,
            truncated: false,
        };
    }
    let preview = debug_string.chars().take(max_chars).collect::<String>();
    DebugStringPreview {
        full_len: debug_char_count,
        preview: format!("{preview}…<truncated>"),
        truncated: true,
    }
}

fn event_kind(event: &Event) -> &'static str {
    match event {
        Event::TransactionBegin { .. } => "TransactionBegin",
        Event::TransactionCommit => "TransactionCommit",
        Event::TransactionConflict => "TransactionConflict",
        Event::UdfStart { .. } => "UdfStart",
        Event::UdfEnd { .. } => "UdfEnd",
        Event::PausePointHit { .. } => "PausePointHit",
        Event::Custom { .. } => "Custom",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use common::event_recorder::{
        Event,
        TraceEvent,
    };

    use super::{
        debug_string_preview,
        determinism_diff,
        determinism_failure_message,
        signed_delta_usize,
        DeterminismDiff,
        TestResult,
    };

    #[test]
    fn test_result_equality_ignores_num_polls_and_trace() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };
        let run2 = TestResult {
            num_polls: 103,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };

        assert_eq!(run1, run2);
    }

    #[test]
    fn test_result_equality_ignores_trace_contents() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionBegin {
                    identity: "id1".to_string(),
                },
            }],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 1,
                elapsed: Duration::from_secs(2),
                event: Event::TransactionBegin {
                    identity: "id1".to_string(),
                },
            }],
        };

        assert_eq!(run1, run2);
    }

    #[test]
    fn test_result_equality_detects_trace_event_difference() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionBegin {
                    identity: "id1".to_string(),
                },
            }],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 1,
                elapsed: Duration::from_secs(2),
                event: Event::TransactionCommit,
            }],
        };

        assert_ne!(run1, run2);
    }

    #[test]
    fn test_result_equality_detects_trace_length_difference() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionCommit,
            }],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };

        assert_ne!(run1, run2);
    }

    #[test]
    fn test_determinism_diff_reports_trace_length_only_mismatch() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionCommit,
            }],
        };
        let run2 = TestResult {
            num_polls: 101,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };

        assert_eq!(
            determinism_diff(&run1, &run2),
            DeterminismDiff {
                rng_mismatch: false,
                output_mismatch: false,
                trace_len_mismatch: true,
                trace_len_mismatch_side: Some("run1_longer"),
                trace_len_delta: 1,
                paired_trace_event_count: 0,
                trace_mismatch_kind: "length_boundary",
                trace_event_mismatch_index: Some(0),
                trace_event_mismatch_run1_kind: Some("TransactionCommit"),
                trace_event_mismatch_run2_kind: None,
            }
        );
    }

    #[test]
    fn test_determinism_diff_reports_trace_length_only_mismatch_other_direction() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };
        let run2 = TestResult {
            num_polls: 101,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionCommit,
            }],
        };

        assert_eq!(
            determinism_diff(&run1, &run2),
            DeterminismDiff {
                rng_mismatch: false,
                output_mismatch: false,
                trace_len_mismatch: true,
                trace_len_mismatch_side: Some("run2_longer"),
                trace_len_delta: -1,
                paired_trace_event_count: 0,
                trace_mismatch_kind: "length_boundary",
                trace_event_mismatch_index: Some(0),
                trace_event_mismatch_run1_kind: None,
                trace_event_mismatch_run2_kind: Some("TransactionCommit"),
            }
        );
    }

    #[test]
    fn test_determinism_diff_prefers_event_mismatch_over_length_boundary() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![
                TraceEvent {
                    seq: 0,
                    elapsed: Duration::from_secs(1),
                    event: Event::TransactionBegin {
                        identity: "id1".to_string(),
                    },
                },
                TraceEvent {
                    seq: 1,
                    elapsed: Duration::from_secs(2),
                    event: Event::TransactionCommit,
                },
            ],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 9,
                elapsed: Duration::from_secs(9),
                event: Event::TransactionConflict,
            }],
        };

        assert_eq!(
            determinism_diff(&run1, &run2),
            DeterminismDiff {
                rng_mismatch: false,
                output_mismatch: false,
                trace_len_mismatch: true,
                trace_len_mismatch_side: Some("run1_longer"),
                trace_len_delta: 1,
                paired_trace_event_count: 1,
                trace_mismatch_kind: "event_payload_mismatch",
                trace_event_mismatch_index: Some(0),
                trace_event_mismatch_run1_kind: Some("TransactionBegin"),
                trace_event_mismatch_run2_kind: Some("TransactionConflict"),
            }
        );
    }

    #[test]
    fn test_determinism_diff_reports_boundary_after_matching_prefix() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![
                TraceEvent {
                    seq: 0,
                    elapsed: Duration::from_secs(1),
                    event: Event::TransactionCommit,
                },
                TraceEvent {
                    seq: 1,
                    elapsed: Duration::from_secs(2),
                    event: Event::UdfStart {
                        udf_path: "x:y".to_string(),
                        udf_type: "query".to_string(),
                    },
                },
            ],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 9,
                elapsed: Duration::from_secs(9),
                event: Event::TransactionCommit,
            }],
        };

        assert_eq!(
            determinism_diff(&run1, &run2),
            DeterminismDiff {
                rng_mismatch: false,
                output_mismatch: false,
                trace_len_mismatch: true,
                trace_len_mismatch_side: Some("run1_longer"),
                trace_len_delta: 1,
                paired_trace_event_count: 1,
                trace_mismatch_kind: "length_boundary",
                trace_event_mismatch_index: Some(1),
                trace_event_mismatch_run1_kind: Some("UdfStart"),
                trace_event_mismatch_run2_kind: None,
            }
        );
    }

    #[test]
    fn test_determinism_diff_reports_multi_event_length_delta() {
        let long_run = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![
                TraceEvent {
                    seq: 0,
                    elapsed: Duration::from_secs(1),
                    event: Event::TransactionCommit,
                },
                TraceEvent {
                    seq: 1,
                    elapsed: Duration::from_secs(2),
                    event: Event::UdfStart {
                        udf_path: "x:y".to_string(),
                        udf_type: "query".to_string(),
                    },
                },
                TraceEvent {
                    seq: 2,
                    elapsed: Duration::from_secs(3),
                    event: Event::UdfEnd {
                        udf_path: "x:y".to_string(),
                        success: true,
                    },
                },
            ],
        };
        let short_run = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 9,
                elapsed: Duration::from_secs(9),
                event: Event::TransactionCommit,
            }],
        };

        assert_eq!(
            determinism_diff(&long_run, &short_run),
            DeterminismDiff {
                rng_mismatch: false,
                output_mismatch: false,
                trace_len_mismatch: true,
                trace_len_mismatch_side: Some("run1_longer"),
                trace_len_delta: 2,
                paired_trace_event_count: 1,
                trace_mismatch_kind: "length_boundary",
                trace_event_mismatch_index: Some(1),
                trace_event_mismatch_run1_kind: Some("UdfStart"),
                trace_event_mismatch_run2_kind: None,
            }
        );
        assert_eq!(
            determinism_diff(&short_run, &long_run),
            DeterminismDiff {
                rng_mismatch: false,
                output_mismatch: false,
                trace_len_mismatch: true,
                trace_len_mismatch_side: Some("run2_longer"),
                trace_len_delta: -2,
                paired_trace_event_count: 1,
                trace_mismatch_kind: "length_boundary",
                trace_event_mismatch_index: Some(1),
                trace_event_mismatch_run1_kind: None,
                trace_event_mismatch_run2_kind: Some("UdfStart"),
            }
        );
    }

    #[test]
    fn test_determinism_diff_is_match_when_only_num_polls_differs() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionCommit,
            }],
        };
        let run2 = TestResult {
            num_polls: 101,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 999,
                elapsed: Duration::from_secs(99),
                event: Event::TransactionCommit,
            }],
        };

        assert!(determinism_diff(&run1, &run2).is_match());
    }

    #[test]
    fn test_result_equality_detects_rng_difference() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 43,
            output: "ok".to_string(),
            trace: vec![],
        };

        assert_ne!(run1, run2);
    }

    #[test]
    fn test_result_equality_detects_output_difference() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "not_ok".to_string(),
            trace: vec![],
        };

        assert_ne!(run1, run2);
    }

    #[test]
    fn test_signed_delta_usize_handles_extreme_values() {
        assert_eq!(signed_delta_usize(usize::MAX, 0), usize::MAX as i128);
        assert_eq!(signed_delta_usize(0, usize::MAX), -(usize::MAX as i128));
        assert_eq!(signed_delta_usize(usize::MAX, usize::MAX), 0);
    }

    #[test]
    fn test_signed_delta_usize_sign_and_symmetry() {
        assert_eq!(signed_delta_usize(7, 3), 4);
        assert_eq!(signed_delta_usize(3, 7), -4);
        assert_eq!(signed_delta_usize(7, 3), -signed_delta_usize(3, 7));
    }

    #[test]
    fn test_debug_string_preview_not_truncated_at_exact_limit() {
        let preview = debug_string_preview(&"🙂🙂🙂".to_string(), 5);
        assert_eq!(preview.full_len, 5);
        assert_eq!(preview.preview, "\"🙂🙂🙂\"");
        assert!(!preview.truncated);
    }

    #[test]
    fn test_debug_string_preview_truncates_when_above_limit() {
        let preview = debug_string_preview(&"abcdef".to_string(), 4);
        assert_eq!(preview.full_len, 8);
        assert_eq!(preview.preview, "\"abc…<truncated>");
        assert!(preview.truncated);
    }

    #[test]
    fn test_determinism_failure_message_is_compact_and_diagnostic() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionCommit,
            }],
        };
        let run2 = TestResult {
            num_polls: 103,
            rng_next_u64: 43,
            output: "not_ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(2),
                event: Event::TransactionCommit,
            }],
        };

        let diff = determinism_diff(&run1, &run2);
        let message = determinism_failure_message(123, &run1, &run2, &diff);

        assert!(message.contains("Determinism failure for seed 123"));
        assert!(message.contains("rng_mismatch: true"));
        assert!(message.contains("output_mismatch: true"));
        assert!(message.contains("run1_output_debug_len: 4"));
        assert!(message.contains("run2_output_debug_len: 8"));
        assert!(message.contains("run1_output_debug_truncated: false"));
        assert!(message.contains("run2_output_debug_truncated: false"));
        assert!(message.contains("output_debug_preview_chars_limit: 256"));
        assert!(message.contains("run1_num_polls: 100"));
        assert!(message.contains("run2_num_polls: 103"));
        assert!(message.contains("num_polls_delta: -3"));
        assert!(message.contains("run1_trace_len: 1"));
        assert!(message.contains("run2_trace_len: 1"));
        assert!(message.contains("trace_len_mismatch: false"));
        assert!(message.contains("trace_len_mismatch_side: None"));
        assert!(message.contains("trace_len_delta: 0"));
        assert!(message.contains("paired_trace_event_count: 1"));
        assert!(message.contains("trace_mismatch_kind: \"none\""));
        assert!(message.contains("trace_event_mismatch_index: None"));
        assert!(message.contains("trace_event_mismatch_run1_kind: None"));
        assert!(message.contains("trace_event_mismatch_run2_kind: None"));
        assert!(
            !message.contains("<truncated>"),
            "short outputs should not be truncated"
        );
        assert!(
            !message.contains("TraceEvent"),
            "message should avoid dumping full traces"
        );
    }

    #[test]
    fn test_determinism_failure_message_reports_trace_mismatch_index() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionBegin {
                    identity: "id1".to_string(),
                },
            }],
        };
        let run2 = TestResult {
            num_polls: 103,
            rng_next_u64: 43,
            output: "not_ok".to_string(),
            trace: vec![TraceEvent {
                seq: 1,
                elapsed: Duration::from_secs(2),
                event: Event::TransactionCommit,
            }],
        };

        let diff = determinism_diff(&run1, &run2);
        let message = determinism_failure_message(123, &run1, &run2, &diff);
        assert!(message.contains("trace_len_mismatch: false"));
        assert!(message.contains("trace_len_mismatch_side: None"));
        assert!(message.contains("num_polls_delta: -3"));
        assert!(message.contains("trace_len_delta: 0"));
        assert!(message.contains("paired_trace_event_count: 1"));
        assert!(message.contains("trace_mismatch_kind: \"event_payload_mismatch\""));
        assert!(message.contains("trace_event_mismatch_index: Some(0)"));
        assert!(message.contains("trace_event_mismatch_run1_kind: Some(\"TransactionBegin\")"));
        assert!(message.contains("trace_event_mismatch_run2_kind: Some(\"TransactionCommit\")"));
    }

    #[test]
    fn test_determinism_failure_message_reports_nonzero_trace_mismatch_index() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![
                TraceEvent {
                    seq: 0,
                    elapsed: Duration::from_secs(1),
                    event: Event::TransactionCommit,
                },
                TraceEvent {
                    seq: 1,
                    elapsed: Duration::from_secs(2),
                    event: Event::TransactionBegin {
                        identity: "id1".to_string(),
                    },
                },
            ],
        };
        let run2 = TestResult {
            num_polls: 103,
            rng_next_u64: 43,
            output: "not_ok".to_string(),
            trace: vec![
                TraceEvent {
                    seq: 2,
                    elapsed: Duration::from_secs(3),
                    event: Event::TransactionCommit,
                },
                TraceEvent {
                    seq: 3,
                    elapsed: Duration::from_secs(4),
                    event: Event::TransactionConflict,
                },
            ],
        };

        let diff = determinism_diff(&run1, &run2);
        let message = determinism_failure_message(123, &run1, &run2, &diff);
        assert!(message.contains("trace_len_mismatch: false"));
        assert!(message.contains("trace_len_delta: 0"));
        assert!(message.contains("paired_trace_event_count: 2"));
        assert!(message.contains("trace_mismatch_kind: \"event_payload_mismatch\""));
        assert!(message.contains("trace_event_mismatch_index: Some(1)"));
        assert!(message.contains("trace_event_mismatch_run1_kind: Some(\"TransactionBegin\")"));
        assert!(message.contains("trace_event_mismatch_run2_kind: Some(\"TransactionConflict\")"));
    }

    #[test]
    fn test_determinism_failure_message_reports_trace_length_mismatch() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionCommit,
            }],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };

        let diff = determinism_diff(&run1, &run2);
        let message = determinism_failure_message(123, &run1, &run2, &diff);
        assert!(message.contains("trace_len_mismatch: true"));
        assert!(message.contains("trace_len_mismatch_side: Some(\"run1_longer\")"));
        assert!(message.contains("num_polls_delta: 0"));
        assert!(message.contains("trace_len_delta: 1"));
        assert!(message.contains("paired_trace_event_count: 0"));
        assert!(message.contains("trace_mismatch_kind: \"length_boundary\""));
        assert!(message.contains("trace_event_mismatch_index: Some(0)"));
        assert!(message.contains("trace_event_mismatch_run1_kind: Some(\"TransactionCommit\")"));
        assert!(message.contains("trace_event_mismatch_run2_kind: None"));
    }

    #[test]
    fn test_determinism_failure_message_reports_trace_length_mismatch_other_direction() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionCommit,
            }],
        };

        let diff = determinism_diff(&run1, &run2);
        let message = determinism_failure_message(123, &run1, &run2, &diff);
        assert!(message.contains("trace_len_mismatch: true"));
        assert!(message.contains("trace_len_mismatch_side: Some(\"run2_longer\")"));
        assert!(message.contains("num_polls_delta: 0"));
        assert!(message.contains("trace_len_delta: -1"));
        assert!(message.contains("paired_trace_event_count: 0"));
        assert!(message.contains("trace_mismatch_kind: \"length_boundary\""));
        assert!(message.contains("trace_event_mismatch_index: Some(0)"));
        assert!(message.contains("trace_event_mismatch_run1_kind: None"));
        assert!(message.contains("trace_event_mismatch_run2_kind: Some(\"TransactionCommit\")"));
    }

    #[test]
    fn test_determinism_failure_message_reports_multi_event_trace_length_delta() {
        let long_run = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![
                TraceEvent {
                    seq: 0,
                    elapsed: Duration::from_secs(1),
                    event: Event::TransactionCommit,
                },
                TraceEvent {
                    seq: 1,
                    elapsed: Duration::from_secs(2),
                    event: Event::UdfStart {
                        udf_path: "x:y".to_string(),
                        udf_type: "query".to_string(),
                    },
                },
                TraceEvent {
                    seq: 2,
                    elapsed: Duration::from_secs(3),
                    event: Event::UdfEnd {
                        udf_path: "x:y".to_string(),
                        success: true,
                    },
                },
            ],
        };
        let short_run = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 9,
                elapsed: Duration::from_secs(9),
                event: Event::TransactionCommit,
            }],
        };

        let diff_pos = determinism_diff(&long_run, &short_run);
        let message_pos = determinism_failure_message(123, &long_run, &short_run, &diff_pos);
        assert!(message_pos.contains("trace_len_mismatch_side: Some(\"run1_longer\")"));
        assert!(message_pos.contains("trace_len_delta: 2"));
        assert!(message_pos.contains("paired_trace_event_count: 1"));
        assert!(message_pos.contains("trace_event_mismatch_index: Some(1)"));
        assert!(message_pos.contains("trace_event_mismatch_run1_kind: Some(\"UdfStart\")"));
        assert!(message_pos.contains("trace_event_mismatch_run2_kind: None"));

        let diff_neg = determinism_diff(&short_run, &long_run);
        let message_neg = determinism_failure_message(123, &short_run, &long_run, &diff_neg);
        assert!(message_neg.contains("trace_len_mismatch_side: Some(\"run2_longer\")"));
        assert!(message_neg.contains("trace_len_delta: -2"));
        assert!(message_neg.contains("paired_trace_event_count: 1"));
        assert!(message_neg.contains("trace_event_mismatch_index: Some(1)"));
        assert!(message_neg.contains("trace_event_mismatch_run1_kind: None"));
        assert!(message_neg.contains("trace_event_mismatch_run2_kind: Some(\"UdfStart\")"));
    }

    #[test]
    fn test_determinism_failure_message_truncates_large_output_previews() {
        let run1 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "a".repeat(600),
            trace: vec![],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 43,
            output: "b".repeat(600),
            trace: vec![],
        };
        let diff = determinism_diff(&run1, &run2);
        let message = determinism_failure_message(123, &run1, &run2, &diff);

        assert!(message.contains("run1_output_debug_len: 602"));
        assert!(message.contains("run2_output_debug_len: 602"));
        assert!(message.contains("run1_output_debug_truncated: true"));
        assert!(message.contains("run2_output_debug_truncated: true"));
        assert!(message.contains("output_debug_preview_chars_limit: 256"));
        assert!(message.contains("…<truncated>"));
    }

    #[test]
    fn test_determinism_failure_message_reports_positive_num_polls_delta() {
        let run1 = TestResult {
            num_polls: 105,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };
        let run2 = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "not_ok".to_string(),
            trace: vec![],
        };
        let diff = determinism_diff(&run1, &run2);
        let message = determinism_failure_message(123, &run1, &run2, &diff);

        assert!(message.contains("num_polls_delta: 5"));
    }

    #[test]
    fn test_determinism_failure_message_reports_extreme_num_polls_deltas() {
        let run_max = TestResult {
            num_polls: usize::MAX,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![],
        };
        let run_zero = TestResult {
            num_polls: 0,
            rng_next_u64: 42,
            output: "not_ok".to_string(),
            trace: vec![],
        };

        let diff_pos = determinism_diff(&run_max, &run_zero);
        let message_pos = determinism_failure_message(123, &run_max, &run_zero, &diff_pos);
        assert!(message_pos.contains(&format!(
            "num_polls_delta: {}",
            usize::MAX as i128
        )));

        let diff_neg = determinism_diff(&run_zero, &run_max);
        let message_neg = determinism_failure_message(123, &run_zero, &run_max, &diff_neg);
        assert!(message_neg.contains(&format!(
            "num_polls_delta: {}",
            -(usize::MAX as i128)
        )));
    }

    #[test]
    fn test_partial_eq_consistent_with_determinism_diff() {
        let base = TestResult {
            num_polls: 100,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 0,
                elapsed: Duration::from_secs(1),
                event: Event::TransactionCommit,
            }],
        };

        let poll_only_diff = TestResult {
            num_polls: 101,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 99,
                elapsed: Duration::from_secs(9),
                event: Event::TransactionCommit,
            }],
        };
        assert_eq!(base == poll_only_diff, determinism_diff(&base, &poll_only_diff).is_match());

        let output_diff = TestResult {
            num_polls: 101,
            rng_next_u64: 42,
            output: "not_ok".to_string(),
            trace: vec![TraceEvent {
                seq: 99,
                elapsed: Duration::from_secs(9),
                event: Event::TransactionCommit,
            }],
        };
        assert_eq!(base == output_diff, determinism_diff(&base, &output_diff).is_match());

        let trace_event_diff = TestResult {
            num_polls: 101,
            rng_next_u64: 42,
            output: "ok".to_string(),
            trace: vec![TraceEvent {
                seq: 99,
                elapsed: Duration::from_secs(9),
                event: Event::TransactionConflict,
            }],
        };
        assert_eq!(
            base == trace_event_diff,
            determinism_diff(&base, &trace_event_diff).is_match()
        );
    }
}
