// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// A [`PortEvent`] is interested only in events originating from within the
/// process (see [`PortEvent.futex`] for more details), and the waiter is
/// may be notified up.
const FUTEX_WAITING: i32 = 0;
/// A [`PortEvent`] is interested only in events originating from within the
/// process (see [`PortEvent.futex`] for more details), and the waiter is
/// has been notified of an event.
const FUTEX_NOTIFIED: i32 = 1;
/// A [`PortEvent`] is interested only in events originating from within the
/// process (see [`PortEvent.futex`] for more details), and the waiter is
/// has been notified of an interrupt.
const FUTEX_INTERRUPTED: i32 = 2;
/// A [`PortEvent`] is interested in events originating from outside of
/// process (see [`PortEvent.futex`] for more details). The waiter's `zx::Port`
/// should be used instead of the Futex.
const FUTEX_USE_PORT: i32 = 3;

/// Specifies the ordering for atomics accessed by both the "notifier" and
/// "notifee" (the waiter).
///
/// Relaxed ordering because the [`PortEvent`] does not provide synchronization
/// between the "notifier" and the "notifee". If a notifiee needs synchronization,
/// it needs to perform that synchronization itself.
///
/// See [`PortEvent.wait`] for more details.
const ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE: Ordering = Ordering::Relaxed;

/// A wrapper around a [`zx::Port`] that optimizes for the case where events are
/// signaled within a process.
///
/// This object will prefer to use a Futex for notifications/interrupts but will
/// fallback to a `zx::Port` when the port is subscribed for events on an object
/// with [`PortEvent.object_wait_async`].
///
/// Note that the `PortEvent` does not provide any synchronization between a
/// notifier (caller of [`PortEvent.notify`]) and a notifiee/waiter (caller of
/// [`PortEvent.wait`].
#[derive(Debug)]
pub struct PortEvent {
    /// The Futex used to wake up a thread when this waiter is waiting for
    /// events that don't depend on a `zx::Port`.
    ///
    /// This mode is subject to spurious wakeups.
    /// See more in https://fuchsia.dev/reference/syscalls/futex_wait?hl=en#spurious_wakeups
    futex: zx::Futex,
    /// The underlying Zircon port that the waiter waits on when it is
    /// interested in events that cross process boundaries.
    ///
    /// Lazily allocated to optimize for the case where waiters are interested
    /// only in events triggered within a process.
    port: LazyLock<zx::Port>,
    /// Indicates whether a user packet is sitting in the `zx::Port` to wake up
    /// waiter after handling user events.
    has_pending_user_packet: AtomicBool,
}

/// The kind of notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotifyKind {
    Regular,
    Interrupt,
}

/// The result of a call to [`PortEvent.wait`].
#[derive(Debug, Eq, PartialEq)]
pub enum PortWaitResult {
    /// Signals asserted on an object.
    Signal { key: u64, observed: zx::Signals },
    /// A notification to wake up waiters.
    Notification { kind: NotifyKind },
    /// Wait timed out.
    TimedOut,
}

impl PortWaitResult {
    const NOTIFY_REGULAR: Self = Self::Notification { kind: NotifyKind::Regular };
    const NOTIFY_INTERRUPT: Self = Self::Notification { kind: NotifyKind::Interrupt };
}

impl PortEvent {
    /// Returns a new `PortEvent`.
    pub fn new() -> Self {
        Self {
            futex: zx::Futex::new(FUTEX_WAITING),
            port: LazyLock::new(zx::Port::create),
            has_pending_user_packet: Default::default(),
        }
    }

    /// Wait for an event to occur, or the deadline has been reached.
    pub fn wait(&self, deadline: zx::MonotonicInstant) -> PortWaitResult {
        let mut state = self.futex.load(ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE);
        loop {
            match state {
                FUTEX_WAITING => match self.futex.wait(FUTEX_WAITING, None, deadline) {
                    Ok(()) | Err(zx::Status::BAD_STATE) => {
                        state = self.futex.load(ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE);
                    }
                    Err(zx::Status::TIMED_OUT) => {
                        return PortWaitResult::TimedOut;
                    }
                    Err(e) => panic!("Unexpected error from zx_futex_wait: {e}"),
                },
                FUTEX_NOTIFIED | FUTEX_INTERRUPTED => {
                    match self.futex.compare_exchange(
                        state,
                        FUTEX_WAITING,
                        ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE,
                        ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE,
                    ) {
                        Ok(new_state) => {
                            debug_assert_eq!(new_state, state);
                            return if new_state == FUTEX_INTERRUPTED {
                                PortWaitResult::NOTIFY_INTERRUPT
                            } else {
                                PortWaitResult::NOTIFY_REGULAR
                            };
                        }
                        Err(new_state) => {
                            debug_assert_ne!(new_state, state);
                            state = new_state;
                        }
                    }
                }
                FUTEX_USE_PORT => {
                    break;
                }
                state => unreachable!("unexpected value = {state}"),
            }
        }

        match self.port.wait(deadline) {
            Ok(packet) => match packet.status() {
                zx::sys::ZX_OK => {
                    match packet.contents() {
                        zx::PacketContents::SignalOne(sigpkt) => PortWaitResult::Signal {
                            key: packet.key(),
                            observed: sigpkt.observed(),
                        },
                        zx::PacketContents::User(_) => {
                            // User packet w/ OK status is only used to wake up
                            // the waiter after handling process-internal events.
                            //
                            // Note that we can be woken up even when we will
                            // not handle any user events. This is because right
                            // after we set `has_pending_user_packet` to `false`,
                            // another thread can immediately queue a new user
                            // event and set `has_pending_user_packet` to `true`.
                            // However, that event will be handled by us (by the
                            // caller when this method returns) as if the event
                            // was enqueued before we received this user packet.
                            // Once the caller handles all the current user events,
                            // we end up with no remaining user events but a user
                            // packet sitting in the `zx::Port`.
                            assert!(
                                self.has_pending_user_packet
                                    .swap(false, ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE)
                            );

                            PortWaitResult::NOTIFY_REGULAR
                        }
                        _contents => panic!("unexpected packet = {:?}", packet),
                    }
                }
                zx::sys::ZX_ERR_CANCELED => PortWaitResult::NOTIFY_INTERRUPT,
                status => {
                    panic!("Unexpected status in port wait {}", status);
                }
            },
            Err(zx::Status::TIMED_OUT) => PortWaitResult::TimedOut,
            Err(e) => panic!("Unexpected error from port_wait: {e}"),
        }
    }

    /// Subscribe for signals on an object.
    pub fn object_wait_async(
        &self,
        handle: &dyn zx::AsHandleRef,
        key: u64,
        signals: zx::Signals,
        opts: zx::WaitAsyncOpts,
    ) -> Result<(), zx::Status> {
        match self.futex.swap(FUTEX_USE_PORT, ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE) {
            FUTEX_WAITING => {
                self.futex.wake_all();
            }
            state @ (FUTEX_NOTIFIED | FUTEX_INTERRUPTED) => {
                self.queue_user_packet_data(if state == FUTEX_INTERRUPTED {
                    NotifyKind::Interrupt
                } else {
                    NotifyKind::Regular
                })
            }
            FUTEX_USE_PORT => {}
            v => unreachable!("unexpected value = {v}"),
        }

        handle.as_handle_ref().wait_async(&self.port, key, signals, opts)
    }

    /// Cancels async port notifications on an object.
    pub fn cancel(&self, key: u64) {
        let _: Result<(), zx::Status> = self.port.cancel(key);
    }

    /// Queue a packet to the underlying Zircon port, which will cause the
    /// waiter to wake up.
    ///
    /// This method should only be called when the waiter is interested in
    /// events that may originate from outside of the process.
    fn queue_user_packet_data(&self, kind: NotifyKind) {
        let status = match kind {
            NotifyKind::Interrupt => zx::sys::ZX_ERR_CANCELED,
            NotifyKind::Regular => {
                if self
                    .has_pending_user_packet
                    .swap(true, ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE)
                {
                    return;
                }

                zx::sys::ZX_OK
            }
        };

        let packet = zx::Packet::from_user_packet(0, status, zx::UserPacket::default());
        self.port.queue(&packet).unwrap()
    }

    /// Marks the port as ready to handle a notification (or an interrupt) and
    /// wakes up any blocked waiters.
    pub fn notify(&self, kind: NotifyKind) {
        let futex_val = match kind {
            NotifyKind::Interrupt => FUTEX_INTERRUPTED,
            NotifyKind::Regular => FUTEX_NOTIFIED,
        };

        match self.futex.compare_exchange(
            FUTEX_WAITING,
            futex_val,
            ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE,
            ORDERING_FOR_ATOMICS_BETWEEN_NOTIFIER_AND_NOTIFEE,
        ) {
            Ok(observed) => {
                debug_assert_eq!(observed, FUTEX_WAITING);
                self.futex.wake_all();
            }
            Err(observed) => match observed {
                FUTEX_WAITING => unreachable!("this should have passed"),
                FUTEX_NOTIFIED | FUTEX_INTERRUPTED => {}
                FUTEX_USE_PORT => {
                    self.queue_user_packet_data(kind);
                }
                observed => unreachable!("unexpected value = {observed}"),
            },
        }
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use test_case::test_case;

    use super::*;

    #[test]
    fn test_signal_and_wait_block() {
        const KEY: u64 = 1;
        const ASSERTED_SIGNAL: zx::Signals = zx::Signals::USER_0;

        let event = Arc::new(PortEvent::new());
        let object = zx::Event::create();
        assert_eq!(
            event.object_wait_async(&object, KEY, ASSERTED_SIGNAL, zx::WaitAsyncOpts::empty()),
            Ok(())
        );

        let event_clone = event.clone();
        let thread = std::thread::spawn(move || {
            assert_eq!(
                event_clone.wait(zx::MonotonicInstant::INFINITE),
                PortWaitResult::Signal { key: KEY, observed: ASSERTED_SIGNAL }
            );
        });

        object.signal(/*clear_mask=*/ zx::Signals::NONE, /*set_mask=*/ ASSERTED_SIGNAL).unwrap();
        thread.join().expect("join thread");
    }

    #[test]
    fn test_signal_then_wait_nonblock() {
        const KEY: u64 = 2;
        const ASSERTED_SIGNAL: zx::Signals = zx::Signals::USER_1;

        let event = PortEvent::new();
        let object = zx::Event::create();
        assert_eq!(
            event.object_wait_async(&object, KEY, ASSERTED_SIGNAL, zx::WaitAsyncOpts::empty()),
            Ok(())
        );
        object.signal(/*clear_mask=*/ zx::Signals::NONE, /*set_mask=*/ ASSERTED_SIGNAL).unwrap();

        assert_eq!(
            event.wait(zx::MonotonicInstant::INFINITE_PAST),
            PortWaitResult::Signal { key: KEY, observed: ASSERTED_SIGNAL }
        );
    }

    #[test]
    fn test_signal_then_cancel_then_wait() {
        const KEY: u64 = 3;
        const ASSERTED_SIGNAL: zx::Signals = zx::Signals::USER_2;

        let event = PortEvent::new();
        let object = zx::Event::create();

        assert_eq!(
            event.object_wait_async(&object, KEY, ASSERTED_SIGNAL, zx::WaitAsyncOpts::empty()),
            Ok(())
        );
        object.signal(/*clear_mask=*/ zx::Signals::NONE, /*set_mask=*/ ASSERTED_SIGNAL).unwrap();

        event.cancel(KEY);
        assert_eq!(event.wait(zx::MonotonicInstant::INFINITE_PAST), PortWaitResult::TimedOut);
    }

    #[test_case(NotifyKind::Interrupt, true; "interrupt with object")]
    #[test_case(NotifyKind::Regular, true; "not interrupt with object")]
    #[test_case(NotifyKind::Interrupt, false; "interrupt without object")]
    #[test_case(NotifyKind::Regular, false; "not interrupt without object")]
    fn test_notify_and_wait_block(kind: NotifyKind, with_object: bool) {
        const KEY: u64 = 4;

        let event = Arc::new(PortEvent::new());
        let object = zx::Event::create();
        if with_object {
            assert_eq!(
                event.object_wait_async(
                    &object,
                    KEY,
                    zx::Signals::USER_3,
                    zx::WaitAsyncOpts::empty()
                ),
                Ok(())
            );
        }

        let event_clone = event.clone();
        let thread = std::thread::spawn(move || {
            assert_eq!(
                event_clone.wait(zx::MonotonicInstant::INFINITE),
                PortWaitResult::Notification { kind }
            );
        });

        event.notify(kind);
        thread.join().expect("join thread");
    }

    #[test_case(NotifyKind::Interrupt, true; "interrupt with object")]
    #[test_case(NotifyKind::Regular, true; "not interrupt with object")]
    #[test_case(NotifyKind::Interrupt, false; "interrupt without object")]
    #[test_case(NotifyKind::Regular, false; "not interrupt without object")]
    fn test_notify_then_wait_nonblock(kind: NotifyKind, with_object: bool) {
        const KEY: u64 = 5;

        let event = PortEvent::new();
        let object = zx::Event::create();
        if with_object {
            assert_eq!(
                event.object_wait_async(
                    &object,
                    KEY,
                    zx::Signals::USER_4,
                    zx::WaitAsyncOpts::empty()
                ),
                Ok(())
            );
        }

        event.notify(kind);
        assert_eq!(
            event.wait(zx::MonotonicInstant::INFINITE_PAST),
            PortWaitResult::Notification { kind }
        );
    }

    #[test_case(true, zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(100)); "blocking with object")]
    #[test_case(false, zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(100)); "blocking without object")]
    #[test_case(true, zx::MonotonicInstant::INFINITE_PAST; "non blocking with object")]
    #[test_case(false, zx::MonotonicInstant::INFINITE_PAST; "non blocking without object")]
    fn test_wait_timeout(with_object: bool, deadline: zx::MonotonicInstant) {
        const KEY: u64 = 6;
        const ASSERTED_SIGNAL: zx::Signals = zx::Signals::USER_5;

        let event = PortEvent::new();
        let object = zx::Event::create();

        if with_object {
            assert_eq!(
                event.object_wait_async(&object, KEY, ASSERTED_SIGNAL, zx::WaitAsyncOpts::empty()),
                Ok(())
            );
        }
        assert_eq!(event.wait(deadline), PortWaitResult::TimedOut);
    }
}
