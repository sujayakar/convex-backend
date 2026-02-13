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

impl<T: Eq> PartialEq for TestResult<T> {
    fn eq(&self, other: &Self) -> bool {
        // `num_polls` is excluded from determinism equality. Tokio's
        // `worker_poll_count` includes scheduler-internal/background task polls
        // and is only flushed at scheduler submit points, so it can differ under
        // host CPU contention even when logical execution is identical.
        //
        // Trace is excluded as well since it includes wall-clock timing.
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
    if *run1 != run2 {
        anyhow::bail!("{}", determinism_failure_message(config.seed, run1, &run2));
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
) -> String {
    format!(
        "Determinism failure for seed {}:\n  run1: {{ rng_next_u64: {}, output: {:?} }}\n  run2: {{ rng_next_u64: {}, output: {:?} }}\n  diagnostics: {{ run1_num_polls: {}, run2_num_polls: {}, run1_trace_len: {}, run2_trace_len: {} }}",
        seed,
        run1.rng_next_u64,
        run1.output,
        run2.rng_next_u64,
        run2.output,
        run1.num_polls,
        run2.num_polls,
        run1.trace.len(),
        run2.trace.len(),
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use common::event_recorder::{
        Event,
        TraceEvent,
    };

    use super::{
        determinism_failure_message,
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
                event: Event::TransactionCommit,
            }],
        };

        assert_eq!(run1, run2);
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

        let message = determinism_failure_message(123, &run1, &run2);

        assert!(message.contains("Determinism failure for seed 123"));
        assert!(message.contains("run1_num_polls: 100"));
        assert!(message.contains("run2_num_polls: 103"));
        assert!(message.contains("run1_trace_len: 1"));
        assert!(message.contains("run2_trace_len: 1"));
        assert!(
            !message.contains("TraceEvent"),
            "message should avoid dumping full traces"
        );
    }
}
