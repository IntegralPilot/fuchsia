// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::Stream;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use zx::{
    AsHandleRef, BootTimeline, Instant, Interrupt, InterruptKind, RealInterruptKind, Timeline, sys,
};

use crate::runtime::{EHandle, PacketReceiver, RawReceiverRegistration};
use futures::task::{AtomicWaker, Context};

struct OnInterruptReceiver {
    maybe_timestamp: AtomicUsize,
    task: AtomicWaker,
}

impl OnInterruptReceiver {
    fn get_interrupt(&self, cx: &mut Context<'_>) -> Poll<sys::zx_time_t> {
        let mut timestamp = self.maybe_timestamp.swap(0, Ordering::Relaxed);
        if timestamp == 0 {
            // The interrupt did not fire -- register to receive a wakeup when it does.
            self.task.register(cx.waker());
            // Check again for a timestamp after registering for a wakeup in case it fired
            // between registering and the initial load.
            // NOTE: We might be able to use a weaker ordering because we use AtomicWaker.
            timestamp = self.maybe_timestamp.swap(0, Ordering::SeqCst);
        }
        if timestamp == 0 { Poll::Pending } else { Poll::Ready(timestamp as i64) }
    }

    fn set_timestamp(&self, timestamp: sys::zx_time_t) {
        self.maybe_timestamp.store(timestamp as usize, Ordering::SeqCst);
        self.task.wake();
    }
}

impl PacketReceiver for OnInterruptReceiver {
    fn receive_packet(&self, packet: zx::Packet) {
        let zx::PacketContents::Interrupt(interrupt) = packet.contents() else {
            return;
        };
        self.set_timestamp(interrupt.timestamp());
    }
}

pin_project_lite::pin_project! {
/// A stream that returns each time an interrupt fires.
#[must_use = "future streams do nothing unless polled"]
pub struct OnInterrupt<K: InterruptKind = RealInterruptKind, T: Timeline = BootTimeline> {
    interrupt: Interrupt<K, T>,
    #[pin]
    registration: RawReceiverRegistration<OnInterruptReceiver>,
}

impl<K: InterruptKind, T: Timeline> PinnedDrop for OnInterrupt<K, T> {
        fn drop(mut this: Pin<&mut Self>) {
        this.unregister()
    }
}

}

impl<K: InterruptKind, T: Timeline> OnInterrupt<K, T> {
    /// Creates a new OnInterrupt object which will notifications when `interrupt` fires.
    /// NOTE: This will only work on a port that was created with the BIND_TO_INTERRUPT option.
    pub fn new(interrupt: Interrupt<K, T>) -> Self {
        Self {
            interrupt,
            registration: RawReceiverRegistration::new(OnInterruptReceiver {
                maybe_timestamp: AtomicUsize::new(0),
                task: AtomicWaker::new(),
            }),
        }
    }

    fn register(
        mut registration: Pin<&mut RawReceiverRegistration<OnInterruptReceiver>>,
        interrupt: &Interrupt<K, T>,
        cx: Option<&mut Context<'_>>,
    ) -> Result<(), zx::Status> {
        registration.as_mut().register(EHandle::local());

        // If a context has been supplied, we must register it now before calling
        // `bind_port` below to avoid races.
        if let Some(cx) = cx {
            registration.receiver().task.register(cx.waker());
        }

        interrupt.bind_port(registration.port().unwrap(), registration.key().unwrap())?;

        Ok(())
    }

    fn unregister(self: Pin<&mut Self>) {
        let mut this = self.project();
        if let Some((ehandle, key)) = this.registration.as_mut().unregister() {
            let _ = ehandle.port().cancel(key);
        }
    }
}

impl<K: InterruptKind, T: Timeline> AsHandleRef for OnInterrupt<K, T> {
    fn as_handle_ref(&self) -> zx::HandleRef<'_> {
        self.interrupt.as_handle_ref()
    }
}

impl<K: InterruptKind, T: Timeline> AsRef<Interrupt<K, T>> for OnInterrupt<K, T> {
    fn as_ref(&self) -> &Interrupt<K, T> {
        &self.interrupt
    }
}

impl<K: InterruptKind, T: Timeline> Stream for OnInterrupt<K, T> {
    type Item = Result<Instant<T>, zx::Status>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if !self.registration.is_registered() {
            let mut this = self.project();
            Self::register(this.registration.as_mut(), this.interrupt, Some(cx))?;
            Poll::Pending
        } else {
            match self.registration.receiver().get_interrupt(cx) {
                Poll::Ready(timestamp) => {
                    Poll::Ready(Some(Ok(Instant::<T>::from_nanos(timestamp))))
                }
                Poll::Pending => Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::future::pending;

    #[test]
    fn wait_for_event() -> Result<(), zx::Status> {
        let port = zx::Port::create_with_opts(zx::PortOptions::BIND_TO_INTERRUPT);
        let mut exec = crate::TestExecutor::builder().port(port).build();
        let mut deliver_events =
            || assert!(exec.run_until_stalled(&mut pending::<()>()).is_pending());

        let irq = zx::VirtualInterrupt::create_virtual()?;
        let mut irq = std::pin::pin!(OnInterrupt::new(irq));
        let (waker, waker_count) = futures_test::task::new_count_waker();
        let cx = &mut std::task::Context::from_waker(&waker);

        // Check that `irq` is still pending before the interrupt has fired.
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Pending);
        deliver_events();
        assert_eq!(waker_count, 0);
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Pending);

        // Trigger the interrupt and check that we receive the same timestamp.
        let timestamp = zx::BootInstant::from_nanos(10);
        irq.interrupt.trigger(timestamp)?;
        deliver_events();
        assert_eq!(waker_count, 1);
        let expected: Result<_, zx::Status> = Ok(timestamp);
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Ready(Some(expected)));

        // Check that we are polling pending now.
        deliver_events();
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Pending);

        // Signal a second time to check that the stream works.
        irq.interrupt.ack()?;
        let timestamp = zx::BootInstant::from_nanos(20);
        irq.interrupt.trigger(timestamp)?;
        deliver_events();
        let expected: Result<_, zx::Status> = Ok(timestamp);
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Ready(Some(expected)));

        Ok(())
    }

    #[test]
    fn wait_for_event_monotonic() -> Result<(), zx::Status> {
        let port = zx::Port::create_with_opts(zx::PortOptions::BIND_TO_INTERRUPT);
        let mut exec = crate::TestExecutor::builder().port(port).build();
        let mut deliver_events =
            || assert!(exec.run_until_stalled(&mut pending::<()>()).is_pending());

        let irq =
            zx::Interrupt::<zx::VirtualInterruptKind, zx::MonotonicTimeline>::create_virtual()?;
        let mut irq = std::pin::pin!(OnInterrupt::new(irq));
        let (waker, waker_count) = futures_test::task::new_count_waker();
        let cx = &mut std::task::Context::from_waker(&waker);

        // Check that `irq` is still pending before the interrupt has fired.
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Pending);
        deliver_events();
        assert_eq!(waker_count, 0);
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Pending);

        // Trigger the interrupt and check that we receive the same timestamp.
        let timestamp = zx::MonotonicInstant::from_nanos(10);
        irq.interrupt.trigger(timestamp)?;
        deliver_events();
        assert_eq!(waker_count, 1);
        let expected: Result<_, zx::Status> = Ok(timestamp);
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Ready(Some(expected)));

        // Check that we are polling pending now.
        deliver_events();
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Pending);

        // Signal a second time to check that the stream works.
        irq.interrupt.ack()?;
        let timestamp = zx::MonotonicInstant::from_nanos(20);
        irq.interrupt.trigger(timestamp)?;
        deliver_events();
        let expected: Result<_, zx::Status> = Ok(timestamp);
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Ready(Some(expected)));

        Ok(())
    }

    #[test]
    fn incorrect_port() -> Result<(), zx::Status> {
        let _exec = crate::TestExecutor::new();

        let irq = zx::VirtualInterrupt::create_virtual()?;
        let mut irq = std::pin::pin!(OnInterrupt::new(irq));
        let (waker, _waker_count) = futures_test::task::new_count_waker();
        let cx = &mut std::task::Context::from_waker(&waker);

        // Polling the interrupt should cause an error.
        assert_eq!(irq.as_mut().poll_next(cx), Poll::Ready(Some(Err(zx::Status::WRONG_TYPE))));

        Ok(())
    }
}
