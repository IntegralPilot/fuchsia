// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use std::{fmt, mem};

use crate::runtime::{EHandle, PacketReceiver, RawReceiverRegistration};
use futures::task::{AtomicWaker, Context};
use zx::AsHandleRef;

struct OnSignalsReceiver {
    maybe_signals: AtomicUsize,
    task: AtomicWaker,
}

impl OnSignalsReceiver {
    fn get_signals(&self, cx: &mut Context<'_>) -> Poll<zx::Signals> {
        let mut signals = self.maybe_signals.load(Ordering::Relaxed);
        if signals == 0 {
            // No signals were received-- register to receive a wakeup when they arrive.
            self.task.register(cx.waker());
            // Check again for signals after registering for a wakeup in case signals
            // arrived between registering and the initial load of signals
            signals = self.maybe_signals.load(Ordering::SeqCst);
        }
        if signals == 0 {
            Poll::Pending
        } else {
            Poll::Ready(zx::Signals::from_bits_truncate(signals as u32))
        }
    }

    fn set_signals(&self, signals: zx::Signals) {
        self.maybe_signals.store(signals.bits() as usize, Ordering::SeqCst);
        self.task.wake();
    }
}

impl PacketReceiver for OnSignalsReceiver {
    fn receive_packet(&self, packet: zx::Packet) {
        let observed = if let zx::PacketContents::SignalOne(p) = packet.contents() {
            p.observed()
        } else {
            return;
        };

        self.set_signals(observed);
    }
}

pin_project_lite::pin_project! {
    /// A future that completes when some set of signals become available on a Handle.
    #[must_use = "futures do nothing unless polled"]
    pub struct OnSignals<'a, H: AsHandleRef> {
        handle: H,
        signals: zx::Signals,
        #[pin]
        registration: RawReceiverRegistration<OnSignalsReceiver>,
        phantom: PhantomData<&'a H>,
    }

    impl<'a, H: AsHandleRef> PinnedDrop for OnSignals<'a, H> {
        fn drop(mut this: Pin<&mut Self>) {
            this.unregister();
        }
    }
}

impl<'a, H: AsHandleRef + 'a> OnSignals<'a, H> {
    /// Creates a new `OnSignals` object which will receive notifications when
    /// any signals in `signals` occur on `handle`.
    pub fn new(handle: H, signals: zx::Signals) -> Self {
        // We don't register for the signals until first polled.  When we are first polled, we'll
        // check to see if the signals are set and if they are, we're done.  If they aren't, we then
        // register for an asynchronous notification via the port.
        //
        // We could change the code to register for the asynchronous notification here, but then
        // when first polled, if the notification hasn't arrived, we'll still check to see if the
        // signals are set (see below for the reason why).  Given that the time between construction
        // and when we first poll is typically small, registering here probably won't make much
        // difference (and on a single-threaded executor, a notification is unlikely to be processed
        // before the first poll anyway).  The way we have it now means we don't have to register at
        // all if the signals are already set, which will be a win some of the time.
        OnSignals {
            handle,
            signals,
            registration: RawReceiverRegistration::new(OnSignalsReceiver {
                maybe_signals: AtomicUsize::new(0),
                task: AtomicWaker::new(),
            }),
            phantom: PhantomData,
        }
    }

    /// Takes the handle.
    pub fn take_handle(mut self: Pin<&mut Self>) -> H
    where
        H: From<zx::NullableHandle>,
    {
        if self.registration.is_registered() {
            self.as_mut().unregister();
        }
        mem::replace(self.project().handle, zx::NullableHandle::invalid().into())
    }

    fn register(
        mut registration: Pin<&mut RawReceiverRegistration<OnSignalsReceiver>>,
        handle: &H,
        signals: zx::Signals,
        cx: Option<&mut Context<'_>>,
    ) -> Result<(), zx::Status> {
        registration.as_mut().register(EHandle::local());

        // If a context has been supplied, we must register it now before calling
        // `wait_async_handle` below to avoid races.
        if let Some(cx) = cx {
            registration.receiver().task.register(cx.waker());
        }

        handle.as_handle_ref().wait_async(
            registration.port().unwrap(),
            registration.key().unwrap(),
            signals,
            zx::WaitAsyncOpts::empty(),
        )?;

        Ok(())
    }

    fn unregister(self: Pin<&mut Self>) {
        let mut this = self.project();
        if let Some((ehandle, key)) = this.registration.as_mut().unregister()
            && this.registration.receiver().maybe_signals.load(Ordering::SeqCst) == 0
        {
            // Ignore the error from zx_port_cancel, because it might just be a race condition.
            // If the packet is handled between the above maybe_signals check and the port
            // cancel, it will fail with ZX_ERR_NOT_FOUND, and we can't do anything about it.
            let _ = ehandle.port().cancel(key);
        }
    }
}

impl<H: AsHandleRef> Future for OnSignals<'_, H> {
    type Output = Result<zx::Signals, zx::Status>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.registration.is_registered() {
            match self
                .handle
                .as_handle_ref()
                .wait_one(self.signals, zx::MonotonicInstant::INFINITE_PAST)
                .to_result()
            {
                Ok(signals) => Poll::Ready(Ok(signals)),
                Err(zx::Status::TIMED_OUT) => {
                    let mut this = self.project();
                    Self::register(
                        this.registration.as_mut(),
                        this.handle,
                        *this.signals,
                        Some(cx),
                    )?;
                    Poll::Pending
                }
                Err(e) => Poll::Ready(Err(e)),
            }
        } else {
            match self.registration.receiver().get_signals(cx) {
                Poll::Ready(signals) => Poll::Ready(Ok(signals)),
                Poll::Pending => {
                    // We haven't received a notification for the signals, but we still want to poll
                    // the kernel in case the notification hasn't been processed yet by the
                    // executor.  This behaviour is relied upon in some cases: in Component Manager,
                    // in some shutdown paths, it wants to drain and process all messages in
                    // channels before it closes them.  There is no other reliable way to flush a
                    // pending notification (particularly on a multi-threaded executor).  This will
                    // incur a small performance penalty in the case that this future has been
                    // polled when no notification was actually received (such as can be the case
                    // with some futures combinators).
                    match self
                        .handle
                        .as_handle_ref()
                        .wait_one(self.signals, zx::MonotonicInstant::INFINITE_PAST)
                        .to_result()
                    {
                        Ok(signals) => Poll::Ready(Ok(signals)),
                        Err(_) => Poll::Pending,
                    }
                }
            }
        }
    }
}

impl<H: AsHandleRef> fmt::Debug for OnSignals<'_, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OnSignals")
    }
}

impl<H: AsHandleRef> AsHandleRef for OnSignals<'_, H> {
    fn as_handle_ref(&self) -> zx::HandleRef<'_> {
        self.handle.as_handle_ref()
    }
}

impl<H: AsHandleRef> AsRef<H> for OnSignals<'_, H> {
    fn as_ref(&self) -> &H {
        &self.handle
    }
}

/// Alias for the common case where OnSignals is used with zx::HandleRef.
pub type OnSignalsRef<'a> = OnSignals<'a, zx::HandleRef<'a>>;

#[cfg(test)]
mod test {
    use super::*;
    use crate::TestExecutor;
    use assert_matches::assert_matches;
    use futures::future::{FutureExt, pending};
    use std::pin::pin;
    use std::task::Waker;

    #[test]
    fn wait_for_event() -> Result<(), zx::Status> {
        let mut exec = crate::TestExecutor::new();
        let mut deliver_events =
            || assert!(exec.run_until_stalled(&mut pending::<()>()).is_pending());

        let event = zx::Event::create();
        let mut signals = pin!(OnSignals::new(&event, zx::Signals::EVENT_SIGNALED));
        let (waker, waker_count) = futures_test::task::new_count_waker();
        let cx = &mut std::task::Context::from_waker(&waker);

        // Check that `signals` is still pending before the event has been signaled
        assert_eq!(signals.poll_unpin(cx), Poll::Pending);
        deliver_events();
        assert_eq!(waker_count, 0);
        assert_eq!(signals.poll_unpin(cx), Poll::Pending);

        // signal the event and check that `signals` has been woken up and is
        // no longer pending
        event.signal(zx::Signals::NONE, zx::Signals::EVENT_SIGNALED)?;
        deliver_events();
        assert_eq!(waker_count, 1);
        assert_eq!(signals.poll_unpin(cx), Poll::Ready(Ok(zx::Signals::EVENT_SIGNALED)));

        Ok(())
    }

    #[test]
    fn drop_before_event() {
        let mut fut = std::pin::pin!(async {
            let ehandle = EHandle::local();

            let event = zx::Event::create();
            let key = {
                let mut signals = pin!(OnSignals::new(&event, zx::Signals::EVENT_SIGNALED));
                assert_eq!(futures::poll!(&mut signals), Poll::Pending);
                signals.registration.key().unwrap()
            };

            assert!(ehandle.port().cancel(key) == Err(zx::Status::NOT_FOUND));
        });

        assert!(TestExecutor::new().run_until_stalled(&mut fut).is_ready());
    }

    #[test]
    fn test_always_polls() {
        let mut exec = TestExecutor::new();

        let (rx, tx) = zx::Channel::create();

        let mut fut = pin!(OnSignals::new(&rx, zx::Signals::CHANNEL_READABLE));

        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Pending);

        tx.write(b"hello", &mut []).expect("write failed");

        // Poll the future directly which guarantees the port notification for the write hasn't
        // arrived.
        assert_matches!(
            fut.poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(signals)) if signals.contains(zx::Signals::CHANNEL_READABLE)
        );
    }

    #[test]
    fn test_take_handle() {
        let mut exec = TestExecutor::new();

        let (rx, tx) = zx::Channel::create();

        let mut fut = pin!(OnSignals::new(rx, zx::Signals::CHANNEL_READABLE));

        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Pending);

        tx.write(b"hello", &mut []).expect("write failed");

        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(_)));

        let mut message = zx::MessageBuf::new();
        fut.take_handle().read(&mut message).unwrap();

        assert_eq!(message.bytes(), b"hello");
    }
}
