// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Defines trait abstractions for platform dependencies of the DHCP client
//! core, and provides fake implementations of these dependencies for testing
//! purposes.

use diagnostics_traits::InspectableInstant;
use fuchsia_async as fasync;
use rand::Rng;
use std::future::Future;

/// Provides access to random number generation.
pub trait RngProvider {
    /// The random number generator being provided.
    type RNG: Rng + ?Sized;

    /// Get access to a random number generator.
    fn get_rng(&mut self) -> &mut Self::RNG;
}

impl RngProvider for rand::rngs::StdRng {
    type RNG = Self;
    fn get_rng(&mut self) -> &mut Self::RNG {
        self
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
/// Contains information about a datagram received on a socket.
pub struct DatagramInfo<T> {
    /// The length in bytes of the datagram received on the socket.
    pub length: usize,
    /// The address associated with the datagram received on the socket
    /// (usually, the address from which the datagram was received).
    pub address: T,
}

#[derive(thiserror::Error, Debug)]
/// Errors encountered while performing a socket operation.
pub enum SocketError {
    /// Failure while attempting to open a socket.
    #[error("failed to open socket: {0}")]
    FailedToOpen(anyhow::Error),
    /// Tried to bind a socket on a nonexistent interface.
    #[error("tried to bind socket on nonexistent interface")]
    NoInterface,
    /// The hardware type of the interface is unsupported.
    #[error("unsupported hardware type")]
    UnsupportedHardwareType,
    /// The host we are attempting to send to is unreachable.
    #[error("host unreachable")]
    HostUnreachable,
    /// The network is unreachable.
    #[error("network unreachable")]
    NetworkUnreachable,
    /// The address is not available.
    #[error("address not available")]
    AddrNotAvailable,
    /// Broken pipe.
    #[error("broken pipe")]
    BrokenPipe,
    /// The connection was aborted.
    #[error("connection aborted")]
    ConnectionAborted,
    /// Other IO errors observed on socket operations.
    #[error("socket error: {0}")]
    Other(std::io::Error),
}

/// Abstracts sending and receiving datagrams on a socket.
pub trait Socket<T> {
    /// Sends a datagram containing the contents of `buf` to `addr`.
    fn send_to(&self, buf: &[u8], addr: T) -> impl Future<Output = Result<(), SocketError>>;

    /// Receives a datagram into `buf`, returning the number of bytes received
    /// and the address the datagram was received from.
    fn recv_from(
        &self,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<DatagramInfo<T>, SocketError>>;
}

/// Provides access to AF_PACKET sockets.
pub trait PacketSocketProvider {
    /// The type of sockets provided by this `PacketSocketProvider`.
    type Sock: Socket<net_types::ethernet::Mac>;

    /// Gets a packet socket bound to the device on which the DHCP client
    /// protocol is being performed. The packet socket should already be bound
    /// to the appropriate device and protocol number.
    fn get_packet_socket(&self) -> impl Future<Output = Result<Self::Sock, SocketError>>;
}

/// Provides access to UDP sockets.
pub trait UdpSocketProvider {
    /// The type of sockets provided by this `UdpSocketProvider`.
    type Sock: Socket<std::net::SocketAddr>;

    /// Gets a UDP socket bound to the given address. The UDP socket should be
    /// allowed to send broadcast packets.
    fn bind_new_udp_socket(
        &self,
        bound_addr: std::net::SocketAddr,
    ) -> impl Future<Output = Result<Self::Sock, SocketError>>;
}

/// A type representing an instant in time.
pub trait Instant:
    Sized + Ord + Copy + Clone + std::fmt::Debug + Send + Sync + InspectableInstant
{
    /// Returns the time `self + duration`. Panics if `self + duration` would
    /// overflow the underlying instant storage type.
    fn add(&self, duration: std::time::Duration) -> Self;

    /// Returns the instant halfway between `self` and `other`.
    fn average(&self, other: Self) -> Self;
}

impl Instant for fasync::MonotonicInstant {
    fn add(&self, duration: std::time::Duration) -> Self {
        *self + duration.into()
    }

    fn average(&self, other: Self) -> Self {
        let lower = *self.min(&other);
        let higher = *self.max(&other);
        lower + (higher - lower) / 2
    }
}

/// Provides access to system-time-related operations.
pub trait Clock {
    /// The type representing monotonic system time.
    type Instant: Instant;

    /// Completes once the monotonic system time is at or after the given time.
    fn wait_until(&self, time: Self::Instant) -> impl Future<Output = ()>;

    /// Gets the monotonic system time.
    fn now(&self) -> Self::Instant;
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use diagnostics_traits::InstantPropertyName;
    use futures::StreamExt as _;
    use futures::channel::{mpsc, oneshot};
    use futures::lock::Mutex;
    use rand::SeedableRng as _;
    use std::cell::RefCell;
    use std::cmp::Reverse;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::ops::{Deref as _, DerefMut as _};
    use std::rc::Rc;

    /// Provides a seedable implementation of `RngProvider` using `StdRng`.
    pub(crate) struct FakeRngProvider {
        std_rng: rand::rngs::StdRng,
    }

    impl FakeRngProvider {
        pub(crate) fn new(seed: u64) -> Self {
            Self { std_rng: rand::rngs::StdRng::seed_from_u64(seed) }
        }
    }

    impl RngProvider for FakeRngProvider {
        type RNG = rand::rngs::StdRng;
        fn get_rng(&mut self) -> &mut Self::RNG {
            &mut self.std_rng
        }
    }

    /// Provides a fake implementation of `Socket` using `mpsc` channels.
    ///
    /// Simply forwards pairs of (payload, address) over the channel. This means
    /// that the "sent to" address from the sender side is actually observed as
    /// the "received from" address on the receiver side.
    pub(crate) struct FakeSocket<T> {
        sender: mpsc::UnboundedSender<(Vec<u8>, T)>,
        receiver: Mutex<mpsc::UnboundedReceiver<(Vec<u8>, T)>>,
    }

    impl<T> FakeSocket<T> {
        pub(crate) fn new_pair() -> (FakeSocket<T>, FakeSocket<T>) {
            let (send_a, recv_a) = mpsc::unbounded();
            let (send_b, recv_b) = mpsc::unbounded();
            (
                FakeSocket { sender: send_a, receiver: Mutex::new(recv_b) },
                FakeSocket { sender: send_b, receiver: Mutex::new(recv_a) },
            )
        }
    }

    impl<T: Send> Socket<T> for FakeSocket<T> {
        async fn send_to(&self, buf: &[u8], addr: T) -> Result<(), SocketError> {
            let FakeSocket { sender, receiver: _ } = self;
            sender.clone().unbounded_send((buf.to_vec(), addr)).expect("unbounded_send error");
            Ok(())
        }

        async fn recv_from(&self, buf: &mut [u8]) -> Result<DatagramInfo<T>, SocketError> {
            let FakeSocket { receiver, sender: _ } = self;
            let mut receiver = receiver.lock().await;
            let (bytes, addr) = receiver.next().await.expect("TestSocket receiver closed");
            if buf.len() < bytes.len() {
                panic!("TestSocket receiver would produce short read")
            }
            (buf[..bytes.len()]).copy_from_slice(&bytes);
            Ok(DatagramInfo { length: bytes.len(), address: addr })
        }
    }

    impl<T, U> Socket<U> for T
    where
        T: AsRef<FakeSocket<U>>,
        U: Send + 'static,
    {
        async fn send_to(&self, buf: &[u8], addr: U) -> Result<(), SocketError> {
            self.as_ref().send_to(buf, addr).await
        }

        async fn recv_from(&self, buf: &mut [u8]) -> Result<DatagramInfo<U>, SocketError> {
            self.as_ref().recv_from(buf).await
        }
    }

    /// Fake socket provider implementation that vends out copies of
    /// the same `FakeSocket`.
    ///
    /// These copies will compete to receive and send on the same underlying
    /// `mpsc` channels.
    pub(crate) struct FakeSocketProvider<T, E> {
        /// The socket being vended out.
        pub(crate) socket: Rc<FakeSocket<T>>,

        /// If present, used to notify tests when the client binds new sockets.
        pub(crate) bound_events: Option<mpsc::UnboundedSender<E>>,
    }

    impl<T, E> FakeSocketProvider<T, E> {
        pub(crate) fn new(socket: FakeSocket<T>) -> Self {
            Self { socket: Rc::new(socket), bound_events: None }
        }

        pub(crate) fn new_with_events(
            socket: FakeSocket<T>,
            bound_events: mpsc::UnboundedSender<E>,
        ) -> Self {
            Self { socket: Rc::new(socket), bound_events: Some(bound_events) }
        }
    }

    impl PacketSocketProvider for FakeSocketProvider<net_types::ethernet::Mac, ()> {
        type Sock = Rc<FakeSocket<net_types::ethernet::Mac>>;
        async fn get_packet_socket(&self) -> Result<Self::Sock, SocketError> {
            let Self { socket, bound_events } = self;
            if let Some(bound_events) = bound_events {
                bound_events.unbounded_send(()).expect("events receiver should not be dropped");
            }
            Ok(socket.clone())
        }
    }

    impl UdpSocketProvider for FakeSocketProvider<std::net::SocketAddr, std::net::SocketAddr> {
        type Sock = Rc<FakeSocket<std::net::SocketAddr>>;
        async fn bind_new_udp_socket(
            &self,
            bound_addr: std::net::SocketAddr,
        ) -> Result<Self::Sock, SocketError> {
            let Self { socket, bound_events } = self;
            if let Some(bound_events) = bound_events {
                bound_events
                    .unbounded_send(bound_addr)
                    .expect("events receiver should not be dropped");
            }
            Ok(socket.clone())
        }
    }

    #[derive(Copy, Clone, PartialEq, Eq, Debug, PartialOrd, Ord)]
    pub(crate) struct TestInstant(pub(crate) std::time::Duration);

    impl InspectableInstant for TestInstant {
        fn record<I: diagnostics_traits::Inspector>(
            &self,
            name: InstantPropertyName,
            inspector: &mut I,
        ) {
            inspector.record_debug(name.into(), self);
        }
    }

    impl Instant for TestInstant {
        fn add(&self, duration: std::time::Duration) -> Self {
            Self(self.0.checked_add(duration).unwrap())
        }

        fn average(&self, other: Self) -> Self {
            let lower = self.0.min(other.0);
            let higher = self.0.max(other.0);
            Self(lower + (higher - lower) / 2)
        }
    }

    /// Fake implementation of `Time` that uses `std::time::Duration` as its
    /// `Instant` type.
    pub(crate) struct FakeTimeController {
        pub(super) timer_heap:
            BTreeMap<std::cmp::Reverse<std::time::Duration>, Vec<oneshot::Sender<()>>>,
        pub(super) current_time: std::time::Duration,
    }

    impl FakeTimeController {
        pub(crate) fn new() -> Rc<RefCell<FakeTimeController>> {
            Rc::new(RefCell::new(FakeTimeController {
                timer_heap: BTreeMap::default(),
                current_time: std::time::Duration::default(),
            }))
        }
    }

    /// Advances the "current time" encoded by `ctl` by `duration`. Any timers
    /// that were set at or before the resulting "current time" will fire.
    pub(crate) fn advance(ctl: &Rc<RefCell<FakeTimeController>>, duration: std::time::Duration) {
        let timers_to_fire = {
            let mut ctl = ctl.borrow_mut();
            let FakeTimeController { timer_heap, current_time } = ctl.deref_mut();
            let next_time = *current_time + duration;
            *current_time = next_time;
            timer_heap.split_off(&std::cmp::Reverse(next_time))
        };
        for (_, senders) in timers_to_fire {
            for sender in senders {
                match sender.send(()) {
                    Ok(()) => (),
                    Err(()) => {
                        // ignore, it's fine for the client core to drop a timer
                        // to cancel it
                    }
                }
            }
        }
    }

    pub(crate) fn run_until_next_timers_fire<F>(
        executor: &mut fasync::TestExecutor,
        time: &Rc<RefCell<FakeTimeController>>,
        main_future: &mut F,
    ) -> std::task::Poll<F::Output>
    where
        F: Future + Unpin,
    {
        let poll: std::task::Poll<_> = executor.run_until_stalled(main_future);
        if poll.is_ready() {
            return poll;
        }

        {
            let mut time = time.borrow_mut();
            let FakeTimeController { timer_heap, current_time } = time.deref_mut();

            // NOTE: the timer heap is ordered by Reverse<Duration> in order to
            // facilitate the implementation of `advance()` by making
            // `BTreeMap::split_off` have the right edge-case behavior. This
            // makes it easy to get it first_entry vs last_entry mixed up here,
            // though.
            let earliest_entry = timer_heap.last_entry().expect("no timers installed");

            let (Reverse(instant), senders) = earliest_entry.remove_entry();
            *current_time = instant;
            for sender in senders {
                match sender.send(()) {
                    Ok(()) => (),
                    Err(()) => {
                        // ignore, it's fine for the client core to drop a timer
                        // to cancel it
                    }
                }
            }
        }

        executor.run_until_stalled(main_future)
    }

    impl Clock for Rc<RefCell<FakeTimeController>> {
        type Instant = TestInstant;

        fn now(&self) -> Self::Instant {
            let ctl = self.borrow_mut();
            let FakeTimeController { timer_heap: _, current_time } = ctl.deref();
            TestInstant(*current_time)
        }

        async fn wait_until(&self, TestInstant(time): Self::Instant) {
            log::info!("registering timer at {:?}", time);
            let receiver = {
                let mut ctl = self.borrow_mut();
                let FakeTimeController { timer_heap, current_time } = ctl.deref_mut();
                if *current_time >= time {
                    return;
                }
                let (sender, receiver) = oneshot::channel();
                timer_heap.entry(std::cmp::Reverse(time)).or_default().push(sender);
                receiver
            };
            receiver.await.expect("shouldn't be cancelled")
        }
    }
}

#[cfg(test)]
mod test {
    use super::testutil::*;
    use super::*;
    use futures::channel::mpsc;
    use futures::{FutureExt, StreamExt};
    use net_declare::std_socket_addr;
    use rand::RngExt as _;
    use std::pin::pin;

    #[test]
    fn test_rng() {
        let make_sequence = |seed| {
            let mut rng = FakeRngProvider::new(seed);
            std::iter::from_fn(|| Some(rng.get_rng().random::<u32>())).take(5).collect::<Vec<_>>()
        };
        assert_eq!(
            make_sequence(42),
            make_sequence(42),
            "should provide identical sequences with same seed"
        );
        assert_ne!(
            make_sequence(42),
            make_sequence(999999),
            "should provide different sequences with different seeds"
        );
    }

    #[fuchsia::test(logging = false)]
    async fn test_socket() {
        let (a, b) = FakeSocket::new_pair();
        let to_send = [
            (b"hello".to_vec(), "1.2.3.4:5".to_string()),
            (b"test".to_vec(), "1.2.3.5:5".to_string()),
            (b"socket".to_vec(), "1.2.3.6:5".to_string()),
        ];

        let mut buf = [0u8; 10];
        for (msg, addr) in &to_send {
            a.send_to(msg, addr.clone()).await.unwrap();

            let DatagramInfo { length: n, address: received_addr } =
                b.recv_from(&mut buf).await.unwrap();
            assert_eq!(&received_addr, addr);
            assert_eq!(&buf[..n], msg);
        }

        let (a, b) = (b, a);
        for (msg, addr) in &to_send {
            a.send_to(msg, addr.clone()).await.unwrap();

            let DatagramInfo { length: n, address: received_addr } =
                b.recv_from(&mut buf).await.unwrap();
            assert_eq!(&received_addr, addr);
            assert_eq!(&buf[..n], msg);
        }
    }

    #[fuchsia::test(logging = false)]
    #[should_panic]
    async fn test_socket_panics_on_short_read() {
        let (a, b) = FakeSocket::new_pair();

        let mut buf = [0u8; 10];
        let message = b"this message is way longer than 10 bytes";
        a.send_to(message, "1.2.3.4:5".to_string()).await.unwrap();

        // Should panic here.
        let _: Result<_, _> = b.recv_from(&mut buf).await;
    }

    #[fuchsia::test(logging = false)]
    async fn test_fake_udp_socket_provider() {
        let (a, b) = FakeSocket::new_pair();
        let (events_sender, mut events_receiver) = mpsc::unbounded();
        let provider = FakeSocketProvider::new_with_events(b, events_sender);
        const ADDR_1: std::net::SocketAddr = std_socket_addr!("1.1.1.1:11");
        const ADDR_2: std::net::SocketAddr = std_socket_addr!("2.2.2.2:22");
        const ADDR_3: std::net::SocketAddr = std_socket_addr!("3.3.3.3:33");
        let b_1 = provider.bind_new_udp_socket(ADDR_1).await.expect("get packet socket");
        assert_eq!(
            events_receiver
                .next()
                .now_or_never()
                .expect("should have received bound event")
                .expect("stream should not have ended"),
            ADDR_1
        );

        let b_2 = provider.bind_new_udp_socket(ADDR_2).await.expect("get packet socket");
        assert_eq!(
            events_receiver
                .next()
                .now_or_never()
                .expect("should have received bound event")
                .expect("stream should not have ended"),
            ADDR_2
        );

        a.send_to(b"hello", ADDR_3).await.unwrap();
        a.send_to(b"world", ADDR_3).await.unwrap();

        let mut buf = [0u8; 5];
        let DatagramInfo { length, address } = b_1.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..length], b"hello");
        assert_eq!(address, ADDR_3);

        let DatagramInfo { length, address } = b_2.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..length], b"world");
        assert_eq!(address, ADDR_3);
    }

    #[fuchsia::test(logging = false)]
    async fn test_fake_packet_socket_provider() {
        let (a, b) = FakeSocket::new_pair();
        let (events_sender, mut events_receiver) = mpsc::unbounded();
        let provider = FakeSocketProvider::new_with_events(b, events_sender);
        let b_1 = provider.get_packet_socket().await.expect("get packet socket");
        events_receiver
            .next()
            .now_or_never()
            .expect("should have received bound event")
            .expect("stream should not have ended");

        let b_2 = provider.get_packet_socket().await.expect("get packet socket");
        events_receiver
            .next()
            .now_or_never()
            .expect("should have received bound event")
            .expect("stream should not have ended");

        const ADDRESS: net_types::ethernet::Mac = net_declare::net_mac!("01:02:03:04:05:06");

        a.send_to(b"hello", ADDRESS).await.unwrap();

        a.send_to(b"world", ADDRESS).await.unwrap();

        let mut buf = [0u8; 5];
        let DatagramInfo { length, address } = b_1.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..length], b"hello");
        assert_eq!(address, ADDRESS);

        let DatagramInfo { length, address } = b_2.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..length], b"world");
        assert_eq!(address, ADDRESS);
    }

    #[test]
    fn test_time_controller() {
        let time_ctl = FakeTimeController::new();
        assert!(time_ctl.borrow().timer_heap.is_empty());
        assert_eq!(time_ctl.borrow().current_time, std::time::Duration::from_secs(0));
        assert_eq!(time_ctl.now(), TestInstant(std::time::Duration::from_secs(0)));

        let mut timer_registered_before_should_fire_1 =
            pin!(time_ctl.wait_until(TestInstant(std::time::Duration::from_secs(1))));
        let mut timer_registered_before_should_fire_2 =
            pin!(time_ctl.wait_until(TestInstant(std::time::Duration::from_secs(1))));

        let mut timer_should_not_fire =
            pin!(time_ctl.wait_until(TestInstant(std::time::Duration::from_secs(10))));

        // Poll the timer futures once so that they have the chance to
        // register themselves in our timer heap.
        {
            let waker = std::task::Waker::noop();
            let mut context = futures::task::Context::from_waker(&waker);
            assert_eq!(
                timer_registered_before_should_fire_1.poll_unpin(&mut context),
                futures::task::Poll::Pending
            );
            assert_eq!(
                timer_registered_before_should_fire_2.poll_unpin(&mut context),
                futures::task::Poll::Pending
            );
            assert_eq!(
                timer_should_not_fire.poll_unpin(&mut context),
                futures::task::Poll::Pending
            );
        }

        {
            let time_ctl = time_ctl.borrow_mut();
            let entries = time_ctl.timer_heap.iter().collect::<Vec<_>>();
            assert_eq!(entries.len(), 2);

            let (time, senders) = entries[0];
            assert_eq!(time, &std::cmp::Reverse(std::time::Duration::from_secs(10)));
            assert_eq!(senders.len(), 1);

            let (time, senders) = entries[1];
            assert_eq!(time, &std::cmp::Reverse(std::time::Duration::from_secs(1)));
            assert_eq!(senders.len(), 2);
        }

        advance(&time_ctl, std::time::Duration::from_secs(1));

        assert_eq!(time_ctl.now(), TestInstant(std::time::Duration::from_secs(1)));
        {
            let time_ctl = time_ctl.borrow_mut();
            let entries = time_ctl.timer_heap.iter().collect::<Vec<_>>();
            assert_eq!(entries.len(), 1);
            let (time, senders) = entries[0];
            assert_eq!(time, &std::cmp::Reverse(std::time::Duration::from_secs(10)));
            assert_eq!(senders.len(), 1);
        }

        assert_eq!(timer_registered_before_should_fire_1.now_or_never(), Some(()));
        assert_eq!(timer_registered_before_should_fire_2.now_or_never(), Some(()));
        assert_eq!(timer_should_not_fire.now_or_never(), None);

        let timer_set_in_past = time_ctl.wait_until(TestInstant(std::time::Duration::from_secs(0)));
        assert_eq!(timer_set_in_past.now_or_never(), Some(()));

        let timer_set_for_present = time_ctl.wait_until(time_ctl.now());
        assert_eq!(timer_set_for_present.now_or_never(), Some(()));
    }
}
