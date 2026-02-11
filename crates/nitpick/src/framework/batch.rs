use std::{
    cell::Cell,
    panic,
    sync::{
        atomic::{
            AtomicUsize,
            Ordering,
        },
        Arc,
    },
    thread,
};

use rand::Rng;

use super::{
    runner::{
        run_scenario,
        Config,
    },
    scenario::Scenario,
};

/// Result of a batch run.
pub struct BatchResult {
    pub total: usize,
    pub passed: usize,
    pub failed: Option<(u64, anyhow::Error)>, // (seed, error)
}

/// Run many simulations of a scenario in parallel with random seeds.
///
/// Reports progress and, on failure, prints the seed for replay.
pub fn run_batch<S: Scenario + Clone>(
    scenario: S,
    concurrency: usize,
    transactions: usize,
    num_simulations: usize,
    num_threads: usize,
) -> BatchResult {
    let counter = Arc::new(AtomicUsize::new(0));
    let error: Arc<std::sync::Mutex<Option<(u64, String)>>> =
        Arc::new(std::sync::Mutex::new(None));

    // Install panic hook that captures the seed.
    thread_local! {
        static CURRENT_SEED: Cell<Option<u64>> = const { Cell::new(None) };
    }
    panic::update_hook(move |prev, info| {
        if let Some(seed) = CURRENT_SEED.get() {
            eprintln!("[nitpick] Panic while executing seed {seed}");
        }
        prev(info);
    });

    // Spawn worker threads.
    let work_index = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];

    for _ in 0..num_threads {
        let scenario = scenario.clone();
        let work_index = work_index.clone();
        let counter = counter.clone();
        let error = error.clone();

        let handle = thread::spawn(move || {
            let mut rng = rand::rng();
            loop {
                let idx = work_index.fetch_add(1, Ordering::SeqCst);
                if idx >= num_simulations {
                    break;
                }

                // Check if another thread found an error.
                if error.lock().unwrap().is_some() {
                    break;
                }

                let seed: u64 = rng.random();
                CURRENT_SEED.set(Some(seed));

                let config = Config {
                    transactions,
                    concurrency,
                    seed,
                    fault_config: None,
                };

                if let Err(e) = run_scenario(scenario.clone(), config) {
                    let mut err = error.lock().unwrap();
                    if err.is_none() {
                        *err = Some((seed, format!("{e:?}")));
                    }
                    break;
                }

                CURRENT_SEED.set(None);
                let completed = counter.fetch_add(1, Ordering::SeqCst) + 1;
                if completed % 100 == 0 || completed == num_simulations {
                    tracing::info!(
                        "[nitpick] Progress: {completed}/{num_simulations} simulations"
                    );
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.join();
    }

    let passed = counter.load(Ordering::SeqCst);
    let failed = error
        .lock()
        .unwrap()
        .take()
        .map(|(seed, msg)| (seed, anyhow::anyhow!(msg)));

    if let Some((seed, ref err)) = failed {
        eprintln!(
            "\n[nitpick] FAILURE on seed {seed}, concurrency={concurrency}, \
             transactions={transactions}"
        );
        eprintln!("[nitpick] Error: {err}");
        eprintln!(
            "[nitpick] Replay: cargo test -p nitpick -- --nocapture # with seed={seed}"
        );
    } else {
        tracing::info!(
            "[nitpick] Batch complete: {passed}/{num_simulations} passed"
        );
    }

    BatchResult {
        total: num_simulations,
        passed,
        failed,
    }
}
