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
    reset_rng_log_for_current_run,
    rng_log,
    TestDriver,
    TestRuntime,
};

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
        // `num_polls` (Tokio's `worker_poll_count`) includes polls of
        // infrastructure background tasks whose exact poll count can vary
        // slightly.  Trace contains wall-clock timing.  Both are excluded.
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
        // Each transaction gets a forked RNG so that concurrent transactions
        // don't share the global test RNG.  Without this, the order of
        // `rt.rng()` calls depends on task scheduling, causing
        // non-deterministic RNG state across replays.
        while transactions.len() < config.concurrency && next_tx_id < max_tx_attempts {
            let tx_id = next_tx_id;
            next_tx_id += 1;
            let tx_rt = rt.fork_rng();
            let future = test_run.run_transaction(tx_rt, application);
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
const DETERMINISM_CHECK_PROBABILITY: f64 = 1.0;

/// Global RwLock for batch mode: batch worker threads hold a read-lock
/// while running simulations, and the determinism-check subprocess
/// acquires an exclusive write-lock to ensure it has undisturbed CPU
/// access (no concurrent batch workers competing for CPU).
pub static BATCH_CPU_LOCK: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Run a scenario with the given config on a dedicated thread with a large
/// stack. Probabilistically checks determinism by re-running with the same seed.
pub fn run_scenario<S: Scenario>(scenario: S, config: Config) -> anyhow::Result<()> {
    let thread_handle = std::thread::Builder::new()
        .stack_size(*RUNTIME_STACK_SIZE)
        .spawn(move || {
            let should_check = {
                // Hold read lock during the main simulation run.
                let _cpu_guard = BATCH_CPU_LOCK.read().unwrap_or_else(|e| e.into_inner());
                let td = TestDriver::new_with_seed(config.seed);
                let _run1 = run_once(&scenario, &td, config)?;
                td.rt().rng().random_bool(DETERMINISM_CHECK_PROBABILITY)
            };

            if should_check {
                tracing::info!(
                    "[nitpick] Running determinism check for seed {} (subprocess)",
                    config.seed
                );
                // Acquire exclusive lock to pause all other batch threads
                // while the subprocess runs, ensuring deterministic CPU access.
                let _cpu_guard = BATCH_CPU_LOCK
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                check_determinism_subprocess(scenario.name(), config)?;
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
            check_determinism_subprocess(scenario.name(), config)?;
            anyhow::Ok(())
        })?;
    thread_handle.join().expect("nitpick thread panicked")?;
    Ok(())
}

/// Run the determinism check in the current process (called from subprocess).
pub fn check_determinism_in_process<S: Scenario>(
    scenario: S,
    config: Config,
) -> anyhow::Result<()> {
    let thread_handle = std::thread::Builder::new()
        .stack_size(*RUNTIME_STACK_SIZE)
        .spawn(move || {
            struct RunIdEnvGuard;
            impl Drop for RunIdEnvGuard {
                fn drop(&mut self) {
                    unsafe {
                        std::env::remove_var("NITPICK_RUN_ID");
                    }
                }
            }

            let _run_id_guard = RunIdEnvGuard;
            // #region agent log
            unsafe {
                std::env::set_var("NITPICK_RUN_ID", "1");
            }
            reset_rng_log_for_current_run();
            // #endregion
            let (run1, run1_rng_next_u64_calls) = {
                let td = TestDriver::new_with_seed(config.seed);
                let run1 = run_once(&scenario, &td, config)?;
                let call_count = td.rng_next_u64_call_count();
                // #region agent log
                rng_log(&format!(
                    "RUN_SUMMARY run=1 rng_next_u64_calls={call_count}"
                ));
                // #endregion
                (run1, call_count)
            };
            tracing::info!(
                "[nitpick] Running determinism check for seed {} (run1 rng_next_u64_calls={})",
                config.seed,
                run1_rng_next_u64_calls
            );
            // #region agent log
            unsafe {
                std::env::set_var("NITPICK_RUN_ID", "2");
            }
            reset_rng_log_for_current_run();
            // #endregion
            let td = TestDriver::new_with_seed(config.seed);
            let run2 = run_once(&scenario, &td, config)?;
            let run2_rng_next_u64_calls = td.rng_next_u64_call_count();
            // #region agent log
            rng_log(&format!(
                "RUN_SUMMARY run=2 rng_next_u64_calls={run2_rng_next_u64_calls}"
            ));
            // #endregion
            tracing::info!(
                "[nitpick] Determinism check seed {} run2 rng_next_u64_calls={}",
                config.seed,
                run2_rng_next_u64_calls
            );
            if run1 != run2 {
                anyhow::bail!(
                    "Determinism failure for seed {}:\n  run1: {:?}\n  run2: {:?}",
                    config.seed,
                    &run1,
                    &run2
                );
            }
            tracing::info!("[nitpick] Determinism check passed");
            anyhow::Ok(())
        })?;
    thread_handle.join().expect("nitpick thread panicked")?;
    Ok(())
}

/// Spawn a child process to run the determinism check.
///
/// V8's platform is initialized once per process (`Once`) and its internal
/// state (code cache, IC type-feedback, GC thresholds, heap statistics)
/// accumulates across isolate lifetimes.  When run1's V8 work modifies
/// the shared platform state, run2 sees different V8 behavior — different
/// JIT decisions, GC timing, and internal task scheduling — causing
/// non-deterministic poll counts and, in some cases, different application
/// behavior (e.g., different OCC retry patterns).
///
/// By forking a child process, both run1 and run2 start from the same
/// fresh V8 initialization state.  The global lock serializes subprocess
/// invocations to eliminate CPU contention between the subprocess and
/// other batch worker threads.
fn check_determinism_subprocess(scenario_name: &str, config: Config) -> anyhow::Result<()> {
    let exe = std::env::current_exe().expect("Failed to get current exe path");
    let output = std::process::Command::new(&exe)
        .args([
            "--concurrency",
            &config.concurrency.to_string(),
            "--transactions",
            &config.transactions.to_string(),
            "determinism-check",
            scenario_name,
            &config.seed.to_string(),
        ])
        .output()
        .expect("Failed to spawn determinism-check subprocess");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!(
            "Determinism check subprocess failed for seed {}:\nstdout: \
             {}\nstderr: {}",
            config.seed,
            stdout,
            stderr
        );
    }
    Ok(())
}
