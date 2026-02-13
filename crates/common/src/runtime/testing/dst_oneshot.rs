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
        atomic::{
            AtomicBool,
            Ordering,
        },
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
    recv_waker: Mutex<Option<Waker>>,
    sender_closed_waker: Mutex<Option<Waker>>,
    sender_dropped: AtomicBool,
    receiver_dropped: AtomicBool,
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
        recv_waker: Mutex::new(None),
        sender_closed_waker: Mutex::new(None),
        sender_dropped: AtomicBool::new(false),
        receiver_dropped: AtomicBool::new(false),
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
        if self.shared.receiver_dropped.load(Ordering::SeqCst) {
            return Err(value);
        }
        {
            let mut slot = self.shared.value.lock().unwrap();
            *slot = Some(value);
        }
        if let Some(waker) = self.shared.recv_waker.lock().unwrap().take() {
            defer_waker_to_tokio_thread(waker);
        }
        Ok(())
    }

    pub fn closed(&mut self) -> DstSenderClosed<T> {
        DstSenderClosed {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for DstOneshotSender<T> {
    fn drop(&mut self) {
        self.shared.sender_dropped.store(true, Ordering::SeqCst);
        if let Some(waker) = self.shared.recv_waker.lock().unwrap().take() {
            defer_waker_to_tokio_thread(waker);
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
        if self.shared.sender_dropped.load(Ordering::SeqCst) {
            return Poll::Ready(Err(DstRecvError));
        }
        // Store the waker for the sender to fire later.
        *self.shared.recv_waker.lock().unwrap() = Some(cx.waker().clone());
        // Double-check after storing the waker (avoids lost-wakeup race).
        if let Some(value) = self.shared.value.lock().unwrap().take() {
            return Poll::Ready(Ok(value));
        }
        if self.shared.sender_dropped.load(Ordering::SeqCst) {
            return Poll::Ready(Err(DstRecvError));
        }
        Poll::Pending
    }
}

impl<T> Drop for DstOneshotReceiver<T> {
    fn drop(&mut self) {
        self.shared.receiver_dropped.store(true, Ordering::SeqCst);
        if let Some(waker) = self.shared.sender_closed_waker.lock().unwrap().take() {
            defer_waker_to_tokio_thread(waker);
        }
    }
}

pub struct DstSenderClosed<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Future for DstSenderClosed<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.shared.receiver_dropped.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }
        *self.shared.sender_closed_waker.lock().unwrap() = Some(cx.waker().clone());
        if self.shared.receiver_dropped.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }
        Poll::Pending
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
