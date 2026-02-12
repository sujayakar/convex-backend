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
        let mut wake_path = "none";
        if let Some(waker) = self.shared.waker.lock().unwrap().take() {
            wake_path = if defer_waker_to_tokio_thread(waker) {
                "deferred"
            } else {
                "direct"
            };
        }
        let (tokio_wake_calls, deferred_waker_fires) = super::waker_counters();
        // #region agent log
        super::dst_log_event(
            "crates/common/src/runtime/testing/dst_oneshot.rs:68",
            "dst_oneshot_send",
            serde_json::json!({
                "wake_path": wake_path,
                "tokio_wake_calls": tokio_wake_calls,
                "deferred_waker_fires": deferred_waker_fires,
            }),
        );
        // #endregion
        Ok(())
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

#[derive(Debug)]
pub struct DstRecvError;

impl std::fmt::Display for DstRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DstOneshot sender dropped without sending")
    }
}

impl std::error::Error for DstRecvError {}
