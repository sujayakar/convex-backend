mod thread_future;
// `defer_waker_to_tokio_thread` is still used by `DstOneshotSender`.
pub use thread_future::defer_waker_to_tokio_thread;

mod dst_oneshot;
pub use dst_oneshot::{
    dst_oneshot_channel,
    DstOneshotReceiver,
    DstOneshotSender,
};

mod dst_mpsc;
pub use dst_mpsc::{
    dst_mpsc_channel,
    DstMpscReceiver,
    DstMpscSender,
    TrySendError as DstTrySendError,
};

use std::{
    backtrace::Backtrace,
    fs::OpenOptions,
    io::Write,
    pin::Pin,
    sync::{
        Arc,
        LazyLock,
        Weak,
    },
    time::{
        Duration,
        SystemTime,
    },
};

use futures::{
    future::FusedFuture,
    Future,
    FutureExt,
};
use parking_lot::Mutex;
use rand::{
    RngCore,
    SeedableRng,
};
use rand_chacha::ChaCha12Rng;
use thread_future::ThreadFuture;
use tokio::runtime::{
    Builder,
    RngSeed,
    UnhandledPanic,
};

use super::{
    Runtime,
    SpawnHandle,
    TokioSpawnHandle,
};
use crate::pause::PauseClient;

pub static CONVEX_EPOCH: LazyLock<SystemTime> =
    LazyLock::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1620198000)); // May 5th, 2021 :)

fn nitpick_rng_run_id() -> Option<u32> {
    let run_id = std::env::var("NITPICK_RUN_ID").ok()?;
    run_id.parse::<u32>().ok().filter(|id| *id > 0)
}

fn nitpick_rng_log_path() -> Option<String> {
    let run_id = nitpick_rng_run_id()?;
    Some(format!("/tmp/nitpick_rng_run{run_id}.log"))
}

pub fn reset_rng_log_for_current_run() {
    let Some(path) = nitpick_rng_log_path() else {
        return;
    };
    let _ = std::fs::remove_file(path);
}

pub fn rng_log(message: &str) {
    let Some(path) = nitpick_rng_log_path() else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

pub struct TestDriver {
    tokio_runtime: Option<tokio::runtime::Runtime>,
    state: Arc<Mutex<TestRuntimeState>>,
    pause_client: PauseClient,
}

impl TestDriver {
    pub fn new() -> Self {
        Self::new_with_seed(0)
    }

    pub fn new_with_seed(seed: u64) -> Self {
        Self::new_with_config(seed, PauseClient::new())
    }

    pub fn new_with_pause_client(pause_client: PauseClient) -> Self {
        Self::new_with_config(0, pause_client)
    }

    pub fn new_with_config(seed: u64, pause_client: PauseClient) -> Self {
        let tokio_seed = RngSeed::from_bytes(&seed.to_le_bytes());
        let tokio_runtime = Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .unhandled_panic(UnhandledPanic::ShutdownRuntime)
            .rng_seed(tokio_seed)
            .build()
            .expect("Failed to create Tokio runtime");
        let rng = ChaCha12Rng::seed_from_u64(seed);
        let creation_time = {
            let _handle = tokio_runtime.enter();
            tokio::time::Instant::now()
        };
        Self {
            tokio_runtime: Some(tokio_runtime),
            state: Arc::new(Mutex::new(TestRuntimeState {
                creation_time,
                rng,
                rng_next_u64_calls: 0,
            })),
            pause_client,
        }
    }

    pub fn rt(&self) -> TestRuntime {
        self.rt_with_event_recorder(crate::event_recorder::EventRecorder::new())
    }

    /// Create a TestRuntime with a specific EventRecorder (e.g., an active
    /// one for simulation testing).
    pub fn rt_with_event_recorder(
        &self,
        event_recorder: crate::event_recorder::EventRecorder,
    ) -> TestRuntime {
        TestRuntime {
            tokio_handle: self
                .tokio_runtime
                .as_ref()
                .expect("tokio_runtime disappeared?")
                .handle()
                .clone(),
            state: Arc::downgrade(&self.state),
            _owned_state: None,
            pause_client: self.pause_client.clone(),
            event_recorder,
        }
    }

    pub fn run_until<F: Future>(&self, f: F) -> F::Output {
        self.tokio_runtime
            .as_ref()
            .expect("tokio_runtime disappeared?")
            .block_on(f)
    }

    pub fn rng_next_u64_call_count(&self) -> usize {
        self.state.lock().rng_next_u64_calls
    }
}

impl Drop for TestDriver {
    fn drop(&mut self) {
        assert_eq!(Arc::strong_count(&self.state), 1);
        // Use a blocking shutdown so all spawned tasks (including
        // `ThreadFuture` OS threads hosting V8 isolates) are fully
        // joined before this `TestDriver` is dropped.
        // `shutdown_background()` is non-blocking and can leave V8
        // worker threads alive, causing the V8 platform thread to
        // serve stale isolates concurrently with a subsequent run's
        // isolates — a source of cross-run non-determinism.
        self.tokio_runtime
            .take()
            .expect("tokio_runtime disappeared?")
            .shutdown_timeout(std::time::Duration::from_secs(5));
    }
}

struct TestRuntimeState {
    creation_time: tokio::time::Instant,
    rng: ChaCha12Rng,
    rng_next_u64_calls: usize,
}

#[derive(Clone)]
pub struct TestRuntime {
    tokio_handle: tokio::runtime::Handle,
    state: Weak<Mutex<TestRuntimeState>>,
    /// If this runtime was created via `fork_rng()`, holds the owning Arc
    /// to keep the forked state alive.
    _owned_state: Option<Arc<Mutex<TestRuntimeState>>>,
    pause_client: PauseClient,
    event_recorder: crate::event_recorder::EventRecorder,
}

impl TestRuntime {
    fn with_state<R>(&self, f: impl FnOnce(&mut TestRuntimeState) -> R) -> R {
        let state = self
            .state
            .upgrade()
            .expect("TestRuntime is used after `TestDriver` has been dropped");
        let mut state = state.lock();
        f(&mut state)
    }

    pub async fn advance_time(&self, duration: Duration) {
        tokio::time::advance(duration).await
    }

    /// Create a copy of this runtime with an independent RNG, seeded from
    /// the current shared RNG.  This allows concurrent tasks to consume
    /// their own deterministic RNG without the ordering of RNG calls
    /// depending on task scheduling.
    pub fn fork_rng(&self) -> Self {
        let seed: u64 = self.with_state(|state| state.rng.next_u64());
        let forked_rng = ChaCha12Rng::seed_from_u64(seed);
        let creation_time = self.with_state(|state| state.creation_time);
        let owned = Arc::new(Mutex::new(TestRuntimeState {
            rng: forked_rng,
            creation_time,
            rng_next_u64_calls: 0,
        }));
        TestRuntime {
            tokio_handle: self.tokio_handle.clone(),
            state: Arc::downgrade(&owned),
            _owned_state: Some(owned),
            pause_client: self.pause_client.clone(),
            event_recorder: self.event_recorder.clone(),
        }
    }
}

impl Runtime for TestRuntime {
    fn wait(&self, duration: Duration) -> Pin<Box<dyn FusedFuture<Output = ()> + Send + 'static>> {
        // NB: `TestRuntime` uses Tokio's current thread runtime with the timer paused,
        // so can still achieve determinism. This sleep will suspend until either time
        // is manually advanced forward, or the Tokio runtime runs out of work to do and
        // auto advances to the next pending timer.
        Box::pin(tokio::time::sleep(duration).fuse())
    }

    fn spawn(
        &self,
        _name: &'static str,
        f: impl Future<Output = ()> + Send + 'static,
    ) -> Box<dyn SpawnHandle> {
        let handle = self.tokio_handle.spawn(f);
        Box::new(TokioSpawnHandle::from(handle))
    }

    fn spawn_thread<Fut: Future<Output = ()>, F: FnOnce() -> Fut + Send + 'static>(
        &self,
        _name: &str,
        f: F,
    ) -> Box<dyn SpawnHandle> {
        let handle = self
            .tokio_handle
            .spawn(ThreadFuture::new(self.tokio_handle.clone(), f));
        Box::new(TokioSpawnHandle::from(handle))
    }

    fn system_time(&self) -> SystemTime {
        let elapsed = tokio::time::Instant::now() - self.with_state(|state| state.creation_time);
        *CONVEX_EPOCH + elapsed
    }

    fn monotonic_now(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }

    fn rng(&self) -> Box<dyn RngCore> {
        Box::new(TestRng { rt: self.clone() })
    }

    fn pause_client(&self) -> PauseClient {
        self.pause_client.clone()
    }

    fn event_recorder(&self) -> crate::event_recorder::EventRecorder {
        self.event_recorder.clone()
    }
}

struct TestRng {
    rt: TestRuntime,
}

fn rng_backtrace_summary() -> String {
    let mut frames = vec![];
    let raw = format!("{:?}", Backtrace::force_capture());
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line == "stack backtrace:" {
            continue;
        }
        if line.contains("common::runtime::testing::rng_backtrace_summary")
            || line.contains("common::runtime::testing::TestRng::next_u64")
        {
            continue;
        }
        frames.push(line.to_string());
        if frames.len() >= 6 {
            break;
        }
    }
    if frames.is_empty() {
        "no_frames".to_string()
    } else {
        frames.join(" | ")
    }
}

impl RngCore for TestRng {
    fn next_u32(&mut self) -> u32 {
        self.rt.with_state(|state| state.rng.next_u32())
    }

    fn next_u64(&mut self) -> u64 {
        let (value, call_count) = self.rt.with_state(|state| {
            let value = state.rng.next_u64();
            state.rng_next_u64_calls += 1;
            (value, state.rng_next_u64_calls)
        });
        // #region agent log
        rng_log(&format!(
            "RNG_NEXT_U64 call={} value={} backtrace={}",
            call_count,
            value,
            rng_backtrace_summary()
        ));
        // #endregion
        value
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.rt.with_state(|state| state.rng.fill_bytes(dest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::JoinError;

    #[test]
    fn test_runtime2() -> anyhow::Result<()> {
        let td = TestDriver::new_with_seed(0);
        let rt = td.rt();
        td.run_until(async {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let mut r = rt.spawn_thread("test", || async move {
                println!("hi!");
                let _ = tx.send(());
            });
            println!("there!");
            let _ = rx.await;
            r.shutdown();
            let (Ok(()) | Err(JoinError::Canceled)) = r.join().await else {
                panic!("Expected JoinError::Canceled");
            };
        });
        Ok(())
    }
}
