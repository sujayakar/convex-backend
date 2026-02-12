use std::{
    cell::RefCell,
    fs::OpenOptions,
    io::Write,
    pin::Pin,
    sync::{
        atomic::{
            AtomicU64,
            Ordering,
        },
        Arc,
    },
    task::{
        Context,
        Poll,
        Wake,
        Waker,
    },
    time::{
        SystemTime,
        UNIX_EPOCH,
    },
};

use futures::{
    Future,
    FutureExt,
};
use serde_json::json;

use crate::knobs::RUNTIME_STACK_SIZE;

const WAKER_TRACE_LOG_PATH: &str = "/tmp/nitpick_waker_trace.log";
static THREAD_FUTURE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static TOKIO_POLL_SEQ_COUNTER: AtomicU64 = AtomicU64::new(1);
static WAKER_FIRE_SEQ_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_thread_future_id() -> u64 {
    THREAD_FUTURE_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn next_tokio_poll_seq() -> u64 {
    TOKIO_POLL_SEQ_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn next_waker_fire_seq() -> u64 {
    WAKER_FIRE_SEQ_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn current_poll_count() -> Option<u64> {
    tokio::runtime::Handle::try_current()
        .ok()
        .map(|handle| handle.metrics().worker_poll_count(0))
}

fn append_waker_trace(
    hypothesis_id: &'static str,
    location: &'static str,
    message: &'static str,
    data: serde_json::Value,
) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    let line = json!({
        "hypothesisId": hypothesis_id,
        "timestamp": timestamp,
        "location": location,
        "message": message,
        "thread_id": format!("{:?}", std::thread::current().id()),
        "poll_count": current_poll_count(),
        "data": data,
    });
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(WAKER_TRACE_LOG_PATH)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Channel for deferring waker fires from the OS thread to the Tokio
/// thread.  Code running inside a `ThreadFuture` poll can call
/// [`defer_waker_to_tokio_thread`] to enqueue a waker; it will be fired
/// on the Tokio runtime thread after the poll completes, ensuring
/// deterministic local-queue scheduling instead of non-deterministic
/// injection-queue scheduling.
thread_local! {
    static DEFERRED_WAKER_TX: RefCell<Option<crossbeam_channel::Sender<Waker>>> =
        const { RefCell::new(None) };
}

/// Defer a waker fire so it happens on the Tokio runtime thread instead
/// of the current OS thread.  If called outside a `ThreadFuture` context,
/// the waker is fired immediately (production runtime path).
pub fn defer_waker_to_tokio_thread(waker: Waker) {
    DEFERRED_WAKER_TX.with(|tx| {
        let tx = tx.borrow();
        if let Some(tx) = tx.as_ref() {
            let _ = tx.send(waker);
        } else {
            // Not inside a ThreadFuture — fire immediately.
            waker.wake();
        }
    });
}

struct PollRequest {
    waker: Waker,
    poll_seq: u64,
    tokio_thread_id: String,
}

struct TracedThreadFutureWaker {
    thread_future_id: u64,
    poll_seq: u64,
    tokio_thread_id: String,
    inner: Waker,
}

impl TracedThreadFutureWaker {
    fn log_fire(&self, method: &'static str) {
        let wake_seq = next_waker_fire_seq();
        let wake_thread_id = format!("{:?}", std::thread::current().id());
        // #region agent log
        append_waker_trace(
            "A",
            "crates/common/src/runtime/testing/thread_future.rs:117",
            "thread_future_waker_fire",
            json!({
                "wake_seq": wake_seq,
                "method": method,
                "thread_future_id": self.thread_future_id,
                "poll_seq": self.poll_seq,
                "wake_thread_id": wake_thread_id,
                "tokio_thread_id": self.tokio_thread_id.clone(),
                "is_tokio_thread": wake_thread_id == self.tokio_thread_id,
            }),
        );
        // #endregion
    }
}

impl Wake for TracedThreadFutureWaker {
    fn wake(self: Arc<Self>) {
        self.log_fire("wake");
        self.inner.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.log_fire("wake_by_ref");
        self.inner.wake_by_ref();
    }
}

pub struct ThreadFuture {
    id: u64,
    poll_calls: u64,
    std_handle: Option<std::thread::JoinHandle<()>>,
    poll_request_tx: Option<crossbeam_channel::Sender<PollRequest>>,
    poll_response_rx: crossbeam_channel::Receiver<Poll<bool>>,
    /// Receives wakers that the OS thread deferred via
    /// [`defer_waker_to_tokio_thread`].  Drained and fired on the Tokio
    /// thread after each poll response.
    deferred_waker_rx: crossbeam_channel::Receiver<Waker>,
}

impl ThreadFuture {
    pub(crate) fn new<Fut: Future<Output = ()>, F: FnOnce() -> Fut + Send + 'static>(
        tokio_handle: tokio::runtime::Handle,
        f: F,
    ) -> Self {
        let id = next_thread_future_id();
        let (poll_request_tx, poll_request_rx) = crossbeam_channel::bounded(1);
        let (poll_response_tx, poll_response_rx) = crossbeam_channel::bounded(1);
        let (deferred_waker_tx, deferred_waker_rx) = crossbeam_channel::unbounded();
        let std_handle = std::thread::Builder::new()
            .stack_size(*RUNTIME_STACK_SIZE)
            .spawn(move || {
                let _guard = tokio_handle.enter();
                // Install the deferred-waker channel for this thread.
                DEFERRED_WAKER_TX.with(|tx| {
                    *tx.borrow_mut() = Some(deferred_waker_tx);
                });
                let fut = f();
                tokio::pin!(fut);
                loop {
                    let Ok(poll_request) = poll_request_rx.recv() else {
                        tracing::debug!("ThreadFuture gone away, returning.");
                        return;
                    };
                    let PollRequest {
                        waker,
                        poll_seq,
                        tokio_thread_id,
                    } = poll_request;
                    // #region agent log
                    append_waker_trace(
                        "B",
                        "crates/common/src/runtime/testing/thread_future.rs:177",
                        "thread_future_inner_poll_start",
                        json!({
                            "thread_future_id": id,
                            "poll_seq": poll_seq,
                            "tokio_thread_id": tokio_thread_id.clone(),
                        }),
                    );
                    // #endregion
                    let traced_waker = Waker::from(Arc::new(TracedThreadFutureWaker {
                        thread_future_id: id,
                        poll_seq,
                        tokio_thread_id: tokio_thread_id.clone(),
                        inner: waker,
                    }));
                    let mut cx = Context::from_waker(&traced_waker);
                    let response = match fut.poll_unpin(&mut cx) {
                        Poll::Ready(()) => Poll::Ready(false),
                        Poll::Pending => Poll::Pending,
                    };
                    // #region agent log
                    append_waker_trace(
                        "B",
                        "crates/common/src/runtime/testing/thread_future.rs:200",
                        "thread_future_inner_poll_end",
                        json!({
                            "thread_future_id": id,
                            "poll_seq": poll_seq,
                            "response": if response.is_ready() { "ready" } else { "pending" },
                        }),
                    );
                    // #endregion
                    poll_response_tx
                        .send(response)
                        .expect("TestRuntime went away without waiting for a poll response");
                    if response.is_ready() {
                        tracing::debug!("ThreadFuture polled ready, returning.");
                        return;
                    }
                }
            })
            .expect("Failed to start new thread");
        Self {
            id,
            poll_calls: 0,
            std_handle: Some(std_handle),
            poll_request_tx: Some(poll_request_tx),
            poll_response_rx,
            deferred_waker_rx,
        }
    }
}

impl Future for ThreadFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.poll_calls += 1;
        let poll_seq = next_tokio_poll_seq();
        let tokio_thread_id = format!("{:?}", std::thread::current().id());
        // #region agent log
        append_waker_trace(
            "B",
            "crates/common/src/runtime/testing/thread_future.rs:236",
            "thread_future_tokio_poll_enter",
            json!({
                "thread_future_id": this.id,
                "poll_seq": poll_seq,
                "poll_call": this.poll_calls,
                "tokio_thread_id": tokio_thread_id,
            }),
        );
        // #endregion

        // Forward the poll request to the thread.
        if this
            .poll_request_tx
            .as_ref()
            .expect("poll() called after drop?")
            .send(PollRequest {
                waker: cx.waker().clone(),
                poll_seq,
                tokio_thread_id: tokio_thread_id.clone(),
            })
            .is_err()
        {
            // #region agent log
            append_waker_trace(
                "B",
                "crates/common/src/runtime/testing/thread_future.rs:261",
                "thread_future_poll_send_failed",
                json!({
                    "thread_future_id": this.id,
                    "poll_seq": poll_seq,
                    "poll_call": this.poll_calls,
                }),
            );
            // #endregion
            tracing::error!("ThreadFuture worker thread terminated.");
            return Poll::Ready(());
        }
        let response = match this.poll_response_rx.recv() {
            Ok(response) => response,
            Err(..) => {
                tracing::error!("ThreadFuture worker thread terminated.");
                return Poll::Ready(());
            },
        };
        // #region agent log
        append_waker_trace(
            "B",
            "crates/common/src/runtime/testing/thread_future.rs:282",
            "thread_future_tokio_poll_response",
            json!({
                "thread_future_id": this.id,
                "poll_seq": poll_seq,
                "poll_call": this.poll_calls,
                "response": if response.is_ready() { "ready" } else { "pending" },
            }),
        );
        // #endregion

        // Fire any wakers that the OS thread deferred via
        // `defer_waker_to_tokio_thread`.  Because we're on the Tokio
        // runtime thread, `Waker::wake()` routes through
        // `Schedule::schedule` → local queue (deterministic), instead of
        // the injection queue (non-deterministic).
        while let Ok(waker) = this.deferred_waker_rx.try_recv() {
            // #region agent log
            append_waker_trace(
                "C",
                "crates/common/src/runtime/testing/thread_future.rs:302",
                "thread_future_deferred_waker_fire_tokio",
                json!({
                    "thread_future_id": this.id,
                    "poll_seq": poll_seq,
                    "poll_call": this.poll_calls,
                }),
            );
            // #endregion
            waker.wake();
        }

        match response {
            Poll::Ready(was_canceled) => {
                tracing::debug!(
                    "ThreadFuture completed (was_canceled: {was_canceled}), returning."
                );
                Poll::Ready(())
            },
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for ThreadFuture {
    fn drop(&mut self) {
        let Some(std_handle) = self.std_handle.take() else {
            return;
        };
        let Some(command_tx) = self.poll_request_tx.take() else {
            return;
        };
        if std_handle.is_finished() {
            return;
        }
        drop(command_tx);
        tracing::debug!("Waiting for worker thread to shutdown on drop.");
        let r = self.poll_response_rx.recv();
        tracing::debug!("Worker thread shutdown response: {r:?}");
        std_handle.join().expect("Worker thread panicked");
    }
}
