use std::{
    cell::RefCell,
    pin::Pin,
    sync::Arc,
    task::{
        Context,
        Poll,
        RawWaker,
        RawWakerVTable,
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

/// Create a waker that, when woken, defers the real waker through the
/// `DEFERRED_WAKER_TX` channel.  This intercepts ALL waker fires from
/// the OS thread — including those from `tokio::sync` primitives like
/// `mpsc::Sender::send()` and `oneshot::Sender::send()` — routing them
/// to the Tokio runtime thread's deterministic local queue instead of
/// the non-deterministic injection queue.
fn make_deferring_waker(real_waker: Waker) -> Waker {
    // We box the real waker and pass it through the RawWaker data pointer.
    let data = Arc::new(real_waker);
    let raw = RawWaker::new(
        Arc::into_raw(data) as *const (),
        &DEFERRING_VTABLE,
    );
    // SAFETY: raw waker follows the contract — clone increments refcount,
    // wake/wake_by_ref defer the inner waker, drop decrements refcount.
    unsafe { Waker::from_raw(raw) }
}

const DEFERRING_VTABLE: RawWakerVTable = RawWakerVTable::new(
    deferring_clone,
    deferring_wake,
    deferring_wake_by_ref,
    deferring_drop,
);

unsafe fn deferring_clone(data: *const ()) -> RawWaker {
    let arc = unsafe { Arc::from_raw(data as *const Waker) };
    let cloned = arc.clone();
    std::mem::forget(arc); // Don't decrement the original's refcount.
    RawWaker::new(
        Arc::into_raw(cloned) as *const (),
        &DEFERRING_VTABLE,
    )
}

unsafe fn deferring_wake(data: *const ()) {
    let arc = unsafe { Arc::from_raw(data as *const Waker) };
    defer_waker_to_tokio_thread((*arc).clone());
    // arc is dropped here, decrementing the refcount.
}

unsafe fn deferring_wake_by_ref(data: *const ()) {
    let arc = unsafe { Arc::from_raw(data as *const Waker) };
    defer_waker_to_tokio_thread((*arc).clone());
    std::mem::forget(arc); // Don't decrement the refcount.
}

unsafe fn deferring_drop(data: *const ()) {
    let _arc = unsafe { Arc::from_raw(data as *const Waker) };
    // arc is dropped here, decrementing the refcount.
}

pub struct ThreadFuture {
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
                    let Ok(real_waker) = poll_request_rx.recv() else {
                        tracing::debug!("ThreadFuture gone away, returning.");
                        return;
                    };
                    // Wrap the real waker so that ANY waker.wake() call
                    // from this poll (including tokio::sync internals)
                    // goes through the deferred channel.
                    let deferring_waker = make_deferring_waker(real_waker);
                    let mut cx = Context::from_waker(&deferring_waker);
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

        // Fire any wakers that the OS thread deferred via
        // `defer_waker_to_tokio_thread` or through the deferring waker.
        // Because we're on the Tokio runtime thread,
        // `Waker::wake()` routes through `Schedule::schedule` →
        // local queue (deterministic), instead of the injection queue
        // (non-deterministic).
        while let Ok(waker) = this.deferred_waker_rx.try_recv() {
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
