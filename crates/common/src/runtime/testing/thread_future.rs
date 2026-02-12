use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    pin::Pin,
    sync::{
        atomic::{
            AtomicUsize,
            Ordering,
        },
        LazyLock,
        Mutex,
    },
    task::{
        Context,
        Poll,
        Waker,
    },
};

use futures::{
    Future,
    FutureExt,
};

use crate::knobs::RUNTIME_STACK_SIZE;

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

static NEXT_THREAD_FUTURE_ID: AtomicUsize = AtomicUsize::new(1);
static THREAD_FUTURE_POLL_COUNTS: LazyLock<Mutex<BTreeMap<usize, usize>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn nitpick_run_id() -> Option<usize> {
    let run_id = std::env::var("NITPICK_RUN_ID").ok()?;
    run_id.parse::<usize>().ok().filter(|id| *id > 0)
}

fn tf_log_path() -> Option<String> {
    let run_id = nitpick_run_id()?;
    Some(format!("/tmp/nitpick_tf_run{run_id}.log"))
}

fn append_tf_log(line: &str) {
    let Some(path) = tf_log_path() else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

pub fn reset_thread_future_poll_tracking() {
    NEXT_THREAD_FUTURE_ID.store(1, Ordering::Relaxed);
    THREAD_FUTURE_POLL_COUNTS
        .lock()
        .expect("thread future poll map lock poisoned")
        .clear();
    if let Some(path) = tf_log_path() {
        let _ = std::fs::remove_file(path);
    }
}

pub(crate) fn write_thread_future_poll_summary() {
    let Some(run_id) = nitpick_run_id() else {
        return;
    };
    let snapshot = THREAD_FUTURE_POLL_COUNTS
        .lock()
        .expect("thread future poll map lock poisoned")
        .clone();
    // #region agent log
    append_tf_log(&format!(
        "THREAD_FUTURE_SUMMARY_BEGIN run_id={} total_thread_futures={}",
        run_id,
        snapshot.len()
    ));
    // #endregion
    for (thread_future_id, poll_count) in snapshot {
        // #region agent log
        append_tf_log(&format!(
            "THREAD_FUTURE_SUMMARY id={} total_polls={}",
            thread_future_id, poll_count
        ));
        // #endregion
    }
    // #region agent log
    append_tf_log(&format!("THREAD_FUTURE_SUMMARY_END run_id={run_id}"));
    // #endregion
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

pub struct ThreadFuture {
    id: usize,
    std_handle: Option<std::thread::JoinHandle<()>>,
    poll_request_tx: Option<crossbeam_channel::Sender<Waker>>,
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
        let id = NEXT_THREAD_FUTURE_ID.fetch_add(1, Ordering::Relaxed);
        THREAD_FUTURE_POLL_COUNTS
            .lock()
            .expect("thread future poll map lock poisoned")
            .insert(id, 0);
        // #region agent log
        append_tf_log(&format!("THREAD_FUTURE_NEW id={id}"));
        // #endregion
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
                    let Ok(waker) = poll_request_rx.recv() else {
                        tracing::debug!("ThreadFuture gone away, returning.");
                        return;
                    };
                    let mut cx = Context::from_waker(&waker);
                    let response = match fut.poll_unpin(&mut cx) {
                        Poll::Ready(()) => Poll::Ready(false),
                        Poll::Pending => Poll::Pending,
                    };
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
        let poll_number = {
            let mut map = THREAD_FUTURE_POLL_COUNTS
                .lock()
                .expect("thread future poll map lock poisoned");
            let entry = map.entry(this.id).or_insert(0);
            *entry += 1;
            *entry
        };

        // Forward the poll request to the thread.
        if this
            .poll_request_tx
            .as_ref()
            .expect("poll() called after drop?")
            .send(cx.waker().clone())
            .is_err()
        {
            tracing::error!("ThreadFuture worker thread terminated.");
            // #region agent log
            append_tf_log(&format!(
                "THREAD_FUTURE_POLL id={} poll={} response=ReadyWorkerTerminatedOnSend",
                this.id, poll_number
            ));
            // #endregion
            return Poll::Ready(());
        }
        let response = match this.poll_response_rx.recv() {
            Ok(response) => response,
            Err(..) => {
                tracing::error!("ThreadFuture worker thread terminated.");
                // #region agent log
                append_tf_log(&format!(
                    "THREAD_FUTURE_POLL id={} poll={} response=ReadyWorkerTerminatedOnRecv",
                    this.id, poll_number
                ));
                // #endregion
                return Poll::Ready(());
            },
        };

        // Fire any wakers that the OS thread deferred via
        // `defer_waker_to_tokio_thread`.  Because we're on the Tokio
        // runtime thread, `Waker::wake()` routes through
        // `Schedule::schedule` → local queue (deterministic), instead of
        // the injection queue (non-deterministic).
        while let Ok(waker) = this.deferred_waker_rx.try_recv() {
            waker.wake();
        }

        match response {
            Poll::Ready(was_canceled) => {
                // #region agent log
                append_tf_log(&format!(
                    "THREAD_FUTURE_POLL id={} poll={} response=Ready was_canceled={}",
                    this.id, poll_number, was_canceled
                ));
                // #endregion
                tracing::debug!(
                    "ThreadFuture completed (was_canceled: {was_canceled}), returning."
                );
                Poll::Ready(())
            },
            Poll::Pending => {
                // #region agent log
                append_tf_log(&format!(
                    "THREAD_FUTURE_POLL id={} poll={} response=Pending",
                    this.id, poll_number
                ));
                // #endregion
                Poll::Pending
            },
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
