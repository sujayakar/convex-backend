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
    dst_log,
    TestDriver,
    TestRuntime,
};
// #region agent log
use runtime::testing::{DST_RUN_ID, DST_TF_ID};
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
        // Trace is excluded from equality -- it contains wall-clock timing
        // that may differ between runs.
        self.num_polls == other.num_polls
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

    let recorder = EventRecorder::active();
    let rt = td.rt_with_event_recorder(recorder.clone());
    let application = td.run_until(async {
        let application = Application::new_for_tests(&rt).await?;
        // #region agent log
        {
            let wpc = tokio::runtime::Handle::current()
                .metrics()
                .worker_poll_count(0);
            dst_log(&format!("RUN_ONCE after_application_new_for_tests wpc={}", wpc));
        }
        // #endregion
        tracing::info!(
            "[nitpick] Created application in {:?}",
            test_start.elapsed()
        );
        anyhow::Ok(application)
    })?;

    let test_run = td.run_until(async {
        let run = scenario.start_run(td.rt(), &application).await?;
        // #region agent log
        {
            let wpc = tokio::runtime::Handle::current()
                .metrics()
                .worker_poll_count(0);
            dst_log(&format!("RUN_ONCE after_scenario_start_run wpc={}", wpc));
        }
        // #endregion
        tracing::info!(
            "[nitpick] Initialized scenario {:?} in {:?}",
            scenario.name(),
            test_start.elapsed()
        );
        anyhow::Ok(run)
    })?;

    let num_polls = td.run_until(run_transactions(td.rt(), &application, &test_run, config))?;
    // #region agent log
    dst_log(&format!(
        "RUN_ONCE after_run_transactions returned_num_polls={}",
        num_polls
    ));
    // #endregion

    // Always run validation at the end.
    let start = Instant::now();
    let wpc_after_validate = td.run_until(async {
        test_run.validate(&application).await?;
        let wpc = tokio::runtime::Handle::current()
            .metrics()
            .worker_poll_count(0);
        anyhow::Ok(wpc)
    })?;
    // #region agent log
    dst_log(&format!(
        "RUN_ONCE after_test_run_validate wpc={}",
        wpc_after_validate
    ));
    // #endregion
    tracing::info!(
        "[nitpick] Final verification passed in {:?}",
        start.elapsed()
    );

    let (output, wpc_after_finalize) = td.run_until(async {
        let output = test_run.finalize(&application).await?;
        let wpc = tokio::runtime::Handle::current()
            .metrics()
            .worker_poll_count(0);
        anyhow::Ok((output, wpc))
    })?;
    // #region agent log
    dst_log(&format!(
        "RUN_ONCE after_test_run_finalize wpc={}",
        wpc_after_finalize
    ));
    // #endregion

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
        // #region agent log
        {
            let wpc = tokio::runtime::Handle::current()
                .metrics()
                .worker_poll_count(0);
            dst_log(&format!(
                "RUN_TX loop_start completed={} in_flight={} next_tx_id={} max_tx_attempts={} wpc={}",
                num_completed,
                transactions.len(),
                next_tx_id,
                max_tx_attempts,
                wpc
            ));
        }
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
        for r in results {
            if let Err(e) = r {
                if e.is_occ() {
                    // #region agent log
                    {
                        let wpc = tokio::runtime::Handle::current()
                            .metrics()
                            .worker_poll_count(0);
                        dst_log(&format!(
                            "RUN_TX tx_complete status=occ completed={} in_flight={} next_tx_id={} wpc={}",
                            num_completed,
                            transactions.len(),
                            next_tx_id,
                            wpc
                        ));
                    }
                    // #endregion
                    tracing::debug!("[nitpick] Transaction OCC'd");
                    continue;
                }
                // #region agent log
                {
                    let wpc = tokio::runtime::Handle::current()
                        .metrics()
                        .worker_poll_count(0);
                    dst_log(&format!(
                        "RUN_TX tx_complete status=error completed={} in_flight={} next_tx_id={} err={:?} wpc={}",
                        num_completed,
                        transactions.len(),
                        next_tx_id,
                        e,
                        wpc
                    ));
                }
                // #endregion
                return Err(e);
            }
            num_completed += 1;
            // #region agent log
            {
                let wpc = tokio::runtime::Handle::current()
                    .metrics()
                    .worker_poll_count(0);
                dst_log(&format!(
                    "RUN_TX tx_complete status=ok completed={} in_flight={} next_tx_id={} wpc={}",
                    num_completed,
                    transactions.len(),
                    next_tx_id,
                    wpc
                ));
            }
            // #endregion
        }
        // #region agent log
        {
            let wpc = tokio::runtime::Handle::current()
                .metrics()
                .worker_poll_count(0);
            dst_log(&format!(
                "RUN_TX loop_after_drain completed={} in_flight={} next_tx_id={} wpc={}",
                num_completed,
                transactions.len(),
                next_tx_id,
                wpc
            ));
        }
        // #endregion
        if num_completed >= config.transactions {
            // #region agent log
            {
                let wpc = tokio::runtime::Handle::current()
                    .metrics()
                    .worker_poll_count(0);
                dst_log(&format!(
                    "RUN_TX loop_break_target_reached completed={} target={} in_flight={} next_tx_id={} wpc={}",
                    num_completed,
                    config.transactions,
                    transactions.len(),
                    next_tx_id,
                    wpc
                ));
            }
            // #endregion
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
        // #region agent log
        {
            let wpc = tokio::runtime::Handle::current()
                .metrics()
                .worker_poll_count(0);
            dst_log(&format!(
                "RUN_TX loop_after_refill completed={} in_flight={} next_tx_id={} wpc={}",
                num_completed,
                transactions.len(),
                next_tx_id,
                wpc
            ));
        }
        // #endregion

        // Probabilistically validate.
        if rt.rng().random_bool(VERIFY_PROBABILITY) {
            // #region agent log
            {
                let wpc_before = tokio::runtime::Handle::current()
                    .metrics()
                    .worker_poll_count(0);
                dst_log(&format!(
                    "RUN_TX verify_branch taken completed={} in_flight={} next_tx_id={} wpc_before={}",
                    num_completed,
                    transactions.len(),
                    next_tx_id,
                    wpc_before
                ));
            }
            // #endregion
            test_run.validate(application).await?;
            // #region agent log
            {
                let wpc_after = tokio::runtime::Handle::current()
                    .metrics()
                    .worker_poll_count(0);
                dst_log(&format!(
                    "RUN_TX verify_branch_done completed={} in_flight={} next_tx_id={} wpc_after={}",
                    num_completed,
                    transactions.len(),
                    next_tx_id,
                    wpc_after
                ));
            }
            // #endregion
        } else {
            // #region agent log
            {
                let wpc = tokio::runtime::Handle::current()
                    .metrics()
                    .worker_poll_count(0);
                dst_log(&format!(
                    "RUN_TX verify_branch skipped completed={} in_flight={} next_tx_id={} wpc={}",
                    num_completed,
                    transactions.len(),
                    next_tx_id,
                    wpc
                ));
            }
            // #endregion
        }
    }

    let num_polls = tokio::runtime::Handle::current()
        .metrics()
        .worker_poll_count(0);
    // #region agent log
    dst_log(&format!("RUN_TX return_num_polls={}", num_polls));
    // #endregion
    Ok(num_polls as usize)
}

/// How likely we are to run a determinism check (re-run with same seed).
// #region agent log
const DETERMINISM_CHECK_PROBABILITY: f64 = 1.0;
// #endregion

/// Run a scenario with the given config on a dedicated thread with a large
/// stack. Probabilistically checks determinism by re-running with the same seed.
pub fn run_scenario<S: Scenario>(scenario: S, config: Config) -> anyhow::Result<()> {
    let thread_handle = std::thread::Builder::new()
        .stack_size(*RUNTIME_STACK_SIZE)
        .spawn(move || {
            // #region agent log
            DST_RUN_ID.store(1, Ordering::Relaxed);
            DST_TF_ID.store(0, Ordering::Relaxed);
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
    DST_TF_ID.store(0, Ordering::Relaxed);
    // #endregion
    let td = TestDriver::new_with_seed(config.seed);
    let run2 = run_once(scenario, &td, config)?;
    if *run1 != run2 {
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
