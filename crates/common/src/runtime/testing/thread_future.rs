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

thread_local! {
    static DEFERRED_WAKER_TX: RefCell<Option<crossbeam_channel::Sender<Waker>>> =
        const { RefCell::new(None) };
    /// Channel for deferring task spawns from the OS thread.
    static DEFERRED_SPAWN_TX: RefCell<Option<crossbeam_channel::Sender<Pin<Box<dyn Future<Output = ()> + Send>>>>> =
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
            waker.wake();
        }
    });
}

/// Returns true if the current thread is inside a `ThreadFuture` context.
pub fn is_inside_thread_future() -> bool {
    DEFERRED_WAKER_TX.with(|tx| tx.borrow().is_some())
}

/// Enqueue a future to be spawned on the Tokio thread during the next
/// `ThreadFuture::poll`.  Returns `true` if the enqueue succeeded
/// (inside a ThreadFuture), `false` otherwise (caller should spawn
/// directly).
pub fn enqueue_deferred_spawn(f: Pin<Box<dyn Future<Output = ()> + Send>>) -> bool {
    DEFERRED_SPAWN_TX.with(|tx| {
        let tx = tx.borrow();
        if let Some(tx) = tx.as_ref() {
            let _ = tx.send(f);
            true
        } else {
            false
        }
    })
}

pub struct ThreadFuture {
    std_handle: Option<std::thread::JoinHandle<()>>,
    poll_request_tx: Option<crossbeam_channel::Sender<Waker>>,
    poll_response_rx: crossbeam_channel::Receiver<Poll<bool>>,
    deferred_waker_rx: crossbeam_channel::Receiver<Waker>,
    deferred_spawn_rx: crossbeam_channel::Receiver<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl ThreadFuture {
    pub(crate) fn new<Fut: Future<Output = ()>, F: FnOnce() -> Fut + Send + 'static>(
        tokio_handle: tokio::runtime::Handle,
        f: F,
    ) -> Self {
        let (poll_request_tx, poll_request_rx) = crossbeam_channel::bounded(1);
        let (poll_response_tx, poll_response_rx) = crossbeam_channel::bounded(1);
        let (deferred_waker_tx, deferred_waker_rx) = crossbeam_channel::unbounded();
        let (deferred_spawn_tx, deferred_spawn_rx) = crossbeam_channel::unbounded();
        let std_handle = std::thread::Builder::new()
            .stack_size(*RUNTIME_STACK_SIZE)
            .spawn(move || {
                let _guard = tokio_handle.enter();
                DEFERRED_WAKER_TX.with(|tx| {
                    *tx.borrow_mut() = Some(deferred_waker_tx);
                });
                DEFERRED_SPAWN_TX.with(|tx| {
                    *tx.borrow_mut() = Some(deferred_spawn_tx);
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
            std_handle: Some(std_handle),
            poll_request_tx: Some(poll_request_tx),
            poll_response_rx,
            deferred_waker_rx,
            deferred_spawn_rx,
        }
    }
}

impl Future for ThreadFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

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

        // Spawn deferred tasks on the Tokio thread (local queue).
        while let Ok(future) = this.deferred_spawn_rx.try_recv() {
            tokio::task::spawn(future);
        }

        // Fire deferred wakers on the Tokio thread (local queue).
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
