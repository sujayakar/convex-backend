use std::{
    cell::RefCell,
    pin::Pin,
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
    static THREAD_FUTURE_ID: RefCell<Option<u64>> = const { RefCell::new(None) };
}

/// Defer a waker fire so it happens on the Tokio runtime thread instead
/// of the current OS thread.  If called outside a `ThreadFuture` context,
/// the waker is fired immediately (production runtime path).
pub fn defer_waker_to_tokio_thread(waker: Waker) -> bool {
    DEFERRED_WAKER_TX.with(|tx| {
        let tx = tx.borrow();
        let thread_future_id = THREAD_FUTURE_ID.with(|id| *id.borrow());
        if let Some(tx) = tx.as_ref() {
            let send_ok = tx.send(waker).is_ok();
            // #region agent log
            super::dst_log_event(
                "crates/common/src/runtime/testing/thread_future.rs:39",
                "defer_waker_to_tokio_thread",
                serde_json::json!({
                    "thread_future_id": thread_future_id,
                    "deferred": true,
                    "send_ok": send_ok,
                }),
            );
            // #endregion
            send_ok
        } else {
            // Not inside a ThreadFuture — fire immediately.
            let tokio_wake_calls = super::note_tokio_thread_wake_call();
            // #region agent log
            super::dst_log_event(
                "crates/common/src/runtime/testing/thread_future.rs:52",
                "defer_waker_to_tokio_thread",
                serde_json::json!({
                    "thread_future_id": thread_future_id,
                    "deferred": false,
                    "tokio_wake_calls": tokio_wake_calls,
                }),
            );
            // #endregion
            waker.wake();
            false
        }
    })
}

pub struct ThreadFuture {
    id: u64,
    std_handle: Option<std::thread::JoinHandle<()>>,
    poll_request_tx: Option<crossbeam_channel::Sender<Waker>>,
    poll_response_rx: crossbeam_channel::Receiver<Poll<bool>>,
    /// Receives wakers that the OS thread deferred via
    /// [`defer_waker_to_tokio_thread`].  Drained and fired on the Tokio
    /// thread after each poll response.
    deferred_waker_rx: crossbeam_channel::Receiver<Waker>,
    poll_calls: u64,
}

impl ThreadFuture {
    pub(crate) fn new<Fut: Future<Output = ()>, F: FnOnce() -> Fut + Send + 'static>(
        tokio_handle: tokio::runtime::Handle,
        f: F,
    ) -> Self {
        let id = super::next_thread_future_id();
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
                THREAD_FUTURE_ID.with(|slot| {
                    *slot.borrow_mut() = Some(id);
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
            poll_calls: 0,
        }
    }
}

impl Future for ThreadFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.poll_calls += 1;
        // #region agent log
        super::dst_log_event(
            "crates/common/src/runtime/testing/thread_future.rs:144",
            "thread_future_poll_enter",
            serde_json::json!({
                "thread_future_id": this.id,
                "poll_call": this.poll_calls,
            }),
        );
        // #endregion

        // Forward the poll request to the thread.
        if this
            .poll_request_tx
            .as_ref()
            .expect("poll() called after drop?")
            .send(cx.waker().clone())
            .is_err()
        {
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
        let response_kind = match &response {
            Poll::Ready(was_canceled) => format!("ready({was_canceled})"),
            Poll::Pending => "pending".to_owned(),
        };
        // #region agent log
        super::dst_log_event(
            "crates/common/src/runtime/testing/thread_future.rs:175",
            "thread_future_poll_response",
            serde_json::json!({
                "thread_future_id": this.id,
                "poll_call": this.poll_calls,
                "response": response_kind,
            }),
        );
        // #endregion

        // Fire any wakers that the OS thread deferred via
        // `defer_waker_to_tokio_thread`.  Because we're on the Tokio
        // runtime thread, `Waker::wake()` routes through
        // `Schedule::schedule` → local queue (deterministic), instead of
        // the injection queue (non-deterministic).
        while let Ok(waker) = this.deferred_waker_rx.try_recv() {
            let deferred_waker_fires = super::note_deferred_waker_fire();
            let tokio_wake_calls = super::note_tokio_thread_wake_call();
            // #region agent log
            super::dst_log_event(
                "crates/common/src/runtime/testing/thread_future.rs:196",
                "thread_future_deferred_waker_fire",
                serde_json::json!({
                    "thread_future_id": this.id,
                    "poll_call": this.poll_calls,
                    "tokio_wake_calls": tokio_wake_calls,
                    "deferred_waker_fires": deferred_waker_fires,
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
