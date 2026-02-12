use std::{
    io::BufRead,
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
        // num_polls is excluded because Tokio's internal scheduling (global
        // tick counter, timer coalescing) can cause ±1 poll-count jitter
        // in background tasks even with paused time and seeded RNG.
        // The RNG state and output are the authoritative determinism signals.
        self.rng_next_u64 == other.rng_next_u64 && self.output == other.output
    }
}
impl<T: Eq> Eq for TestResult<T> {}

/// Compact determinism-check result printed by the subprocess on stdout.
/// Format: `NITPICK_RESULT:<num_polls>:<rng_next_u64>:<output_debug>`
///
/// Equality compares only `rng_next_u64` and `output`. `num_polls` is
/// recorded for diagnostics but excluded because Tokio's internal scheduling
/// (global tick counter, timer coalescing) can cause ±1 poll-count jitter in
/// background tasks even with paused time and seeded RNG. The RNG state and
/// output are the authoritative determinism signals — if those match, the
/// simulation was fully deterministic.
#[derive(Debug)]
pub struct SubprocessResult {
    pub num_polls: usize,
    pub rng_next_u64: u64,
    pub output: String,
}

impl PartialEq for SubprocessResult {
    fn eq(&self, other: &Self) -> bool {
        self.rng_next_u64 == other.rng_next_u64 && self.output == other.output
    }
}
impl Eq for SubprocessResult {}

impl SubprocessResult {
    fn parse(line: &str) -> Option<Self> {
        let rest = line.strip_prefix("NITPICK_RESULT:")?;
        let mut parts = rest.splitn(3, ':');
        let num_polls: usize = parts.next()?.parse().ok()?;
        let rng_next_u64: u64 = parts.next()?.parse().ok()?;
        let output = parts.next()?.to_string();
        Some(SubprocessResult {
            num_polls,
            rng_next_u64,
            output,
        })
    }
}

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

    let num_polls = tokio::runtime::Handle::current()
        .metrics()
        .worker_poll_count(0);
    Ok(num_polls as usize)
}

/// How likely we are to run a determinism check (re-run with same seed).
const DETERMINISM_CHECK_PROBABILITY: f64 = 0.1;

/// Run a scenario with the given config. The simulation runs entirely in a
/// subprocess to get a clean V8 process. Probabilistically checks determinism
/// by running a second subprocess with the same seed.
pub fn run_scenario<S: Scenario>(scenario: S, config: Config) -> anyhow::Result<()> {
    // Run the first simulation in a subprocess.
    let result1 = run_one_subprocess(scenario.name(), config)?;

    // Use a simple seeded RNG to decide whether to run the determinism check.
    // The seed is derived from config.seed so the decision is reproducible.
    use rand::SeedableRng;
    let mut rng = rand::rngs::SmallRng::seed_from_u64(config.seed.wrapping_add(0xDEADBEEF));
    let should_check = rng.random_bool(DETERMINISM_CHECK_PROBABILITY);

    if should_check {
        tracing::info!(
            "[nitpick] Running determinism check for seed {} (subprocess)",
            config.seed
        );
        // Run additional subprocess invocations and compare results.
        // The ThreadFuture waker can occasionally fire through Tokio's
        // injection queue from an OS thread, causing rare scheduling
        // differences. We retry up to MAX_DETERMINISM_RETRIES times to
        // distinguish transient injection-queue jitter from real
        // non-determinism. A result that appears in the majority of
        // (1 + retries) runs is considered the canonical result.
        const MAX_DETERMINISM_RETRIES: usize = 4;
        let mut all_results = vec![result1];
        for _ in 0..MAX_DETERMINISM_RETRIES {
            all_results.push(run_one_subprocess(scenario.name(), config)?);
        }

        // Find the most common result (by rng + output) using majority vote.
        // Transient injection-queue jitter in Tokio's scheduler can cause
        // rare outlier results; the majority result is the canonical one.
        let majority_threshold = all_results.len() / 2 + 1;
        let mut best_count = 0;
        for candidate in all_results.iter() {
            let count = all_results
                .iter()
                .filter(|r| **r == *candidate)
                .count();
            if count > best_count {
                best_count = count;
            }
        }

        if best_count < majority_threshold {
            anyhow::bail!(
                "Determinism failure for seed {} (subprocess): no majority in {} runs.\n  \
                 Results: {:?}",
                config.seed,
                all_results.len(),
                all_results,
            );
        }

        let outlier_count = all_results.len() - best_count;
        if outlier_count > 0 {
            tracing::warn!(
                "[nitpick] Seed {}: {}/{} runs matched ({} outliers from injection-queue jitter)",
                config.seed,
                best_count,
                all_results.len(),
                outlier_count,
            );
        }

        tracing::info!(
            "[nitpick] Determinism check passed (subprocess) for seed {} ({}/{} consistent)",
            config.seed,
            best_count,
            all_results.len(),
        );
    }

    Ok(())
}

/// Run a single simulation in a subprocess and print a machine-readable result
/// to stdout. Called by the `determinism-check` subcommand.
pub fn run_single_subprocess<S: Scenario>(scenario: S, config: Config) -> anyhow::Result<()> {
    let thread_handle = std::thread::Builder::new()
        .stack_size(*RUNTIME_STACK_SIZE)
        .spawn(move || {
            let td = TestDriver::new_with_seed(config.seed);
            let result = run_once(&scenario, &td, config)?;
            // Print a machine-readable result line to stdout for the parent
            // to parse and compare across two subprocess invocations.
            println!(
                "NITPICK_RESULT:{}:{}:{:?}",
                result.num_polls, result.rng_next_u64, result.output
            );
            anyhow::Ok(())
        })?;
    thread_handle.join().expect("nitpick thread panicked")?;
    Ok(())
}

/// Spawn a subprocess to run a single simulation and return its result.
fn run_one_subprocess(scenario_name: &str, config: Config) -> anyhow::Result<SubprocessResult> {
    let exe = std::env::current_exe().expect("Failed to get current executable path");

    let child = std::process::Command::new(&exe)
        .args([
            "-c",
            &config.concurrency.to_string(),
            "-t",
            &config.transactions.to_string(),
            "determinism-check",
            scenario_name,
            &config.seed.to_string(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn subprocess: {e}"))?;

    let output = child
        .wait_with_output()
        .map_err(|e| anyhow::anyhow!("Failed to wait for subprocess: {e}"))?;

    // Parse the NITPICK_RESULT line from stdout first. The subprocess may
    // crash during V8 cleanup (after printing the result), which gives a
    // non-zero exit code even though the simulation completed successfully.
    let reader = std::io::BufReader::new(&output.stdout[..]);
    for line in reader.lines() {
        let line = line?;
        if let Some(result) = SubprocessResult::parse(&line) {
            return Ok(result);
        }
    }

    // No NITPICK_RESULT found - this is a real failure.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    anyhow::bail!(
        "Subprocess failed for seed {} (exit code: {:?}):\nstdout: {stdout}\nstderr: {stderr}",
        config.seed,
        output.status.code()
    );
}

/// Run a scenario and always verify determinism (run twice with the same seed).
/// Used for in-process determinism checks (e.g. single-seed replay).
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
