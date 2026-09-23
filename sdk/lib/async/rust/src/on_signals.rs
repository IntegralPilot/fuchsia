// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::callback_state::CallbackSharedState;
use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use core::task::{Context, Poll};
use futures::task::AtomicWaker;
use libasync_dispatcher::{DetectDispatcher, GetAsyncDispatcher};
use libasync_sys::{async_begin_wait, async_cancel_wait, async_dispatcher, async_wait};
use std::sync::Arc;
use zx::Status;
use zx::sys::{ZX_ERR_CANCELED, ZX_OK};

/// Internal state managed for a pending wait callback.
struct WaitState {
    async_dispatcher: NonNull<async_dispatcher>,
    waker: AtomicWaker,
    status: AtomicI32,
    observed: AtomicU32,
}

// SAFETY: It is safe to access an `async_dispatcher_t` from any thread per the libasync C api.
unsafe impl Send for WaitState {}
unsafe impl Sync for WaitState {}

type SharedState = CallbackSharedState<async_wait, WaitState>;

impl WaitState {
    extern "C" fn call(
        dispatcher: *mut async_dispatcher,
        wait: *mut async_wait,
        status: i32,
        signal: *const zx::sys::zx_packet_signal_t,
    ) {
        debug_assert!(
            status == ZX_OK || status == ZX_ERR_CANCELED,
            "wait callback called with status other than ok or canceled: {}",
            status
        );

        // SAFETY: We called make_raw_ptr when we began the wait.
        let state = unsafe { SharedState::from_raw_ptr(wait) };

        debug_assert!(
            dispatcher == state.async_dispatcher.as_ptr(),
            "dispatcher does not match wait's dispatcher"
        );

        if status == ZX_OK {
            debug_assert!(!signal.is_null(), "signal must not be null when status is ZX_OK");
            // SAFETY: signal is not null and valid when status is ZX_OK, as per the contract of
            // async_wait_handler_t.
            let observed_signals = unsafe { (*signal).observed };
            // Store `observed` first with Relaxed ordering because it is guarded by the
            // upcoming Release store on `status`.
            state.observed.store(observed_signals, Ordering::Relaxed);
        }

        // Store `status` using Release ordering, since we already stored `observed` first, loading
        // `status` in poll and cancel_wait with Acquire ensures `observed` updates are also seen
        // by the other thread.
        state.status.store(status, Ordering::Release);

        state.waker.wake();
    }
}

/// Implements methods for setting and waiting on signals on a dispatcher.
pub trait DispatcherSignalExt {
    /// Returns a future that will fire when the given signals are asserted on the handle.
    fn on_signals<H: zx::AsHandleRef>(&self, handle: H, signals: zx::Signals) -> OnSignals<H>;

    /// Returns a future that will fire when the given signals are asserted on the handle. Returns
    /// `None` if there is no dispatcher found on `self`.
    fn try_on_signals<H: zx::AsHandleRef>(
        &self,
        handle: H,
        signals: zx::Signals,
    ) -> Option<OnSignals<H>>;
}

impl<T> DispatcherSignalExt for T
where
    T: GetAsyncDispatcher,
{
    fn on_signals<H: zx::AsHandleRef>(&self, handle: H, signals: zx::Signals) -> OnSignals<H> {
        self.try_on_signals(handle, signals).expect("No current dispatcher")
    }

    fn try_on_signals<H: zx::AsHandleRef>(
        &self,
        handle: H,
        signals: zx::Signals,
    ) -> Option<OnSignals<H>> {
        let dispatcher = self.try_get_async_dispatcher()?;
        Some(OnSignals::new_on(dispatcher, handle, signals))
    }
}

/// A future that completes when specified signals are asserted on a Zircon handle.
pub struct OnSignals<H: zx::AsHandleRef> {
    dispatcher: DetectDispatcher,
    handle: Option<H>,
    signals: zx::Signals,
    state: Option<Arc<SharedState>>,
}

impl<H: zx::AsHandleRef> OnSignals<H> {
    /// Creates a new `OnSignals` object that will receive notifications when signals occur on
    /// `handle`.
    pub fn new(handle: H, signals: zx::Signals) -> Self {
        Self { dispatcher: DetectDispatcher::default(), handle: Some(handle), signals, state: None }
    }

    /// Creates a new `OnSignals` object bound to a specific dispatcher.
    pub fn new_on(dispatcher: impl GetAsyncDispatcher, handle: H, signals: zx::Signals) -> Self {
        let async_disp = dispatcher.get_async_dispatcher();
        Self {
            dispatcher: DetectDispatcher::new(async_disp),
            handle: Some(handle),
            signals,
            state: None,
        }
    }

    /// Takes the handle back, cancelling any active wait.
    pub fn take_handle(&mut self) -> Option<H> {
        self.cancel_wait();
        self.handle.take()
    }

    fn cancel_wait(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };

        // Load `status` using Acquire ordering to synchronize with `WaitState::call`'s Release
        // store.
        if state.status.load(Ordering::Acquire) != Status::SHOULD_WAIT.into_raw() {
            // Callback has already completed; nothing to cancel.
            return;
        }

        let state_ptr = SharedState::as_raw_ptr(&state);
        // SAFETY: async_cancel_wait is thread-safe per the C API doc.
        let status = unsafe { async_cancel_wait(state.async_dispatcher.as_ptr(), state_ptr) };
        if Status::ok(status).is_ok() {
            // SAFETY: Cancellation succeeded. The callback will not run, so we have to release the
            // raw pointer here.
            unsafe { SharedState::release_raw_ptr(state_ptr) };
        }
    }
}

impl<H: zx::AsHandleRef + Unpin> Future for OnSignals<H> {
    type Output = Result<zx::Signals, Status>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(handle) = self.handle.as_ref() else {
            return Poll::Ready(Err(Status::BAD_STATE));
        };

        if let Some(state) = &self.state {
            // Avoid a lost wake by registering the waker first. This prevents the case that the
            // callback running in parallel, stores `status` and wakes the previous waker,
            // after our check but before we register the waker.
            state.waker.register(cx.waker());

            // Use `Acquire` ordering to ensure that the store in `WaitState::call` is visible
            // here.
            let status = state.status.load(Ordering::Acquire);
            if status != Status::SHOULD_WAIT.into_raw() {
                if status == ZX_OK {
                    // We can use `Relaxed` ordering here since we used `Acquire` above, and on the
                    // store side we store in reverse.
                    let observed = state.observed.load(Ordering::Relaxed);
                    return Poll::Ready(Ok(zx::Signals::from_bits_truncate(observed)));
                } else {
                    return Poll::Ready(Err(Status::err_from_raw(status)));
                }
            } else {
                // Optimization: use a non-blocking wait to check if its ready. Alternatively we
                // could just wait for the callback but this way we don't have to wait for the
                // two thread hops.
                match handle
                    .as_handle_ref()
                    .wait_one(self.signals, zx::MonotonicInstant::INFINITE_PAST)
                    .to_result()
                {
                    Ok(signals) => {
                        self.cancel_wait();
                        return Poll::Ready(Ok(signals));
                    }
                    Err(Status::TIMED_OUT) => {
                        return Poll::Pending;
                    }
                    Err(err) => {
                        self.cancel_wait();
                        return Poll::Ready(Err(err));
                    }
                }
            }
        }

        // First poll. Check if its already signaled, if it is we can bypass the async wait setup.
        match handle
            .as_handle_ref()
            .wait_one(self.signals, zx::MonotonicInstant::INFINITE_PAST)
            .to_result()
        {
            Ok(signals) => return Poll::Ready(Ok(signals)),
            Err(Status::TIMED_OUT) => { /* Signal not yet asserted; proceed to register */ }
            Err(err) => return Poll::Ready(Err(err)),
        }

        let dispatcher = self.dispatcher.get_or_detect()?;
        let async_dispatcher = dispatcher.as_ptr();

        let wait = async_wait {
            handler: Some(WaitState::call),
            object: handle.as_handle_ref().raw_handle(),
            trigger: self.signals.bits(),
            options: 0,
            ..Default::default()
        };

        let state = WaitState {
            async_dispatcher,
            waker: AtomicWaker::new(),
            status: AtomicI32::new(Status::SHOULD_WAIT.into_raw()),
            observed: AtomicU32::new(0),
        };

        let shared_state = SharedState::new(wait, state);
        shared_state.waker.register(cx.waker());

        let state_ptr = SharedState::make_raw_ptr(shared_state.clone());

        // SAFETY: async_begin_wait is thread safe per the C API doc.
        let res = Status::ok(unsafe { async_begin_wait(async_dispatcher.as_ptr(), state_ptr) });
        match res {
            Ok(_) => {
                self.state = Some(shared_state);
                Poll::Pending
            }
            Err(err) => {
                // SAFETY: The C callback will not be called on the error case, so we must release
                // the raw pointer here.
                unsafe { SharedState::release_raw_ptr(state_ptr) };
                Poll::Ready(Err(err))
            }
        }
    }
}

impl<H: zx::AsHandleRef> Drop for OnSignals<H> {
    fn drop(&mut self) {
        self.cancel_wait();
    }
}

impl<H: zx::AsHandleRef> zx::AsHandleRef for OnSignals<H> {
    fn as_handle_ref(&self) -> zx::HandleRef<'_> {
        self.handle.as_ref().expect("OnSignals dereferenced after handle was taken").as_handle_ref()
    }
}

impl<H: zx::AsHandleRef> fmt::Debug for OnSignals<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OnSignals {{ handle: {:?}, signals: {:?} }}",
            self.handle.as_ref().map(|h| h.as_handle_ref()),
            self.signals
        )
    }
}

/// Alias for the common case where `OnSignals` is used with `zx::HandleRef`.
pub type OnSignalsRef<'a> = OnSignals<zx::HandleRef<'a>>;

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_env::test::spawn_in_driver;
    use futures::{FutureExt, poll};
    use libasync_dispatcher::CurrentDispatcher;
    use std::sync::{LazyLock, mpsc};
    use std::task::Waker;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn wait_on_signals() {
        spawn_in_driver("testing wait", async move {
            let event = zx::Event::create();
            let mut fut = CurrentDispatcher.on_signals(&event, zx::Signals::EVENT_SIGNALED);
            assert_eq!(poll!(&mut fut), Poll::Pending);

            event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();

            let res = fut.await;
            assert!(res.is_ok());
            assert!(res.unwrap().contains(zx::Signals::EVENT_SIGNALED));
        });
    }

    #[test]
    fn wait_on_already_signaled() {
        spawn_in_driver("testing wait", async move {
            let event = zx::Event::create();
            event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();

            let fut = CurrentDispatcher.on_signals(&event, zx::Signals::EVENT_SIGNALED);

            let res = fut.await;
            assert!(res.is_ok());
            assert!(res.unwrap().contains(zx::Signals::EVENT_SIGNALED));
        });
    }

    #[test]
    fn drop_after_poll() {
        spawn_in_driver("testing wait", async move {
            let event = zx::Event::create();
            let mut fut = CurrentDispatcher.on_signals(&event, zx::Signals::EVENT_SIGNALED);
            assert_eq!(poll!(&mut fut), Poll::Pending);
        });
    }

    #[test]
    fn dispatcher_shutdown_cancel() {
        static EVENT: LazyLock<zx::Event> = LazyLock::new(zx::Event::create);

        let (fut_tx, fut_rx) = mpsc::channel();
        spawn_in_driver("testing wait", async move {
            let mut fut = CurrentDispatcher.on_signals(&*EVENT, zx::Signals::EVENT_SIGNALED);
            assert_eq!(poll!(&mut fut), Poll::Pending);
            fut_tx.send(fut).unwrap();
        });
        let mut fut = fut_rx.recv().unwrap();
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        loop {
            let Poll::Ready(res) = fut.poll_unpin(&mut context) else {
                sleep(Duration::from_millis(10));
                continue;
            };
            assert_eq!(res, Err(Status::CANCELED));
            break;
        }
    }

    #[test]
    fn test_take_handle() {
        spawn_in_driver("testing wait", async move {
            let event = zx::Event::create();
            let mut fut = CurrentDispatcher.on_signals(event, zx::Signals::EVENT_SIGNALED);
            assert_eq!(poll!(&mut fut), Poll::Pending);
            let event = fut.take_handle().unwrap();
            event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();

            assert!(fut.take_handle().is_none());
            assert_eq!(poll!(&mut fut), Poll::Ready(Err(Status::BAD_STATE)));
            assert_eq!(
                event
                    .wait_one(zx::Signals::EVENT_SIGNALED, zx::MonotonicInstant::INFINITE_PAST)
                    .unwrap(),
                zx::Signals::EVENT_SIGNALED
            );
        });
    }
}
