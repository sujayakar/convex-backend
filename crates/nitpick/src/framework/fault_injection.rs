use std::{
    collections::BTreeMap,
    sync::Arc,
    time::Duration,
};

use common::pause::{
    Fault,
    PauseClient,
};
use parking_lot::Mutex;
use rand::{
    Rng,
    SeedableRng,
};
use rand_chacha::ChaCha12Rng;

/// Configuration for randomized fault injection at breakpoints.
#[derive(Clone, Copy, Debug)]
pub struct FaultConfig {
    /// Probability of injecting `Fault::Error` (0.0 to 1.0).
    pub error_probability: f64,

    /// Probability of injecting a random delay (0.0 to 1.0).
    /// The delay advances simulated time without causing an error.
    pub delay_probability: f64,

    /// Maximum delay duration when a delay is injected.
    pub max_delay: Duration,
}

impl Default for FaultConfig {
    fn default() -> Self {
        Self {
            error_probability: 0.02,
            delay_probability: 0.05,
            max_delay: Duration::from_millis(10),
        }
    }
}

/// Internal mutable state for the DST pause controller.
struct DstPauseState {
    rng: ChaCha12Rng,
    config: FaultConfig,
    hits: BTreeMap<&'static str, usize>,
}

/// A deterministic simulation testing (DST) pause controller that randomly
/// injects faults at breakpoints.
///
/// Given a seed, it deterministically decides at each breakpoint whether to:
/// - Pass through immediately (no-op)
/// - Inject a random delay (advances simulated time)
/// - Inject `Fault::Error`
///
/// The controller also tracks which breakpoints were hit for coverage reporting.
pub struct DstPauseController {
    state: Arc<Mutex<DstPauseState>>,
}

impl DstPauseController {
    /// Create a new `DstPauseController` and a `PauseClient` wired to it.
    ///
    /// The `seed` determines all random decisions (fault type, delay duration).
    /// The returned `PauseClient` should be passed to
    /// `TestDriver::new_with_config`.
    pub fn new(seed: u64, config: FaultConfig) -> (Self, PauseClient) {
        let state = Arc::new(Mutex::new(DstPauseState {
            rng: ChaCha12Rng::seed_from_u64(seed),
            config,
            hits: BTreeMap::new(),
        }));
        let state_for_injector = state.clone();
        let injector = Arc::new(move |label: &'static str| -> (Option<Duration>, Fault) {
            let mut state = state_for_injector.lock();
            *state.hits.entry(label).or_insert(0) += 1;

            // Roll a random number to decide what to do.
            let roll: f64 = state.rng.random();
            if roll < state.config.error_probability {
                // Inject an error.
                tracing::info!("[dst] Injecting Fault::Error at breakpoint {label:?}");
                (None, Fault::Error(anyhow::anyhow!(
                    "DST injected fault at breakpoint {label:?}"
                )))
            } else if roll < state.config.error_probability + state.config.delay_probability {
                // Inject a random delay.
                let max_ms = state.config.max_delay.as_millis() as u64;
                let delay_ms = if max_ms > 0 {
                    state.rng.random_range(1..=max_ms)
                } else {
                    0
                };
                let delay = Duration::from_millis(delay_ms);
                tracing::debug!(
                    "[dst] Injecting {delay:?} delay at breakpoint {label:?}"
                );
                (Some(delay), Fault::Noop)
            } else {
                // Pass through.
                tracing::debug!("[dst] Pass-through at breakpoint {label:?}");
                (None, Fault::Noop)
            }
        });

        let client = PauseClient::new_with_fault_injector(injector);
        let controller = Self { state };
        (controller, client)
    }

    /// Return a snapshot of breakpoint hit counts for coverage reporting.
    pub fn hits(&self) -> BTreeMap<&'static str, usize> {
        self.state.lock().hits.clone()
    }

    /// Total number of breakpoint hits across all labels.
    pub fn total_hits(&self) -> usize {
        self.state.lock().hits.values().sum()
    }
}
