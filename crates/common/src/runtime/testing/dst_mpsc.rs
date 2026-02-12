//! An MPSC channel whose sender defers the receiver's waker through
//! [`defer_waker_to_tokio_thread`] instead of firing it directly.
//!
//! Same principle as [`DstOneshot`]: prevents wakers from entering Tokio's
//! injection queue when `send/try_send` is called from a V8 worker OS thread.

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

use futures::{
    Stream,
    stream::FusedStream,
};

use super::defer_waker_to_tokio_thread;

struct Shared<T> {
    queue: Mutex<std::collections::VecDeque<T>>,
    waker: Mutex<Option<Waker>>,
    /// Maximum capacity (0 = unbounded).
    capacity: usize,
    closed: Mutex<bool>,
}

pub struct DstMpscSender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for DstMpscSender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

pub struct DstMpscReceiver<T> {
    shared: Arc<Shared<T>>,
}

#[derive(Debug)]
pub enum TrySendError<T> {
    Full(T),
    Closed(T),
}

impl<T> std::fmt::Display for TrySendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrySendError::Full(_) => write!(f, "channel full"),
            TrySendError::Closed(_) => write!(f, "channel closed"),
        }
    }
}

pub fn dst_mpsc_channel<T>(capacity: usize) -> (DstMpscSender<T>, DstMpscReceiver<T>) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(std::collections::VecDeque::with_capacity(capacity)),
        waker: Mutex::new(None),
        capacity,
        closed: Mutex::new(false),
    });
    (
        DstMpscSender {
            shared: shared.clone(),
        },
        DstMpscReceiver { shared },
    )
}

impl<T> DstMpscSender<T> {
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut queue = self.shared.queue.lock().unwrap();
        if *self.shared.closed.lock().unwrap() {
            return Err(TrySendError::Closed(value));
        }
        if self.shared.capacity > 0 && queue.len() >= self.shared.capacity {
            return Err(TrySendError::Full(value));
        }
        queue.push_back(value);
        drop(queue);
        if let Some(waker) = self.shared.waker.lock().unwrap().take() {
            defer_waker_to_tokio_thread(waker);
        }
        Ok(())
    }
}

impl<T> Stream for DstMpscReceiver<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut queue = self.shared.queue.lock().unwrap();
        if let Some(value) = queue.pop_front() {
            return Poll::Ready(Some(value));
        }
        if *self.shared.closed.lock().unwrap() {
            return Poll::Ready(None);
        }
        *self.shared.waker.lock().unwrap() = Some(cx.waker().clone());
        drop(queue);
        // Double-check after storing waker.
        let mut queue = self.shared.queue.lock().unwrap();
        if let Some(value) = queue.pop_front() {
            return Poll::Ready(Some(value));
        }
        if *self.shared.closed.lock().unwrap() {
            return Poll::Ready(None);
        }
        Poll::Pending
    }
}

impl<T> FusedStream for DstMpscReceiver<T> {
    fn is_terminated(&self) -> bool {
        let queue = self.shared.queue.lock().unwrap();
        queue.is_empty() && *self.shared.closed.lock().unwrap()
    }
}

impl<T> DstMpscReceiver<T> {
    /// Receive the next message, or `None` if all senders have been dropped.
    pub async fn recv(&mut self) -> Option<T> {
        use futures::StreamExt;
        self.next().await
    }
}

impl<T> Drop for DstMpscSender<T> {
    fn drop(&mut self) {
        // Only close if this is the last sender.
        if Arc::strong_count(&self.shared) <= 2 {
            *self.shared.closed.lock().unwrap() = true;
            if let Some(waker) = self.shared.waker.lock().unwrap().take() {
                waker.wake();
            }
        }
    }
}
