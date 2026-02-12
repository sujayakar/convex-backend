//! A oneshot channel whose sender defers the receiver's waker through
//! [`defer_waker_to_tokio_thread`] instead of firing it directly.
//!
//! This is used in deterministic simulation testing to prevent wakers from
//! going through Tokio's cross-thread injection queue (which has
//! non-deterministic drain timing).  By deferring the waker, the
//! `ThreadFuture` poll method fires it on the Tokio runtime thread,
//! routing it to the deterministic local queue.

use std::{
    pin::Pin,
    sync::{
        Arc,
        Mutex,
    },
    task::{
        Context,
        Poll,
        Waker,
    },
};

use futures::Future;

use super::defer_waker_to_tokio_thread;

struct Shared<T> {
    value: Mutex<Option<T>>,
    waker: Mutex<Option<Waker>>,
    /// Waker to fire when the receiver is dropped (for `closed()`).
    closed_waker: Mutex<Option<Waker>>,
    /// Set to true when the receiver is dropped.
    receiver_dropped: Mutex<bool>,
}

pub struct DstOneshotSender<T> {
    shared: Arc<Shared<T>>,
}

pub struct DstOneshotReceiver<T> {
    shared: Arc<Shared<T>>,
}

pub fn dst_oneshot_channel<T>() -> (DstOneshotSender<T>, DstOneshotReceiver<T>) {
    let shared = Arc::new(Shared {
        value: Mutex::new(None),
        waker: Mutex::new(None),
        closed_waker: Mutex::new(None),
        receiver_dropped: Mutex::new(false),
    });
    (
        DstOneshotSender {
            shared: shared.clone(),
        },
        DstOneshotReceiver { shared },
    )
}

impl<T> DstOneshotSender<T> {
    pub fn send(self, value: T) -> Result<(), T> {
        {
            let mut slot = self.shared.value.lock().unwrap();
            *slot = Some(value);
        }
        if let Some(waker) = self.shared.waker.lock().unwrap().take() {
            defer_waker_to_tokio_thread(waker);
        }
        Ok(())
    }

    /// Returns a future that resolves when the receiver half is dropped.
    /// Used as a cancellation signal.
    pub fn closed(&mut self) -> DstOneshotClosed<T> {
        DstOneshotClosed {
            shared: self.shared.clone(),
        }
    }
}

/// Future that completes when the `DstOneshotReceiver` is dropped.
pub struct DstOneshotClosed<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Future for DstOneshotClosed<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if *self.shared.receiver_dropped.lock().unwrap() {
            Poll::Ready(())
        } else {
            *self.shared.closed_waker.lock().unwrap() = Some(cx.waker().clone());
            // Double-check after storing waker.
            if *self.shared.receiver_dropped.lock().unwrap() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }
}

impl<T> Future for DstOneshotReceiver<T> {
    type Output = Result<T, DstRecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Check if the value is already available.
        if let Some(value) = self.shared.value.lock().unwrap().take() {
            return Poll::Ready(Ok(value));
        }
        // Store the waker for the sender to fire later.
        *self.shared.waker.lock().unwrap() = Some(cx.waker().clone());
        // Double-check after storing the waker (avoids lost-wakeup race).
        if let Some(value) = self.shared.value.lock().unwrap().take() {
            return Poll::Ready(Ok(value));
        }
        Poll::Pending
    }
}

impl<T> Drop for DstOneshotReceiver<T> {
    fn drop(&mut self) {
        *self.shared.receiver_dropped.lock().unwrap() = true;
        if let Some(waker) = self.shared.closed_waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

#[derive(Debug)]
pub struct DstRecvError;

impl std::fmt::Display for DstRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DstOneshot sender dropped without sending")
    }
}

impl std::error::Error for DstRecvError {}
