mod thread_future;
pub use thread_future::defer_waker_to_tokio_thread;

mod dst_oneshot;
use std::{
    self,
    pin::Pin,
    sync::{
        atomic::{
            AtomicU32,
            AtomicU64,
            Ordering,
        },
        Arc,
        LazyLock,
        Weak,
    },
    time::{
        Duration,
        SystemTime,
    },
};

pub use dst_oneshot::{
    dst_oneshot_channel,
    DstOneshotReceiver,
    DstOneshotSender,
};

// #region agent log
pub static DST_RUN_ID: AtomicU32 = AtomicU32::new(0);
pub static DST_TF_ID: AtomicU32 = AtomicU32::new(0);
pub static DST_SEED: AtomicU64 = AtomicU64::new(0);
pub static DST_EVENT_SEQ: AtomicU64 = AtomicU64::new(0);
static DST_LOG_LOCK: LazyLock<std::sync::Mutex<()>> =
    LazyLock::new(|| std::sync::Mutex::new(()));
pub fn dst_log(hypothesis_id: &str, location: &str, message: &str, data: serde_json::Value) {
    use std::io::Write;
    let run_id = DST_RUN_ID.load(Ordering::Relaxed);
    if run_id == 0 {
        return;
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let payload = serde_json::json!({
        "hypothesisId": hypothesis_id,
        "location": location,
        "message": message,
        "data": {
            "seed": DST_SEED.load(Ordering::Relaxed),
            "runId": run_id,
            "threadFutureId": DST_TF_ID.load(Ordering::Relaxed),
            "eventSeq": DST_EVENT_SEQ.fetch_add(1, Ordering::Relaxed),
            "details": data,
        },
        "timestamp": timestamp,
    });
    let _guard = DST_LOG_LOCK.lock().expect("dst log lock poisoned");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/opt/cursor/logs/debug.log")
    {
        let _ = writeln!(f, "{payload}");
    }
}
// #endregion

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
            state: Arc::new(Mutex::new(TestRuntimeState { rng, creation_time })),
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
}

#[derive(Clone)]
pub struct TestRuntime {
    tokio_handle: tokio::runtime::Handle,
    state: Weak<Mutex<TestRuntimeState>>,
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

impl RngCore for TestRng {
    fn next_u32(&mut self) -> u32 {
        self.rt.with_state(|state| state.rng.next_u32())
    }

    fn next_u64(&mut self) -> u64 {
        self.rt.with_state(|state| state.rng.next_u64())
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
