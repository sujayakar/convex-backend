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
use runtime::testing::{
    dst_log,
    DST_RUN_ID,
    DST_THREAD_FUTURE_ID,
};
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

/// Maximum allowed jitter in `num_polls` between deterministic replays.
///
/// `worker_poll_count` includes every spawned-task poll on the Tokio
/// current-thread runtime.  V8 isolate workers complete UDFs on OS
/// threads and notify the scheduler via `tokio::sync::oneshot::send()`.
/// This fires the scheduler-task's waker from outside the runtime thread,
/// which Tokio routes to its cross-thread *injection queue*.  The
/// current-thread scheduler only pops that queue when the local run-queue
/// is empty or on a periodic global tick (every ~31 polls).
///
/// Under CPU contention the exact tick at which the entry is consumed
/// varies.  If it is consumed *during* the transaction loop the scheduler
/// can immediately assign the freed worker to the next pending request,
/// generating downstream scheduling polls.  If it is consumed *after* the
/// loop (no pending requests) those polls never happen.  This produces a
/// small, bounded jitter in `worker_poll_count` — typically ±1 — while
/// the logical execution (RNG state, output) remains identical.
///
/// A tolerance of 2 accommodates ±1 from a single deferred completion
/// plus a second ±1 if two workers race on the same tick boundary.
const NUM_POLLS_TOLERANCE: usize = 2;

impl<T: Eq> PartialEq for TestResult<T> {
    fn eq(&self, other: &Self) -> bool {
        // Trace is excluded from equality -- it contains wall-clock timing
        // that may differ between runs.
        self.num_polls.abs_diff(other.num_polls) <= NUM_POLLS_TOLERANCE
            && self.rng_next_u64 == other.rng_next_u64
            && self.output == other.output
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
    let run_id = DST_RUN_ID.load(Ordering::Relaxed);
    dst_log(&format!("=== run_once START (run_id={}, seed={}) ===", run_id, config.seed));
    // #endregion

    let recorder = EventRecorder::active();
    let rt = td.rt_with_event_recorder(recorder.clone());
    let application = td.run_until(async {
        // #region agent log
        let polls_start = tokio::runtime::Handle::current().metrics().worker_poll_count(0);
        dst_log(&format!("create_app: polls_before={}", polls_start));
        // #endregion

        let application = Application::new_for_tests(&rt).await?;

        // #region agent log
        let polls_end = tokio::runtime::Handle::current().metrics().worker_poll_count(0);
        dst_log(&format!("create_app: polls_after={}", polls_end));
        // #endregion

        tracing::info!(
            "[nitpick] Created application in {:?}",
            test_start.elapsed()
        );
        anyhow::Ok(application)
    })?;

    let test_run = td.run_until(async {
        // #region agent log
        let polls_start = tokio::runtime::Handle::current().metrics().worker_poll_count(0);
        dst_log(&format!("start_run: polls_before={}", polls_start));
        // #endregion

        let run = scenario.start_run(td.rt(), &application).await?;

        // #region agent log
        let polls_end = tokio::runtime::Handle::current().metrics().worker_poll_count(0);
        dst_log(&format!("start_run: polls_after={}", polls_end));
        // #endregion

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
    td.run_until(async {
        // #region agent log
        let polls_start = tokio::runtime::Handle::current().metrics().worker_poll_count(0);
        dst_log(&format!("validate: polls_before={}", polls_start));
        // #endregion

        test_run.validate(&application).await?;

        // #region agent log
        let polls_end = tokio::runtime::Handle::current().metrics().worker_poll_count(0);
        dst_log(&format!("validate: polls_after={}", polls_end));
        // #endregion

        anyhow::Ok(())
    })?;
    tracing::info!(
        "[nitpick] Final verification passed in {:?}",
        start.elapsed()
    );

    let output = td.run_until(async {
        // #region agent log
        let polls_start = tokio::runtime::Handle::current().metrics().worker_poll_count(0);
        dst_log(&format!("finalize: polls_before={}", polls_start));
        // #endregion

        let output = test_run.finalize(&application).await?;

        // #region agent log
        let polls_end = tokio::runtime::Handle::current().metrics().worker_poll_count(0);
        dst_log(&format!("finalize: polls_after={}", polls_end));
        // #endregion

        anyhow::Ok(output)
    })?;

    let trace = recorder.drain();
    let result = TestResult {
        num_polls,
        rng_next_u64: td.rt().rng().random(),
        output,
        trace,
    };

    // #region agent log
    dst_log(&format!(
        "=== run_once END (run_id={}, num_polls={}, rng_next_u64={}) ===",
        run_id, result.num_polls, result.rng_next_u64
    ));
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

    // #region agent log
    let polls_at_tx_start = tokio::runtime::Handle::current()
        .metrics()
        .worker_poll_count(0);
    dst_log(&format!("run_transactions: START worker_poll_count={}", polls_at_tx_start));
    let mut loop_iter: u64 = 0;
    // #endregion

    loop {
        // #region agent log
        loop_iter += 1;
        let polls_loop = tokio::runtime::Handle::current()
            .metrics()
            .worker_poll_count(0);
        dst_log(&format!(
            "run_transactions: loop_iter={} completed={}/{} in_flight={} next_tx={} worker_poll_count={}",
            loop_iter, num_completed, config.transactions, transactions.len(), next_tx_id, polls_loop
        ));
        // #endregion

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
        // #region agent log
        let drained_count = results.len();
        // #endregion
        for r in results {
            if let Err(e) = r {
                if e.is_occ() {
                    tracing::debug!("[nitpick] Transaction OCC'd");
                    // #region agent log
                    dst_log(&format!("run_transactions: loop_iter={} tx_OCC", loop_iter));
                    // #endregion
                    continue;
                }
                return Err(e);
            }
            num_completed += 1;
            // #region agent log
            dst_log(&format!(
                "run_transactions: loop_iter={} tx_completed num_completed={}",
                loop_iter, num_completed
            ));
            // #endregion
        }
        // #region agent log
        if drained_count > 0 {
            let polls_after_drain = tokio::runtime::Handle::current()
                .metrics()
                .worker_poll_count(0);
            dst_log(&format!(
                "run_transactions: loop_iter={} after_drain drained={} worker_poll_count={}",
                loop_iter, drained_count, polls_after_drain
            ));
        }
        // #endregion
        if num_completed >= config.transactions {
            break;
        }

        // Refill transactions.
        // #region agent log
        let mut refill_count = 0u32;
        // #endregion
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
            // #region agent log
            refill_count += 1;
            // #endregion
        }
        // #region agent log
        if refill_count > 0 {
            dst_log(&format!(
                "run_transactions: loop_iter={} refilled={} next_tx={}",
                loop_iter, refill_count, next_tx_id
            ));
        }
        // #endregion

        // Probabilistically validate.
        if rt.rng().random_bool(VERIFY_PROBABILITY) {
            // #region agent log
            let polls_before_verify = tokio::runtime::Handle::current()
                .metrics()
                .worker_poll_count(0);
            dst_log(&format!(
                "run_transactions: loop_iter={} verify_start worker_poll_count={}",
                loop_iter, polls_before_verify
            ));
            // #endregion
            test_run.validate(application).await?;
            // #region agent log
            let polls_after_verify = tokio::runtime::Handle::current()
                .metrics()
                .worker_poll_count(0);
            dst_log(&format!(
                "run_transactions: loop_iter={} verify_end worker_poll_count={}",
                loop_iter, polls_after_verify
            ));
            // #endregion
        }
    }

    let num_polls = tokio::runtime::Handle::current()
        .metrics()
        .worker_poll_count(0);
    // #region agent log
    dst_log(&format!(
        "run_transactions: END worker_poll_count={} loop_iters={}",
        num_polls, loop_iter
    ));
    // #endregion
    Ok(num_polls as usize)
}

/// How likely we are to run a determinism check (re-run with same seed).
// #region agent log
const DETERMINISM_CHECK_PROBABILITY: f64 = 1.0; // was 0.1
// #endregion

/// Run a scenario with the given config on a dedicated thread with a large
/// stack. Probabilistically checks determinism by re-running with the same seed.
pub fn run_scenario<S: Scenario>(scenario: S, config: Config) -> anyhow::Result<()> {
    let thread_handle = std::thread::Builder::new()
        .stack_size(*RUNTIME_STACK_SIZE)
        .spawn(move || {
            // #region agent log
            // Clean up old log files from previous runs.
            let _ = std::fs::remove_file("/tmp/nitpick_run1.log");
            let _ = std::fs::remove_file("/tmp/nitpick_run2.log");
            DST_RUN_ID.store(1, Ordering::Relaxed);
            DST_THREAD_FUTURE_ID.store(0, Ordering::Relaxed);
            // #endregion

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

    // #region agent log
    DST_RUN_ID.store(2, Ordering::Relaxed);
    DST_THREAD_FUTURE_ID.store(0, Ordering::Relaxed);
    dst_log(&format!("=== DETERMINISM CHECK START seed={} ===", config.seed));
    // #endregion

    let td = TestDriver::new_with_seed(config.seed);
    let run2 = run_once(scenario, &td, config)?;
    if *run1 != run2 {
        // #region agent log
        dst_log(&format!(
            "DETERMINISM FAILURE: run1.num_polls={} run2.num_polls={} run1.rng={} run2.rng={}",
            run1.num_polls, run2.num_polls, run1.rng_next_u64, run2.rng_next_u64
        ));
        // #endregion
        anyhow::bail!(
            "Determinism failure for seed {}:\n  run1: {:?}\n  run2: {:?}",
            config.seed,
            run1,
            run2
        );
    }
    tracing::info!("[nitpick] Determinism check passed");
    Ok(())
}
