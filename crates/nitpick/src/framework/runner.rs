use std::time::{
    Duration,
    Instant,
};

use application::{
    test_helpers::ApplicationTestExt,
    Application,
};
use common::{
    event_recorder::{
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
// #region agent log
use runtime::testing::{dst_debug_log, DST_RUN_ID, DST_SEED, DST_TF_ID};
use std::sync::atomic::Ordering;
// #endregion

use super::scenario::{
    Scenario,
    TestRun,
};

const VERIFY_PROBABILITY: f64 = 0.01;

const TEST_TIMEOUT: Duration = Duration::from_secs(120);

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

/// Result of a single simulation run, used for determinism comparison.
#[derive(Debug)]
pub struct TestResult<T> {
    /// How many times was the test runtime polled?
    pub num_polls: usize,

    /// What was the next u64 sampled from the runtime's RNG?
    pub rng_next_u64: u64,

    /// What was the output from the test run?
    pub output: T,

    /// Structured execution trace (if event recording was enabled).
    pub trace: Vec<TraceEvent>,
}

impl<T: Eq> PartialEq for TestResult<T> {
    fn eq(&self, other: &Self) -> bool {
        // `num_polls` is intentionally excluded from equality. Tokio's
        // worker poll metric is sensitive to scheduler implementation details
        // and can drift in batch mode under unrelated concurrent load.
        self.rng_next_u64 == other.rng_next_u64 && self.output == other.output
    }
}
impl<T: Eq> Eq for TestResult<T> {}

fn run_once<S: Scenario>(
    scenario: &S,
    td: &TestDriver,
    config: Config,
) -> anyhow::Result<TestResult<<S::TestRun as TestRun>::Output>> {
    let test_start = Instant::now();
    // #region agent log
    dst_debug_log(
        "H5",
        "crates/nitpick/src/framework/runner.rs:run_once",
        "run_once_enter",
        serde_json::json!({
            "scenario": scenario.name(),
            "transactions": config.transactions,
            "concurrency": config.concurrency,
        }),
    );
    // #endregion

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
    // #region agent log
    dst_debug_log(
        "H5",
        "crates/nitpick/src/framework/runner.rs:run_once",
        "run_once_exit",
        serde_json::json!({
            "num_polls": result.num_polls,
            "rng_next_u64": result.rng_next_u64,
            "trace_len": result.trace.len(),
            "output": format!("{:?}", result.output),
        }),
    );
    // #endregion
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

    let num_polls = tokio::runtime::Handle::current()
        .metrics()
        .worker_poll_count(0);
    Ok(num_polls as usize)
}

/// How likely we are to run a determinism check (re-run with same seed).
// #region agent log
const DETERMINISM_CHECK_PROBABILITY: f64 = 0.1; // was 0.1
// #endregion

fn determinism_check_probability() -> f64 {
    if std::env::var("NITPICK_FORCE_DETERMINISM_CHECK")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        1.0
    } else {
        DETERMINISM_CHECK_PROBABILITY
    }
}

/// Run a scenario with the given config on a dedicated thread with a large
/// stack. Probabilistically checks determinism by re-running with the same seed.
pub fn run_scenario<S: Scenario>(scenario: S, config: Config) -> anyhow::Result<()> {
    let thread_handle = std::thread::Builder::new()
        .stack_size(*RUNTIME_STACK_SIZE)
        .spawn(move || {
            // #region agent log
            DST_RUN_ID.store(1, Ordering::Relaxed);
            DST_TF_ID.store(0, Ordering::Relaxed);
            DST_SEED.store(config.seed, Ordering::Relaxed);
            // #endregion
            let (run1, should_check) = {
                let td = TestDriver::new_with_seed(config.seed);
                let run1 = run_once(&scenario, &td, config)?;
                let should_check = td
                    .rt()
                    .rng()
                    .random_bool(determinism_check_probability());
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
            // #region agent log
            DST_RUN_ID.store(1, Ordering::Relaxed);
            DST_TF_ID.store(0, Ordering::Relaxed);
            DST_SEED.store(config.seed, Ordering::Relaxed);
            // #endregion
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
    // #region agent log
    DST_RUN_ID.store(2, Ordering::Relaxed);
    DST_TF_ID.store(0, Ordering::Relaxed);
    DST_SEED.store(config.seed, Ordering::Relaxed);
    // #endregion
    // #region agent log
    dst_debug_log(
        "H5",
        "crates/nitpick/src/framework/runner.rs:check_determinism",
        "run2_driver_create_start",
        serde_json::json!({}),
    );
    // #endregion
    let td = TestDriver::new_with_seed(config.seed);
    // #region agent log
    dst_debug_log(
        "H5",
        "crates/nitpick/src/framework/runner.rs:check_determinism",
        "run2_driver_create_done",
        serde_json::json!({}),
    );
    // #endregion
    let run2 = run_once(scenario, &td, config)?;
    let mut first_trace_divergence: Option<(usize, String, String)> = None;
    for (idx, (left, right)) in run1.trace.iter().zip(run2.trace.iter()).enumerate() {
        let left_event = format!("{:?}", left.event);
        let right_event = format!("{:?}", right.event);
        if left_event != right_event {
            first_trace_divergence = Some((idx, left_event, right_event));
            break;
        }
    }
    let semantic_mismatch =
        run1.rng_next_u64 != run2.rng_next_u64
            || run1.output != run2.output
            || run1.trace.len() != run2.trace.len()
            || first_trace_divergence.is_some();
    if semantic_mismatch {
        // #region agent log
        dst_debug_log(
            "H5",
            "crates/nitpick/src/framework/runner.rs:check_determinism",
            "determinism_mismatch",
            serde_json::json!({
                "run1_num_polls": run1.num_polls,
                "run2_num_polls": run2.num_polls,
                "run1_rng_next_u64": run1.rng_next_u64,
                "run2_rng_next_u64": run2.rng_next_u64,
                "run1_output": format!("{:?}", run1.output),
                "run2_output": format!("{:?}", run2.output),
                "run1_trace_len": run1.trace.len(),
                "run2_trace_len": run2.trace.len(),
                "first_trace_divergence": first_trace_divergence,
            }),
        );
        // #endregion
        anyhow::bail!(
            "Determinism failure for seed {}:\n  run1: {:?}\n  run2: {:?}",
            config.seed,
            run1,
            run2
        );
    }
    if run1.num_polls != run2.num_polls {
        tracing::warn!(
            "[nitpick] Poll-count drift for seed {} (run1={}, run2={}) with identical output/rng/trace",
            config.seed,
            run1.num_polls,
            run2.num_polls
        );
        // #region agent log
        dst_debug_log(
            "H5",
            "crates/nitpick/src/framework/runner.rs:check_determinism",
            "poll_count_drift_only",
            serde_json::json!({
                "run1_num_polls": run1.num_polls,
                "run2_num_polls": run2.num_polls,
                "run1_rng_next_u64": run1.rng_next_u64,
                "run2_rng_next_u64": run2.rng_next_u64,
                "run1_output": format!("{:?}", run1.output),
                "run2_output": format!("{:?}", run2.output),
                "run1_trace_len": run1.trace.len(),
                "run2_trace_len": run2.trace.len(),
                "first_trace_divergence": first_trace_divergence,
            }),
        );
        // #endregion
    }
    tracing::info!("[nitpick] Determinism check passed");
    Ok(())
}
