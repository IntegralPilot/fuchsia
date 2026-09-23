// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Neighbor unreachability detection.

use alloc::collections::{BinaryHeap, VecDeque};
use alloc::vec::Vec;
use core::convert::Infallible as Never;
use core::fmt::Debug;
use core::hash::Hash;
use core::marker::PhantomData;
use core::num::{NonZeroU16, NonZeroU32};
use core::time::Duration;

use assert_matches::assert_matches;
use derivative::Derivative;
use log::{debug, error, warn};
use net_types::ip::{GenericOverIp, Ip, IpMarked, Ipv4, Ipv6};
use net_types::{SpecifiedAddr, UnicastAddr};
use netstack3_base::socket::{SocketIpAddr, SocketIpAddrExt as _};
use netstack3_base::{
    AddressResolutionFailed, AnyDevice, CoreTimerContext, Counter, CounterContext, DeviceIdContext,
    ErrorAndSerializer, EventContext, HandleableTimer, Instant, InstantBindingsTypes, LinkDevice,
    LinkDeviceAddress, LocalTimerHeap, NetworkSerializationContext, NetworkSerializer,
    SendFrameError, SendFrameErrorReason, StrongDeviceIdentifier, TimerBindingsTypes, TimerContext,
    TxMetadataBindingsTypes, WeakDeviceIdentifier,
};
use netstack3_hashmap::hash_map::{Entry, HashMap, OccupiedEntry};
use packet::{
    Buf, BufferMut, GrowBuffer as _, ParsablePacket as _, ParseBufferMut as _, SerializeError,
};
use packet_formats::ip::IpPacket as _;
use packet_formats::ipv4::{Ipv4FragmentType, Ipv4Header as _, Ipv4Packet};
use packet_formats::ipv6::Ipv6Packet;
use packet_formats::utils::NonZeroDuration;
use static_assertions::const_assert;
use thiserror::Error;
use zerocopy::SplitByteSlice;

pub(crate) mod api;

/// The default maximum number of multicast solicitations as defined in [RFC
/// 4861 section 10].
///
/// [RFC 4861 section 10]: https://tools.ietf.org/html/rfc4861#section-10
pub(crate) const DEFAULT_MAX_MULTICAST_SOLICIT: NonZeroU16 = NonZeroU16::new(3).unwrap();

/// The default maximum number of unicast solicitations as defined in [RFC 4861
/// section 10].
///
/// [RFC 4861 section 10]: https://tools.ietf.org/html/rfc4861#section-10
const DEFAULT_MAX_UNICAST_SOLICIT: NonZeroU16 = NonZeroU16::new(3).unwrap();

/// The maximum amount of time between retransmissions of neighbor probe
/// messages as defined in [RFC 7048 section 4].
///
/// [RFC 7048 section 4]: https://tools.ietf.org/html/rfc7048#section-4
const MAX_RETRANS_TIMER: NonZeroDuration = NonZeroDuration::from_secs(60).unwrap();

/// The default value for *RetransTimer* as defined in [RFC 4861 section 10].
///
/// Note that both ARP (IPv4) and NDP (IPv6) uses this default value to align
/// behavior between IPv4 and IPv6 and simplify testing.
///
/// [RFC 4861 section 10]: https://tools.ietf.org/html/rfc4861#section-10
pub const DEFAULT_RETRANS_TIMER: NonZeroDuration = NonZeroDuration::from_secs(1).unwrap();

/// The exponential backoff factor for retransmissions of multicast neighbor
/// probe messages as defined in [RFC 7048 section 4].
///
/// [RFC 7048 section 4]: https://tools.ietf.org/html/rfc7048#section-4
const BACKOFF_MULTIPLE: NonZeroU32 = NonZeroU32::new(3).unwrap();

const MAX_PENDING_FRAMES: usize = 10;

/// The time a neighbor is considered reachable after receiving a reachability
/// confirmation, as defined in [RFC 4861 section 6.3.2].
///
/// [RFC 4861 section 6.3.2]: https://tools.ietf.org/html/rfc4861#section-6.3.2
const DEFAULT_BASE_REACHABLE_TIME: NonZeroDuration = NonZeroDuration::from_secs(30).unwrap();

/// The time after which a neighbor in the DELAY state transitions to PROBE, as
/// defined in [RFC 4861 section 10].
///
/// [RFC 4861 section 10]: https://tools.ietf.org/html/rfc4861#section-10
const DELAY_FIRST_PROBE_TIME: NonZeroDuration = NonZeroDuration::from_secs(5).unwrap();

/// The garbage collection threshold for the neighbor table for a given device.
/// When the number of entries is above this number and an entry transitions
/// into a discardable state, a garbage collection task will be scheduled to
/// remove any entries that are not in use.
pub const GC_THRESHOLD: usize = 512;

/// The maximum number of neighbor entries in the neighbor table for a given
/// device. When the number of entries reaches this number, new entries can no
/// longer be inserted.
pub const MAX_ENTRIES: usize = 1024;
const_assert!(MAX_ENTRIES > GC_THRESHOLD);

/// The minimum amount of time between garbage collection passes when the
/// neighbor table grows beyond `GC_THRESHOLD`.
const MIN_GARBAGE_COLLECTION_INTERVAL: NonZeroDuration = NonZeroDuration::from_secs(30).unwrap();

/// NUD counters.
#[derive(Default)]
pub struct NudCountersInner {
    /// Count of ICMP destination unreachable errors that could not be sent.
    pub icmp_dest_unreachable_dropped: Counter,
}

/// NUD counters.
pub type NudCounters<I> = IpMarked<I, NudCountersInner>;

/// Neighbor confirmation flags.
#[derive(Debug, Copy, Clone)]
pub struct ConfirmationFlags {
    /// True if neighbor was explicitly solicited.
    pub solicited_flag: bool,
    /// True if must override neighbor entry.
    pub override_flag: bool,
}

/// The type of message with a dynamic neighbor update.
#[derive(Debug, Copy, Clone)]
pub enum DynamicNeighborUpdateSource<A> {
    /// Indicates an update from a neighbor probe message.
    ///
    /// E.g. NDP Neighbor Solicitation.
    Probe {
        /// The source link-layer address option.
        link_address: UnicastAddr<A>,
    },

    /// Indicates an update from a neighbor confirmation message.
    ///
    /// E.g. NDP Neighbor Advertisement.
    Confirmation {
        /// The target link-layer address option.
        link_address: Option<UnicastAddr<A>>,
        /// The flags set on the neighbor confirmation.
        flags: ConfirmationFlags,
    },
}

/// A neighbor's state.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
#[cfg_attr(
    any(test, feature = "testutils"),
    derivative(
        Clone(bound = "BT::TxMetadata: Clone"),
        PartialEq(bound = "BT::TxMetadata: PartialEq"),
        Eq(bound = "BT::TxMetadata: Eq")
    )
)]
#[allow(missing_docs)]
pub enum NeighborState<D: LinkDevice, BT: NudBindingsTypes<D>> {
    Dynamic(DynamicNeighborState<D, BT>),
    Static(UnicastAddr<D::Address>),
}

/// The state of a dynamic entry in the neighbor cache within the Neighbor
/// Unreachability Detection state machine, defined in [RFC 4861 section 7.3.2]
/// and [RFC 7048 section 3].
///
/// [RFC 4861 section 7.3.2]: https://tools.ietf.org/html/rfc4861#section-7.3.2
/// [RFC 7048 section 3]: https://tools.ietf.org/html/rfc7048#section-3
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
#[cfg_attr(
    any(test, feature = "testutils"),
    derivative(
        Clone(bound = "BT::TxMetadata: Clone"),
        PartialEq(bound = "BT::TxMetadata: PartialEq"),
        Eq(bound = "BT::TxMetadata: Eq")
    )
)]
pub enum DynamicNeighborState<D: LinkDevice, BT: NudBindingsTypes<D>> {
    /// Address resolution is being performed on the entry.
    ///
    /// Specifically, a probe has been sent to the solicited-node multicast
    /// address of the target, but the corresponding confirmation has not yet
    /// been received.
    Incomplete(Incomplete<D, BT::Notifier, BT::TxMetadata>),

    /// Positive confirmation was received within the last ReachableTime
    /// milliseconds that the forward path to the neighbor was functioning
    /// properly. While `Reachable`, no special action takes place as packets
    /// are sent.
    Reachable(Reachable<D, BT::Instant>),

    /// More than ReachableTime milliseconds have elapsed since the last
    /// positive confirmation was received that the forward path was functioning
    /// properly. While stale, no action takes place until a packet is sent.
    ///
    /// The `Stale` state is entered upon receiving an unsolicited neighbor
    /// message that updates the cached link-layer address. Receipt of such a
    /// message does not confirm reachability, and entering the `Stale` state
    /// ensures reachability is verified quickly if the entry is actually being
    /// used. However, reachability is not actually verified until the entry is
    /// actually used.
    Stale(Stale<D>),

    /// A packet has been recently sent to the neighbor, which has stale
    /// reachability information (i.e. we have not received recent positive
    /// confirmation that the forward path is functioning properly).
    ///
    /// The `Delay` state is an optimization that gives upper-layer protocols
    /// additional time to provide reachability confirmation in those cases
    /// where ReachableTime milliseconds have passed since the last confirmation
    /// due to lack of recent traffic. Without this optimization, the opening of
    /// a TCP connection after a traffic lull would initiate probes even though
    /// the subsequent three-way handshake would provide a reachability
    /// confirmation almost immediately.
    Delay(Delay<D>),

    /// A reachability confirmation is actively sought by retransmitting probes
    /// every RetransTimer milliseconds until a reachability confirmation is
    /// received.
    Probe(Probe<D>),

    /// Similarly to the `Probe` state, a reachability confirmation is actively
    /// sought by retransmitting probes; however, probes are multicast to the
    /// solicited-node multicast address, using a timeout with exponential
    /// backoff, rather than unicast to the cached link address. Also, probes
    /// are only transmitted as long as packets continue to be sent to the
    /// neighbor.
    Unreachable(Unreachable<D>),
}

/// The state of dynamic neighbor table entries as published via events.
///
/// Note that this is not how state is held in the neighbor table itself,
/// see [`DynamicNeighborState`].
///
/// Modeled after RFC 4861 section 7.3.2. Descriptions are kept
/// implementation-independent by using a set of generic terminology.
///
/// ,------------------------------------------------------------------.
/// | Generic Term              | ARP Term    | NDP Term               |
/// |---------------------------+-------------+------------------------|
/// | Reachability Probe        | ARP Request | Neighbor Solicitation  |
/// | Reachability Confirmation | ARP Reply   | Neighbor Advertisement |
/// `---------------------------+-------------+------------------------'
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EventDynamicState<L: LinkDeviceAddress> {
    /// Reachability is in the process of being confirmed for a newly
    /// created entry.
    Incomplete,
    /// Forward reachability has been confirmed; the path to the neighbor
    /// is functioning properly.
    Reachable(UnicastAddr<L>),
    /// Reachability is considered unknown.
    ///
    /// Occurs in one of two ways:
    ///   1. Too much time has elapsed since the last positive reachability
    ///      confirmation was received.
    ///   2. Received a reachability confirmation from a neighbor with a
    ///      different MAC address than the one cached.
    Stale(UnicastAddr<L>),
    /// A packet was recently sent while reachability was considered
    /// unknown.
    ///
    /// This state is an optimization that gives non-Neighbor-Discovery
    /// related protocols time to confirm reachability after the last
    /// confirmation of reachability has expired due to lack of recent
    /// traffic.
    Delay(UnicastAddr<L>),
    /// A reachability confirmation is actively sought by periodically
    /// retransmitting unicast reachability probes until a reachability
    /// confirmation is received, or until the maximum number of probes has
    /// been sent.
    Probe(UnicastAddr<L>),
    /// Target is considered unreachable. A reachability confirmation was not
    /// received after transmitting the maximum number of reachability
    /// probes.
    Unreachable(UnicastAddr<L>),
}

/// Neighbor state published via events.
///
/// Note that this is not how state is held in the neighbor table itself,
/// see [`NeighborState`].
///
/// Either a dynamic state within the Neighbor Unreachability Detection (NUD)
/// state machine, or a static entry that never expires.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum EventState<L: LinkDeviceAddress> {
    /// Dynamic neighbor state.
    Dynamic(EventDynamicState<L>),
    /// Static neighbor state.
    Static(UnicastAddr<L>),
}

/// Neighbor event kind.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum EventKind<L: LinkDeviceAddress> {
    /// A neighbor entry was added.
    Added(EventState<L>),
    /// A neighbor entry has changed.
    Changed(EventState<L>),
    /// A neighbor entry was removed.
    Removed,
}

/// Neighbor event.
#[derive(Debug, Eq, Hash, PartialEq, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct Event<L: LinkDeviceAddress, DeviceId, I: Ip, Instant> {
    /// The device.
    pub device: DeviceId,
    /// The neighbor's address.
    pub addr: SpecifiedAddr<I::Addr>,
    /// The kind of this neighbor event.
    pub kind: EventKind<L>,
    /// Time of this event.
    pub at: Instant,
}

impl<L: LinkDeviceAddress, DeviceId, I: Ip, Instant> Event<L, DeviceId, I, Instant> {
    /// Changes the device id type with `map`.
    pub fn map_device<N, F: FnOnce(DeviceId) -> N>(self, map: F) -> Event<L, N, I, Instant> {
        let Self { device, kind, addr, at } = self;
        Event { device: map(device), kind, addr, at }
    }
}

impl<L: LinkDeviceAddress, DeviceId: Clone, I: Ip, Instant> Event<L, DeviceId, I, Instant> {
    fn changed(
        device: &DeviceId,
        event_state: EventState<L>,
        addr: SpecifiedAddr<I::Addr>,
        at: Instant,
    ) -> Self {
        Self { device: device.clone(), kind: EventKind::Changed(event_state), addr, at }
    }

    fn added(
        device: &DeviceId,
        event_state: EventState<L>,
        addr: SpecifiedAddr<I::Addr>,
        at: Instant,
    ) -> Self {
        Self { device: device.clone(), kind: EventKind::Added(event_state), addr, at }
    }

    fn removed(device: &DeviceId, addr: SpecifiedAddr<I::Addr>, at: Instant) -> Self {
        Self { device: device.clone(), kind: EventKind::Removed, addr, at }
    }
}

fn schedule_timer_if_should_retransmit<I, D, DeviceId, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    timers: &mut TimerHeap<I, BC>,
    neighbor: SpecifiedAddr<I::Addr>,
    event: NudEvent,
    counter: &mut Option<NonZeroU16>,
) -> bool
where
    I: Ip,
    D: LinkDevice,
    DeviceId: StrongDeviceIdentifier,
    BC: NudBindingsContext<I, D, DeviceId>,
    CC: NudConfigContext<I>,
{
    match counter {
        Some(c) => {
            *counter = NonZeroU16::new(c.get() - 1);
            let retransmit_timeout = core_ctx.retransmit_timeout();
            timers.schedule_neighbor(bindings_ctx, retransmit_timeout, neighbor, event);
            true
        }
        None => false,
    }
}

/// The state for an incomplete neighbor entry.
#[derive(Debug, Derivative)]
#[cfg_attr(any(test, feature = "testutils"), derivative(PartialEq(bound = "M: PartialEq"), Eq))]
pub struct Incomplete<D: LinkDevice, N: LinkResolutionNotifier<D>, M> {
    transmit_counter: Option<NonZeroU16>,
    pending_frames: VecDeque<(Buf<Vec<u8>>, M)>,
    #[derivative(PartialEq = "ignore")]
    notifiers: Vec<N>,
    _marker: PhantomData<D>,
}

#[cfg(any(test, feature = "testutils"))]
impl<D: LinkDevice, N: LinkResolutionNotifier<D>, M: Clone> Clone for Incomplete<D, N, M> {
    fn clone(&self) -> Self {
        // Do not clone `notifiers` since the LinkResolutionNotifier type is not
        // required to implement `Clone` and notifiers are not used in equality
        // checks in tests.
        let Self { transmit_counter, pending_frames, notifiers: _, _marker } = self;
        Self {
            transmit_counter: transmit_counter.clone(),
            pending_frames: pending_frames.clone(),
            notifiers: Vec::new(),
            _marker: PhantomData,
        }
    }
}

impl<D: LinkDevice, N: LinkResolutionNotifier<D>, M> Drop for Incomplete<D, N, M> {
    fn drop(&mut self) {
        let Self { transmit_counter: _, pending_frames: _, notifiers, _marker } = self;
        for notifier in notifiers.drain(..) {
            notifier.notify(Err(AddressResolutionFailed));
        }
    }
}

impl<D: LinkDevice, N: LinkResolutionNotifier<D>, M> Incomplete<D, N, M> {
    /// Creates a new `Incomplete` entry with `pending_frames` and remaining
    /// transmits `transmit_counter`.
    #[cfg(any(test, feature = "testutils"))]
    pub fn new_with_pending_frames_and_transmit_counter(
        pending_frames: VecDeque<(Buf<Vec<u8>>, M)>,
        transmit_counter: Option<NonZeroU16>,
    ) -> Self {
        Self {
            transmit_counter,
            pending_frames,
            notifiers: Default::default(),
            _marker: PhantomData,
        }
    }

    fn new<I, CC, BC, DeviceId>(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        neighbor: SpecifiedAddr<I::Addr>,
    ) -> Self
    where
        I: Ip,
        D: LinkDevice,
        BC: NudBindingsContext<I, D, DeviceId>,
        CC: NudConfigContext<I>,
        DeviceId: StrongDeviceIdentifier,
    {
        let mut this = Incomplete {
            transmit_counter: Some(core_ctx.max_multicast_solicit()),
            pending_frames: VecDeque::new(),
            notifiers: Vec::new(),
            _marker: PhantomData,
        };
        // NB: transmission of a neighbor probe on entering INCOMPLETE (and subsequent
        // retransmissions) is done by `handle_timer`, as it need not be done with the
        // neighbor table lock held.
        assert!(this.schedule_timer_if_should_retransmit(core_ctx, bindings_ctx, timers, neighbor));

        this
    }

    fn new_with_notifier<I, CC, BC, DeviceId>(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        neighbor: SpecifiedAddr<I::Addr>,
        notifier: BC::Notifier,
    ) -> Self
    where
        I: Ip,
        D: LinkDevice,
        BC: NudBindingsContext<I, D, DeviceId, Notifier = N>,
        CC: NudConfigContext<I>,
        DeviceId: StrongDeviceIdentifier,
    {
        let mut this = Incomplete {
            transmit_counter: Some(core_ctx.max_multicast_solicit()),
            pending_frames: VecDeque::new(),
            notifiers: [notifier].into(),
            _marker: PhantomData,
        };
        // NB: transmission of a neighbor probe on entering INCOMPLETE (and subsequent
        // retransmissions) is done by `handle_timer`, as it need not be done with the
        // neighbor table lock held.
        assert!(this.schedule_timer_if_should_retransmit(core_ctx, bindings_ctx, timers, neighbor));

        this
    }

    fn schedule_timer_if_should_retransmit<I, DeviceId, CC, BC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        neighbor: SpecifiedAddr<I::Addr>,
    ) -> bool
    where
        I: Ip,
        D: LinkDevice,
        DeviceId: StrongDeviceIdentifier,
        BC: NudBindingsContext<I, D, DeviceId>,
        CC: NudConfigContext<I>,
    {
        let Self { transmit_counter, pending_frames: _, notifiers: _, _marker } = self;
        schedule_timer_if_should_retransmit(
            core_ctx,
            bindings_ctx,
            timers,
            neighbor,
            NudEvent::RetransmitMulticastProbe,
            transmit_counter,
        )
    }

    fn queue_packet<B, S>(
        &mut self,
        body: S,
        meta: M,
    ) -> Result<(), ErrorAndSerializer<SerializeError<Never>, S>>
    where
        B: BufferMut,
        S: NetworkSerializer<Buffer = B>,
    {
        let Self { pending_frames, transmit_counter: _, notifiers: _, _marker } = self;

        // We don't accept new packets when the queue is full because earlier packets
        // are more likely to initiate connections whereas later packets are more likely
        // to carry data. E.g. A TCP SYN/SYN-ACK is likely to appear before a TCP
        // segment with data and dropping the SYN/SYN-ACK may result in the TCP peer not
        // processing the segment with data since the segment completing the handshake
        // has not been received and handled yet.
        if pending_frames.len() < MAX_PENDING_FRAMES {
            pending_frames.push_back((
                body.serialize_vec_outer(&mut NetworkSerializationContext::default())
                    .map_err(|(error, serializer)| ErrorAndSerializer { error, serializer })?
                    .map_a(|b| Buf::new(b.as_ref().to_vec(), ..))
                    .into_inner(),
                meta,
            ));
        }
        Ok(())
    }

    /// Flush pending packets to the resolved link address and notify any observers
    /// that link address resolution is complete.
    fn complete_resolution<I, CC, BC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        link_address: UnicastAddr<D::Address>,
    ) where
        I: Ip,
        D: LinkDevice,
        BC: NudBindingsContext<I, D, CC::DeviceId, TxMetadata = M>,
        CC: NudSenderContext<I, D, BC>,
    {
        let Self { pending_frames, notifiers, transmit_counter: _, _marker } = self;

        // Send out pending packets while holding the NUD lock to prevent a potential
        // ordering violation.
        //
        // If we drop the NUD lock before sending out these queued packets, another
        // thread could take the NUD lock, observe that neighbor resolution is complete,
        // and send a packet *before* these pending packets are sent out, resulting in
        // out-of-order transmission to the device.
        for (body, meta) in pending_frames.drain(..) {
            // Ignore any errors on sending the IP packet, because a failure at this point
            // is not actionable for the caller: failing to send a previously-queued packet
            // doesn't mean that updating the neighbor entry should fail.
            core_ctx
                .send_ip_packet_to_neighbor_link_addr(bindings_ctx, link_address, body, meta)
                .unwrap_or_else(|err| {
                    error!("failed to send pending IP packet to neighbor {link_address:?} {err:?}")
                })
        }
        for notifier in notifiers.drain(..) {
            notifier.notify(Ok(link_address));
        }
    }
}

/// State associated with a reachable neighbor.
#[derive(Debug, Derivative)]
#[cfg_attr(any(test, feature = "testutils"), derivative(Clone, PartialEq, Eq))]
pub struct Reachable<D: LinkDevice, I: Instant> {
    /// The resolved link address.
    pub link_address: UnicastAddr<D::Address>,
    /// The last confirmed instant.
    pub last_confirmed_at: I,
}

/// State associated with a stale neighbor.
#[derive(Debug, Derivative)]
#[cfg_attr(any(test, feature = "testutils"), derivative(Clone, PartialEq, Eq))]
pub struct Stale<D: LinkDevice> {
    /// The resolved link address.
    pub link_address: UnicastAddr<D::Address>,
}

impl<D: LinkDevice> Stale<D> {
    fn enter_delay<I, BC, DeviceId: Clone>(
        &mut self,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        neighbor: SpecifiedAddr<I::Addr>,
    ) -> Delay<D>
    where
        I: Ip,
        BC: NudBindingsContext<I, D, DeviceId>,
    {
        let Self { link_address } = *self;

        // Start a timer to transition into PROBE after DELAY_FIRST_PROBE seconds if no
        // packets are sent to this neighbor.
        timers.schedule_neighbor(
            bindings_ctx,
            DELAY_FIRST_PROBE_TIME,
            neighbor,
            NudEvent::DelayFirstProbe,
        );

        Delay { link_address }
    }
}

/// State associated with a neighbor in delay state.
#[derive(Debug, Derivative)]
#[cfg_attr(any(test, feature = "testutils"), derivative(Clone, PartialEq, Eq))]
pub struct Delay<D: LinkDevice> {
    /// The resolved link address.
    pub link_address: UnicastAddr<D::Address>,
}

impl<D: LinkDevice> Delay<D> {
    fn enter_probe<I, DeviceId, CC, BC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        neighbor: SpecifiedAddr<I::Addr>,
    ) -> Probe<D>
    where
        I: Ip,
        DeviceId: StrongDeviceIdentifier,
        BC: NudBindingsContext<I, D, DeviceId>,
        CC: NudConfigContext<I>,
    {
        let Self { link_address } = *self;

        // NB: transmission of a neighbor probe on entering PROBE (and subsequent
        // retransmissions) is done by `handle_timer`, as it need not be done with the
        // neighbor table lock held.
        let retransmit_timeout = core_ctx.retransmit_timeout();
        timers.schedule_neighbor(
            bindings_ctx,
            retransmit_timeout,
            neighbor,
            NudEvent::RetransmitUnicastProbe,
        );

        Probe {
            link_address,
            transmit_counter: NonZeroU16::new(core_ctx.max_unicast_solicit().get() - 1),
        }
    }
}

#[derive(Debug, Derivative)]
#[cfg_attr(any(test, feature = "testutils"), derivative(Clone, PartialEq, Eq))]
pub struct Probe<D: LinkDevice> {
    link_address: UnicastAddr<D::Address>,
    transmit_counter: Option<NonZeroU16>,
}

impl<D: LinkDevice> Probe<D> {
    fn schedule_timer_if_should_retransmit<I, DeviceId, CC, BC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        neighbor: SpecifiedAddr<I::Addr>,
    ) -> bool
    where
        I: Ip,
        DeviceId: StrongDeviceIdentifier,
        BC: NudBindingsContext<I, D, DeviceId>,
        CC: NudConfigContext<I>,
    {
        let Self { link_address: _, transmit_counter } = self;
        schedule_timer_if_should_retransmit(
            core_ctx,
            bindings_ctx,
            timers,
            neighbor,
            NudEvent::RetransmitUnicastProbe,
            transmit_counter,
        )
    }

    fn enter_unreachable<I, BC, DeviceId>(
        &mut self,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        num_entries: usize,
        gc_state: &mut GarbageCollectionState<BC::Instant>,
    ) -> Unreachable<D>
    where
        I: Ip,
        BC: NudBindingsContext<I, D, DeviceId>,
        DeviceId: Clone,
    {
        // This entry is deemed discardable now that it is not in active use; schedule
        // garbage collection for the neighbor table if we are currently over the
        // maximum amount of entries.
        timers.maybe_schedule_gc(bindings_ctx, num_entries, gc_state);

        let Self { link_address, transmit_counter: _ } = self;
        Unreachable { link_address: *link_address, mode: UnreachableMode::WaitingForPacketSend }
    }
}

#[derive(Debug, Derivative)]
#[cfg_attr(any(test, feature = "testutils"), derivative(Clone, PartialEq, Eq))]
pub struct Unreachable<D: LinkDevice> {
    link_address: UnicastAddr<D::Address>,
    mode: UnreachableMode,
}

/// The dynamic neighbor state specific to the UNREACHABLE state as defined in
/// [RFC 7048].
///
/// When a neighbor entry transitions to UNREACHABLE, the netstack will stop
/// actively retransmitting probes if no packets are being sent to the neighbor.
///
/// If packets are sent through the neighbor, the netstack will continue to
/// retransmit multicast probes, but with exponential backoff on the timer,
/// based on the `BACKOFF_MULTIPLE` and clamped at `MAX_RETRANS_TIMER`.
///
/// [RFC 7048]: https://tools.ietf.org/html/rfc7048
#[derive(Debug, Clone, Copy, Derivative)]
#[cfg_attr(any(test, feature = "testutils"), derivative(PartialEq, Eq))]
pub(crate) enum UnreachableMode {
    WaitingForPacketSend,
    Backoff { probes_sent: NonZeroU32, packet_sent: bool },
}

impl UnreachableMode {
    /// The amount of time to wait before transmitting another multicast probe
    /// to the cached link address, based on how many probes we have transmitted
    /// so far, as defined in [RFC 7048 section 4].
    ///
    /// [RFC 7048 section 4]: https://tools.ietf.org/html/rfc7048#section-4
    fn next_backoff_retransmit_timeout<I, CC>(&self, core_ctx: &mut CC) -> NonZeroDuration
    where
        I: Ip,
        CC: NudConfigContext<I>,
    {
        let probes_sent = match self {
            UnreachableMode::Backoff { probes_sent, packet_sent: _ } => probes_sent,
            UnreachableMode::WaitingForPacketSend => {
                panic!("cannot calculate exponential backoff in state {self:?}")
            }
        };
        // TODO(https://fxbug.dev/42083368): vary this retransmit timeout by some random
        // "jitter factor" to avoid synchronization of transmissions from different
        // hosts.
        (core_ctx.retransmit_timeout() * BACKOFF_MULTIPLE.saturating_pow(probes_sent.get()))
            .min(MAX_RETRANS_TIMER)
    }
}

impl<D: LinkDevice> Unreachable<D> {
    fn handle_timer<I, DeviceId, CC, BC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        device_id: &DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
    ) -> Option<TransmitProbe<UnicastAddr<D::Address>>>
    where
        I: Ip,
        DeviceId: StrongDeviceIdentifier,
        BC: NudBindingsContext<I, D, DeviceId>,
        CC: NudConfigContext<I>,
    {
        let Self { link_address: _, mode } = self;
        match mode {
            UnreachableMode::WaitingForPacketSend => {
                panic!(
                    "timer should not have fired in UNREACHABLE while waiting for packet send; got \
                    a retransmit multicast probe event for {neighbor} on {device_id:?}",
                );
            }
            UnreachableMode::Backoff { probes_sent, packet_sent } => {
                if *packet_sent {
                    // It is all but guaranteed that we will never end up transmitting u32::MAX
                    // probes, given the retransmit timeout backs off to MAX_RETRANS_TIMER (1 minute
                    // by default), and u32::MAX minutes is over 8,000 years. By then we almost
                    // certainly would have garbage-collected the neighbor entry.
                    //
                    // But we do a saturating add just to be safe.
                    *probes_sent = probes_sent.saturating_add(1);
                    *packet_sent = false;

                    let duration = mode.next_backoff_retransmit_timeout(core_ctx);
                    timers.schedule_neighbor(
                        bindings_ctx,
                        duration,
                        neighbor,
                        NudEvent::RetransmitMulticastProbe,
                    );

                    Some(TransmitProbe::Multicast)
                } else {
                    *mode = UnreachableMode::WaitingForPacketSend;

                    None
                }
            }
        }
    }

    /// Advance the UNREACHABLE state machine based on a packet being queued for
    /// transmission.
    ///
    /// Returns whether a multicast neighbor probe should be sent as a result.
    fn handle_packet_queued_to_send<I, DeviceId, CC, BC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        neighbor: SpecifiedAddr<I::Addr>,
    ) -> bool
    where
        I: Ip,
        DeviceId: StrongDeviceIdentifier,
        BC: NudBindingsContext<I, D, DeviceId>,
        CC: NudConfigContext<I>,
    {
        let Self { link_address: _, mode } = self;
        match mode {
            UnreachableMode::WaitingForPacketSend => {
                // We already transmitted MAX_MULTICAST_SOLICIT probes to the neighbor
                // without confirmation, but now a packet is being sent to that neighbor, so
                // we are resuming transmission of probes for as long as packets continue to
                // be sent to the neighbor. Instead of retransmitting on a fixed timeout,
                // use exponential backoff per [RFC 7048 section 4]:
                //
                //   If an implementation transmits more than MAX_UNICAST_SOLICIT/
                //   MAX_MULTICAST_SOLICIT packets, then it SHOULD use the exponential
                //   backoff of the retransmit timer.  This is to avoid any significant
                //   load due to a steady background level of retransmissions from
                //   implementations that retransmit a large number of Neighbor
                //   Solicitations (NS) before discarding the NCE.
                //
                // [RFC 7048 section 4]: https://tools.ietf.org/html/rfc7048#section-4
                let probes_sent = NonZeroU32::new(1).unwrap();
                *mode = UnreachableMode::Backoff { probes_sent, packet_sent: false };

                let duration = mode.next_backoff_retransmit_timeout(core_ctx);
                timers.schedule_neighbor(
                    bindings_ctx,
                    duration,
                    neighbor,
                    NudEvent::RetransmitMulticastProbe,
                );

                // Transmit a multicast probe.
                true
            }
            UnreachableMode::Backoff { probes_sent: _, packet_sent } => {
                // We are in the exponential backoff phase of sending probes. Make a note
                // that a packet was sent since the last transmission so that we will send
                // another when the timer fires.
                *packet_sent = true;

                false
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
pub(crate) enum EnterProbeError {
    #[error("link address is unknown")]
    LinkAddressUnknown,
}

impl<D: LinkDevice, BT: NudBindingsTypes<D>> NeighborState<D, BT> {
    fn to_event_state(&self) -> EventState<D::Address> {
        match self {
            NeighborState::Dynamic(dynamic_state) => {
                EventState::Dynamic(dynamic_state.to_event_dynamic_state())
            }
            NeighborState::Static(addr) => EventState::Static(*addr),
        }
    }

    /// Transitions this neighbor to the Probe state. If a state change
    /// resulted, returns the link address that should be probed by the caller
    /// once the neighbor table lock is released.
    ///
    /// NB: if `neighbor` is a static entry, this will cause it to become and
    /// remain a dynamic entry.
    ///
    /// Returns an error if this neighbor is in the Incomplete state because the
    /// link address must be known in order to trigger a unicast probe.
    pub(crate) fn enter_probe<I, DeviceId, CC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BT,
        timers: &mut TimerHeap<I, BT>,
        neighbor: SpecifiedAddr<I::Addr>,
        device_id: &DeviceId,
    ) -> Result<Option<UnicastAddr<D::Address>>, EnterProbeError>
    where
        I: Ip,
        DeviceId: StrongDeviceIdentifier,
        BT: NudBindingsContext<I, D, DeviceId>,
        CC: NudConfigContext<I>,
    {
        let link_addr = match self {
            NeighborState::Dynamic(dynamic_state) => {
                let link_addr = match dynamic_state {
                    DynamicNeighborState::Reachable(Reachable { link_address, .. }) => {
                        *link_address
                    }
                    DynamicNeighborState::Stale(Stale { link_address }) => *link_address,
                    DynamicNeighborState::Delay(Delay { link_address }) => *link_address,
                    DynamicNeighborState::Unreachable(Unreachable { link_address, .. }) => {
                        *link_address
                    }
                    // The neighbor cannot be probed because its link address is
                    // unknown.
                    DynamicNeighborState::Incomplete(_) => {
                        return Err(EnterProbeError::LinkAddressUnknown);
                    }
                    // The neighbor probe is already in progress; do not send
                    // another probe.
                    DynamicNeighborState::Probe(_) => return Ok(None),
                };

                // Cancel any existing timers in preparation for scheduling the
                // retransmit timer.
                dynamic_state.cancel_timer(bindings_ctx, timers, neighbor);
                link_addr
            }
            NeighborState::Static(link_address) => *link_address,
        };

        // Schedule retransmit.
        let retransmit_timeout = core_ctx.retransmit_timeout();
        timers.schedule_neighbor(
            bindings_ctx,
            retransmit_timeout,
            neighbor,
            NudEvent::RetransmitUnicastProbe,
        );

        // Transition state.
        *self = NeighborState::Dynamic(DynamicNeighborState::Probe(Probe {
            link_address: link_addr,
            transmit_counter: NonZeroU16::new(core_ctx.max_unicast_solicit().get() - 1),
        }));
        let event_state = self.to_event_state();
        bindings_ctx.on_event(Event::changed(
            &device_id,
            event_state,
            neighbor,
            bindings_ctx.now(),
        ));
        Ok(Some(link_addr))
    }
}

impl<D: LinkDevice, BC: NudBindingsTypes<D>> DynamicNeighborState<D, BC> {
    fn cancel_timer<I, DeviceId>(
        &mut self,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        neighbor: SpecifiedAddr<I::Addr>,
    ) where
        I: Ip,
        DeviceId: StrongDeviceIdentifier,
        BC: NudBindingsContext<I, D, DeviceId>,
    {
        let expected_event = match self {
            DynamicNeighborState::Incomplete(Incomplete {
                transmit_counter: _,
                pending_frames: _,
                notifiers: _,
                _marker,
            }) => Some(NudEvent::RetransmitMulticastProbe),
            DynamicNeighborState::Reachable(Reachable {
                link_address: _,
                last_confirmed_at: _,
            }) => Some(NudEvent::ReachableTime),
            DynamicNeighborState::Stale(Stale { link_address: _ }) => None,
            DynamicNeighborState::Delay(Delay { link_address: _ }) => {
                Some(NudEvent::DelayFirstProbe)
            }
            DynamicNeighborState::Probe(Probe { link_address: _, transmit_counter: _ }) => {
                Some(NudEvent::RetransmitUnicastProbe)
            }
            DynamicNeighborState::Unreachable(Unreachable { link_address: _, mode }) => {
                // A timer should be scheduled iff a packet was recently sent to the neighbor
                // and we are retransmitting probes with exponential backoff.
                match mode {
                    UnreachableMode::WaitingForPacketSend => None,
                    UnreachableMode::Backoff { probes_sent: _, packet_sent: _ } => {
                        Some(NudEvent::RetransmitMulticastProbe)
                    }
                }
            }
        };
        assert_eq!(
            timers.cancel_neighbor(bindings_ctx, neighbor),
            expected_event,
            "neighbor {neighbor} ({self:?}) had unexpected timer installed"
        );
    }

    fn cancel_timer_and_complete_resolution<I, CC>(
        mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        neighbor: SpecifiedAddr<I::Addr>,
        link_address: UnicastAddr<D::Address>,
    ) where
        I: Ip,
        BC: NudBindingsContext<I, D, CC::DeviceId>,
        CC: NudSenderContext<I, D, BC>,
    {
        self.cancel_timer(bindings_ctx, timers, neighbor);

        match self {
            DynamicNeighborState::Incomplete(mut incomplete) => {
                incomplete.complete_resolution(core_ctx, bindings_ctx, link_address);
            }
            DynamicNeighborState::Reachable(_)
            | DynamicNeighborState::Stale(_)
            | DynamicNeighborState::Delay(_)
            | DynamicNeighborState::Probe(_)
            | DynamicNeighborState::Unreachable(_) => {}
        }
    }

    fn to_event_dynamic_state(&self) -> EventDynamicState<D::Address> {
        match self {
            Self::Incomplete(_) => EventDynamicState::Incomplete,
            Self::Reachable(Reachable { link_address, last_confirmed_at: _ }) => {
                EventDynamicState::Reachable(*link_address)
            }
            Self::Stale(Stale { link_address }) => EventDynamicState::Stale(*link_address),
            Self::Delay(Delay { link_address }) => EventDynamicState::Delay(*link_address),
            Self::Probe(Probe { link_address, transmit_counter: _ }) => {
                EventDynamicState::Probe(*link_address)
            }
            Self::Unreachable(Unreachable { link_address, mode: _ }) => {
                EventDynamicState::Unreachable(*link_address)
            }
        }
    }

    // Enters reachable state.
    fn enter_reachable<I, CC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        device_id: &CC::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        link_address: UnicastAddr<D::Address>,
    ) where
        I: Ip,
        BC: NudBindingsContext<I, D, CC::DeviceId>,
        CC: NudSenderContext<I, D, BC>,
    {
        // TODO(https://fxbug.dev/42075782): if the new state matches the current state,
        // update the link address as necessary, but do not cancel + reschedule timers.
        let now = bindings_ctx.now();
        match self {
            // If this neighbor entry is already in REACHABLE, rather than proactively
            // rescheduling the timer (which can be a relatively expensive operation
            // especially in the hot path), simply update `last_confirmed_at` so that when
            // the timer does eventually fire, we can reschedule it accordingly.
            DynamicNeighborState::Reachable(Reachable {
                link_address: current,
                last_confirmed_at,
            }) if *current == link_address => {
                *last_confirmed_at = now;
                return;
            }
            DynamicNeighborState::Incomplete(_)
            | DynamicNeighborState::Reachable(_)
            | DynamicNeighborState::Stale(_)
            | DynamicNeighborState::Delay(_)
            | DynamicNeighborState::Probe(_)
            | DynamicNeighborState::Unreachable(_) => {}
        }
        let previous = core::mem::replace(
            self,
            DynamicNeighborState::Reachable(Reachable { link_address, last_confirmed_at: now }),
        );
        let event_dynamic_state = self.to_event_dynamic_state();
        debug_assert_ne!(previous.to_event_dynamic_state(), event_dynamic_state);
        let event_state = EventState::Dynamic(event_dynamic_state);
        bindings_ctx.on_event(Event::changed(device_id, event_state, neighbor, bindings_ctx.now()));
        previous.cancel_timer_and_complete_resolution(
            core_ctx,
            bindings_ctx,
            timers,
            neighbor,
            link_address,
        );
        timers.schedule_neighbor(
            bindings_ctx,
            core_ctx.base_reachable_time(),
            neighbor,
            NudEvent::ReachableTime,
        );
    }

    // Enters the Stale state.
    //
    // # Panics
    //
    // Panics if `self` is already in Stale with a link address equal to
    // `link_address`, i.e. this function should only be called when state
    // actually changes.
    fn enter_stale<I, CC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        device_id: &CC::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        link_address: UnicastAddr<D::Address>,
        num_entries: usize,
        gc_state: &mut GarbageCollectionState<BC::Instant>,
    ) where
        I: Ip,
        BC: NudBindingsContext<I, D, CC::DeviceId>,
        CC: NudSenderContext<I, D, BC>,
    {
        // TODO(https://fxbug.dev/42075782): if the new state matches the current state,
        // update the link address as necessary, but do not cancel + reschedule timers.
        let previous =
            core::mem::replace(self, DynamicNeighborState::Stale(Stale { link_address }));
        let event_dynamic_state = self.to_event_dynamic_state();
        debug_assert_ne!(previous.to_event_dynamic_state(), event_dynamic_state);
        let event_state = EventState::Dynamic(event_dynamic_state);
        bindings_ctx.on_event(Event::changed(device_id, event_state, neighbor, bindings_ctx.now()));
        previous.cancel_timer_and_complete_resolution(
            core_ctx,
            bindings_ctx,
            timers,
            neighbor,
            link_address,
        );

        // This entry is deemed discardable now that it is not in active use; schedule
        // garbage collection for the neighbor table if we are currently over the
        // maximum amount of entries.
        timers.maybe_schedule_gc(bindings_ctx, num_entries, gc_state);

        // Stale entries don't do anything until an outgoing packet is queued for
        // transmission.
    }

    /// Resolve the cached link address for this neighbor entry, or return an
    /// observer for an unresolved neighbor, and advance the NUD state machine
    /// accordingly (as if a packet had been sent to the neighbor).
    ///
    /// Also returns whether a multicast neighbor probe should be sent as a result.
    fn resolve_link_addr<I, DeviceId, CC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        device_id: &DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
    ) -> (
        LinkResolutionResult<
            UnicastAddr<D::Address>,
            <<BC as LinkResolutionContext<D>>::Notifier as LinkResolutionNotifier<D>>::Observer,
        >,
        bool,
    )
    where
        I: Ip,
        DeviceId: StrongDeviceIdentifier,
        BC: NudBindingsContext<I, D, DeviceId>,
        CC: NudConfigContext<I>,
    {
        match self {
            DynamicNeighborState::Incomplete(Incomplete {
                notifiers,
                transmit_counter: _,
                pending_frames: _,
                _marker,
            }) => {
                let (notifier, observer) = BC::Notifier::new();
                notifiers.push(notifier);

                (LinkResolutionResult::Pending(observer), false)
            }
            DynamicNeighborState::Stale(entry) => {
                // Advance the state machine as if a packet had been sent to this neighbor.
                //
                // This is not required by the RFC, and it may result in neighbor probes going
                // out for this neighbor that would not have otherwise (the only other way a
                // STALE entry moves to DELAY is due to a packet being sent to it). However,
                // sending neighbor probes to confirm reachability is likely to be useful given
                // a client is attempting to resolve this neighbor. Additionally, this maintains
                // consistency with Netstack2's behavior.
                let delay @ Delay { link_address } =
                    entry.enter_delay(bindings_ctx, timers, neighbor);
                *self = DynamicNeighborState::Delay(delay);
                let event_state = EventState::Dynamic(self.to_event_dynamic_state());
                bindings_ctx.on_event(Event::changed(
                    device_id,
                    event_state,
                    neighbor,
                    bindings_ctx.now(),
                ));

                (LinkResolutionResult::Resolved(link_address), false)
            }
            DynamicNeighborState::Reachable(Reachable { link_address, last_confirmed_at: _ })
            | DynamicNeighborState::Delay(Delay { link_address })
            | DynamicNeighborState::Probe(Probe { link_address, transmit_counter: _ }) => {
                (LinkResolutionResult::Resolved(*link_address), false)
            }
            DynamicNeighborState::Unreachable(unreachable) => {
                let Unreachable { link_address, mode: _ } = unreachable;
                let link_address = *link_address;

                // Advance the state machine as if a packet had been sent to this neighbor.
                let do_multicast_solicit = unreachable.handle_packet_queued_to_send(
                    core_ctx,
                    bindings_ctx,
                    timers,
                    neighbor,
                );
                (LinkResolutionResult::Resolved(link_address), do_multicast_solicit)
            }
        }
    }

    /// Handle a packet being queued for transmission: either queue it as a
    /// pending packet for an unresolved neighbor, or send it to the cached link
    /// address, and advance the NUD state machine accordingly.
    ///
    /// Returns whether a multicast neighbor probe should be sent as a result.
    fn handle_packet_queued_to_send<I, CC, S>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        device_id: &CC::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        body: S,
        meta: BC::TxMetadata,
    ) -> Result<bool, SendFrameError<S>>
    where
        I: Ip,
        BC: NudBindingsContext<I, D, CC::DeviceId>,
        CC: NudSenderContext<I, D, BC>,
        S: NetworkSerializer,
        S::Buffer: BufferMut,
    {
        match self {
            DynamicNeighborState::Incomplete(incomplete) => {
                incomplete.queue_packet(body, meta).map(|()| false).map_err(|e| e.err_into())
            }
            // Send the IP packet while holding the NUD lock to prevent a potential
            // ordering violation.
            //
            // If we drop the NUD lock before sending out this packet, another thread
            // could take the NUD lock and send a packet *before* this packet is sent
            // out, resulting in out-of-order transmission to the device.
            DynamicNeighborState::Stale(entry) => {
                // Per [RFC 4861 section 7.3.3]:
                //
                //   The first time a node sends a packet to a neighbor whose entry is
                //   STALE, the sender changes the state to DELAY and sets a timer to
                //   expire in DELAY_FIRST_PROBE_TIME seconds.
                //
                // [RFC 4861 section 7.3.3]: https://tools.ietf.org/html/rfc4861#section-7.3.3
                let delay @ Delay { link_address } =
                    entry.enter_delay(bindings_ctx, timers, neighbor);
                *self = DynamicNeighborState::Delay(delay);
                let event_state = EventState::Dynamic(self.to_event_dynamic_state());
                bindings_ctx.on_event(Event::changed(
                    device_id,
                    event_state,
                    neighbor,
                    bindings_ctx.now(),
                ));

                core_ctx.send_ip_packet_to_neighbor_link_addr(
                    bindings_ctx,
                    link_address,
                    body,
                    meta,
                )?;

                Ok(false)
            }
            DynamicNeighborState::Reachable(Reachable { link_address, last_confirmed_at: _ })
            | DynamicNeighborState::Delay(Delay { link_address })
            | DynamicNeighborState::Probe(Probe { link_address, transmit_counter: _ }) => {
                core_ctx.send_ip_packet_to_neighbor_link_addr(
                    bindings_ctx,
                    *link_address,
                    body,
                    meta,
                )?;

                Ok(false)
            }
            DynamicNeighborState::Unreachable(unreachable) => {
                let Unreachable { link_address, mode: _ } = unreachable;
                core_ctx.send_ip_packet_to_neighbor_link_addr(
                    bindings_ctx,
                    *link_address,
                    body,
                    meta,
                )?;

                let do_multicast_solicit = unreachable.handle_packet_queued_to_send(
                    core_ctx,
                    bindings_ctx,
                    timers,
                    neighbor,
                );
                Ok(do_multicast_solicit)
            }
        }
    }

    fn handle_probe<I, CC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        device_id: &CC::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        incoming_link_address: UnicastAddr<D::Address>,
        num_entries: usize,
        gc_state: &mut GarbageCollectionState<BC::Instant>,
    ) where
        I: Ip,
        BC: NudBindingsContext<I, D, CC::DeviceId>,
        CC: NudSenderContext<I, D, BC>,
    {
        // Per [RFC 4861 section 7.2.3] ("Receipt of Neighbor Solicitations"):
        //
        //   If an entry already exists, and the cached link-layer address
        //   differs from the one in the received Source Link-Layer option, the
        //   cached address should be replaced by the received address, and the
        //   entry's reachability state MUST be set to STALE.
        //
        // [RFC 4861 section 7.2.3]: https://tools.ietf.org/html/rfc4861#section-7.2.3
        let transition_to_stale = match self {
            DynamicNeighborState::Incomplete(_) => true,
            DynamicNeighborState::Reachable(Reachable {
                link_address: current,
                last_confirmed_at: _,
            })
            | DynamicNeighborState::Stale(Stale { link_address: current })
            | DynamicNeighborState::Delay(Delay { link_address: current })
            | DynamicNeighborState::Probe(Probe { link_address: current, transmit_counter: _ })
            | DynamicNeighborState::Unreachable(Unreachable { link_address: current, mode: _ }) => {
                current != &incoming_link_address
            }
        };
        if transition_to_stale {
            self.enter_stale(
                core_ctx,
                bindings_ctx,
                timers,
                device_id,
                neighbor,
                incoming_link_address,
                num_entries,
                gc_state,
            );
        }
    }

    fn handle_confirmation<I, CC>(
        &mut self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        timers: &mut TimerHeap<I, BC>,
        device_id: &CC::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        incoming_link_address: Option<UnicastAddr<D::Address>>,
        flags: ConfirmationFlags,
        num_entries: usize,
        gc_state: &mut GarbageCollectionState<BC::Instant>,
    ) where
        I: Ip,
        BC: NudBindingsContext<I, D, CC::DeviceId>,
        CC: NudSenderContext<I, D, BC>,
    {
        let ConfirmationFlags { solicited_flag, override_flag } = flags;
        enum NewState<A> {
            Reachable { link_address: A },
            Stale { link_address: A },
        }

        let new_state = match self {
            DynamicNeighborState::Incomplete(Incomplete {
                transmit_counter: _,
                pending_frames: _,
                notifiers: _,
                _marker,
            }) => {
                // Per RFC 4861 section 7.2.5:
                //
                //   If the advertisement's Solicited flag is set, the state of the
                //   entry is set to REACHABLE; otherwise, it is set to STALE.
                //
                //   Note that the Override flag is ignored if the entry is in the
                //   INCOMPLETE state.
                //
                // Also note that if the target link-layer address was not specified in this
                // neighbor confirmation, we ignore the confirmation: there is nothing we can do
                // since we don't have a cached link-layer address.
                incoming_link_address.map(|link_address| {
                    if solicited_flag {
                        NewState::Reachable { link_address }
                    } else {
                        NewState::Stale { link_address }
                    }
                })
            }

            // Ignore duplicate neighbor confirmations that arrive less than
            // `override_lock_time()` from the previous one. This matches Linux
            // behavior. See https://fxbug.dev/490161899 .
            DynamicNeighborState::Reachable(Reachable {
                link_address: current,
                last_confirmed_at,
            }) if !core_ctx.override_lock_time().is_zero()
                && solicited_flag
                && bindings_ctx.now().saturating_duration_since(*last_confirmed_at)
                    < core_ctx.override_lock_time() =>
            {
                // If the link address matches the cached address (or is omitted, as
                // permitted in unicast NDP advertisements per RFC 4861 section 4.4),
                // this is a harmless duplicate confirmation and is logged at debug level.
                // If the link address differs, it may indicate a conflict or spoofing attempt,
                // so log at warn level.
                if incoming_link_address.as_ref().is_some_and(|addr| addr != current) {
                    warn!(
                        "Ignoring duplicate neighbor confirmation for {:?}. Current address {:?}. \
                        New address {:?}",
                        neighbor, current, incoming_link_address
                    );
                } else {
                    debug!(
                        "Ignoring duplicate neighbor confirmation for {:?}. Current address {:?}. \
                        New address {:?}",
                        neighbor, current, incoming_link_address
                    );
                }
                None
            }

            DynamicNeighborState::Reachable(Reachable {
                link_address: current,
                last_confirmed_at: _,
            })
            | DynamicNeighborState::Stale(Stale { link_address: current })
            | DynamicNeighborState::Delay(Delay { link_address: current })
            | DynamicNeighborState::Probe(Probe { link_address: current, transmit_counter: _ })
            | DynamicNeighborState::Unreachable(Unreachable { link_address: current, mode: _ }) => {
                // Per RFC 4861 section 4.4:
                //
                //    The link-layer address for the target, i.e., the
                //    sender of the advertisement ... MUST be
                //    included on link layers that have addresses when
                //    responding to multicast solicitations.  When
                //    responding to a unicast Neighbor Solicitation this
                //    option SHOULD be included.
                //
                //    ... When responding to unicast
                //    solicitations, the option can be omitted since the
                //    sender of the solicitation has the correct link-
                //    layer address; otherwise, it would not be able to
                //    send the unicast solicitation in the first place.
                //
                // Because neighbors may choose to omit the target link-layer address option
                // from neighbor confirmations, we must be tolerant of its absence. In the case
                // of absence, we use the cached link-layer address if one is available.
                let updated_link_address = incoming_link_address
                    .and_then(|link_address| (*current != link_address).then_some(link_address));

                match (solicited_flag, updated_link_address, override_flag) {
                    // Per RFC 4861 section 7.2.5:
                    //
                    //   If [either] the Override flag is set, or the supplied link-layer address is
                    //   the same as that in the cache, [and] ... the Solicited flag is set, the
                    //   entry MUST be set to REACHABLE.
                    (true, _, true) | (true, None, _) => Some(NewState::Reachable {
                        link_address: incoming_link_address.unwrap_or(*current),
                    }),
                    // Per RFC 4861 section 7.2.5:
                    //
                    //   If the Override flag is clear and the supplied link-layer address differs
                    //   from that in the cache, then one of two actions takes place:
                    //
                    //    a. If the state of the entry is REACHABLE, set it to STALE, but do not
                    //       update the entry in any other way.
                    //    b. Otherwise, the received advertisement should be ignored and MUST NOT
                    //       update the cache.
                    (_, Some(_), false) => match self {
                        // NB: do not update the link address.
                        DynamicNeighborState::Reachable(Reachable {
                            link_address: current,
                            last_confirmed_at: _,
                        }) => Some(NewState::Stale { link_address: *current }),
                        // Ignore the advertisement and do not update the cache.
                        DynamicNeighborState::Stale(_)
                        | DynamicNeighborState::Delay(_)
                        | DynamicNeighborState::Probe(_)
                        | DynamicNeighborState::Unreachable(_) => None,
                        // The INCOMPLETE state was already handled in the outer match.
                        DynamicNeighborState::Incomplete(_) => unreachable!(),
                    },
                    // Per RFC 4861 section 7.2.5:
                    //
                    //   If the Override flag is set [and] ... the Solicited flag is zero and the
                    //   link-layer address was updated with a different address, the state MUST be
                    //   set to STALE.
                    (false, Some(link_address), true) => Some(NewState::Stale { link_address }),
                    // Per RFC 4861 section 7.2.5:
                    //
                    //   There is no need to update the state for unsolicited advertisements that do
                    //   not change the contents of the cache.
                    (false, None, _) => None,
                }
            }
        };
        match new_state {
            Some(NewState::Reachable { link_address }) => self.enter_reachable(
                core_ctx,
                bindings_ctx,
                timers,
                device_id,
                neighbor,
                link_address,
            ),
            Some(NewState::Stale { link_address }) => self.enter_stale(
                core_ctx,
                bindings_ctx,
                timers,
                device_id,
                neighbor,
                link_address,
                num_entries,
                gc_state,
            ),
            None => {}
        }
    }
}

#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use super::*;

    use alloc::sync::Arc;

    use netstack3_base::sync::Mutex;
    use netstack3_base::testutil::{FakeBindingsCtx, FakeCoreCtx};

    /// Asserts that `device_id`'s `neighbor` resolved to `expected_link_addr`.
    pub fn assert_dynamic_neighbor_with_addr<
        I: Ip,
        D: LinkDevice,
        BC: NudBindingsContext<I, D, CC::DeviceId>,
        CC: NudContext<I, D, BC>,
    >(
        core_ctx: &mut CC,
        device_id: CC::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        expected_link_addr: UnicastAddr<D::Address>,
    ) {
        core_ctx.with_nud_state_mut(&device_id, |NudState { neighbors, .. }, _config| {
            assert_matches!(
                neighbors.get(&neighbor),
                Some(NeighborState::Dynamic(
                    DynamicNeighborState::Reachable(Reachable{ link_address, last_confirmed_at: _ })
                    | DynamicNeighborState::Stale(Stale{ link_address })
                )) => {
                    assert_eq!(link_address, &expected_link_addr)
                }
            )
        })
    }

    /// Asserts that the `device_id`'s `neighbor` is at `expected_state`.
    pub fn assert_dynamic_neighbor_state<I, D, BC, CC>(
        core_ctx: &mut CC,
        device_id: CC::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        expected_state: DynamicNeighborState<D, BC>,
    ) where
        I: Ip,
        D: LinkDevice + PartialEq,
        BC: NudBindingsContext<I, D, CC::DeviceId, TxMetadata: PartialEq>,
        CC: NudContext<I, D, BC>,
    {
        core_ctx.with_nud_state_mut(&device_id, |NudState { neighbors, .. }, _config| {
            assert_matches!(
                neighbors.get(&neighbor),
                Some(NeighborState::Dynamic(state)) => {
                    assert_eq!(state, &expected_state)
                }
            )
        })
    }

    /// Asserts that `device_id`'s `neighbor` doesn't exist.
    pub fn assert_neighbor_unknown<
        I: Ip,
        D: LinkDevice,
        BC: NudBindingsContext<I, D, CC::DeviceId>,
        CC: NudContext<I, D, BC>,
    >(
        core_ctx: &mut CC,
        device_id: CC::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
    ) {
        core_ctx.with_nud_state_mut(&device_id, |NudState { neighbors, .. }, _config| {
            assert_matches!(neighbors.get(&neighbor), None)
        })
    }

    impl<D: LinkDevice, Id, Event: Debug, State, FrameMeta> LinkResolutionContext<D>
        for FakeBindingsCtx<Id, Event, State, FrameMeta>
    {
        type Notifier = FakeLinkResolutionNotifier<D>;
    }

    /// A fake implementation of [`LinkResolutionNotifier`].
    #[derive(Debug)]
    pub struct FakeLinkResolutionNotifier<D: LinkDevice>(
        Arc<Mutex<Option<Result<UnicastAddr<D::Address>, AddressResolutionFailed>>>>,
    );

    impl<D: LinkDevice> LinkResolutionNotifier<D> for FakeLinkResolutionNotifier<D> {
        type Observer =
            Arc<Mutex<Option<Result<UnicastAddr<D::Address>, AddressResolutionFailed>>>>;

        fn new() -> (Self, Self::Observer) {
            let inner = Arc::new(Mutex::new(None));
            (Self(inner.clone()), inner)
        }

        fn notify(self, result: Result<UnicastAddr<D::Address>, AddressResolutionFailed>) {
            let Self(inner) = self;
            let mut inner = inner.lock();
            assert_eq!(*inner, None, "resolved link address was set more than once");
            *inner = Some(result);
        }
    }

    impl<S, Meta, DeviceId> UseDelegateNudContext for FakeCoreCtx<S, Meta, DeviceId> where
        S: UseDelegateNudContext
    {
    }
    impl<I: Ip, S, Meta, DeviceId> DelegateNudContext<I> for FakeCoreCtx<S, Meta, DeviceId>
    where
        S: DelegateNudContext<I>,
    {
        type Delegate<T> = S::Delegate<T>;
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
enum NudEvent {
    RetransmitMulticastProbe,
    ReachableTime,
    DelayFirstProbe,
    RetransmitUnicastProbe,
}

/// The timer ID for the NUD module.
#[derive(GenericOverIp, Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[generic_over_ip(I, Ip)]
pub struct NudTimerId<I: Ip, L: LinkDevice, D: WeakDeviceIdentifier> {
    device_id: D,
    timer_type: NudTimerType,
    _marker: PhantomData<(I, L)>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
enum NudTimerType {
    Neighbor,
    GarbageCollection,
}

/// A wrapper for [`LocalTimerHeap`] that we can attach NUD helpers to.
#[derive(Debug)]
pub(crate) struct TimerHeap<I: Ip, BT: TimerBindingsTypes + InstantBindingsTypes> {
    gc: BT::Timer,
    neighbor: LocalTimerHeap<SpecifiedAddr<I::Addr>, NudEvent, BT>,
}

impl<I: Ip, BC: TimerContext> TimerHeap<I, BC> {
    fn new<
        DeviceId: WeakDeviceIdentifier,
        L: LinkDevice,
        CC: CoreTimerContext<NudTimerId<I, L, DeviceId>, BC>,
    >(
        bindings_ctx: &mut BC,
        device_id: DeviceId,
    ) -> Self {
        Self {
            neighbor: LocalTimerHeap::new_with_context::<_, CC>(
                bindings_ctx,
                NudTimerId {
                    device_id: device_id.clone(),
                    timer_type: NudTimerType::Neighbor,
                    _marker: PhantomData,
                },
            ),
            gc: CC::new_timer(
                bindings_ctx,
                NudTimerId {
                    device_id,
                    timer_type: NudTimerType::GarbageCollection,
                    _marker: PhantomData,
                },
            ),
        }
    }

    fn schedule_neighbor(
        &mut self,
        bindings_ctx: &mut BC,
        after: NonZeroDuration,
        neighbor: SpecifiedAddr<I::Addr>,
        event: NudEvent,
    ) {
        let Self { neighbor: heap, gc: _ } = self;
        assert_eq!(heap.schedule_after(bindings_ctx, neighbor, event, after.get()), None);
    }

    fn schedule_neighbor_at(
        &mut self,
        bindings_ctx: &mut BC,
        at: BC::Instant,
        neighbor: SpecifiedAddr<I::Addr>,
        event: NudEvent,
    ) {
        let Self { neighbor: heap, gc: _ } = self;
        assert_eq!(heap.schedule_instant(bindings_ctx, neighbor, event, at), None);
    }

    /// Cancels a neighbor timer.
    fn cancel_neighbor(
        &mut self,
        bindings_ctx: &mut BC,
        neighbor: SpecifiedAddr<I::Addr>,
    ) -> Option<NudEvent> {
        let Self { neighbor: heap, gc: _ } = self;
        heap.cancel(bindings_ctx, &neighbor).map(|(_instant, v)| v)
    }

    fn pop_neighbor(
        &mut self,
        bindings_ctx: &mut BC,
    ) -> Option<(SpecifiedAddr<I::Addr>, NudEvent)> {
        let Self { neighbor: heap, gc: _ } = self;
        heap.pop(bindings_ctx)
    }

    /// Schedules a garbage collection IFF we hit the entries threshold and it's
    /// not already scheduled.
    fn maybe_schedule_gc(
        &mut self,
        bindings_ctx: &mut BC,
        num_entries: usize,
        gc_state: &mut GarbageCollectionState<BC::Instant>,
    ) {
        let GarbageCollectionState { is_dirty, last_gc } = gc_state;
        *is_dirty = true;
        let Self { gc, neighbor: _ } = self;
        if num_entries > GC_THRESHOLD && bindings_ctx.scheduled_instant(gc).is_none() {
            let instant = if let Some(last_gc) = last_gc {
                last_gc.panicking_add(MIN_GARBAGE_COLLECTION_INTERVAL.get())
            } else {
                bindings_ctx.now()
            };
            // Scheduling a timer requires a mutable borrow and we're
            // currently holding it exclusively. We just checked that the timer
            // is not scheduled, so this assertion always holds.
            assert_eq!(bindings_ctx.schedule_timer_instant(instant, gc), None);
        }
    }

    fn cancel_gc(&mut self, bindings_ctx: &mut BC) {
        let Self { gc, neighbor: _ } = self;
        let _: Option<BC::Instant> = bindings_ctx.cancel_timer(gc);
    }
}

/// State related to neighbor table garbage collection.
#[derive(Debug)]
pub struct GarbageCollectionState<Instant> {
    /// The last time garbage collection was run.
    last_gc: Option<Instant>,
    /// Whether the table contains potentially discardable entries (e.g. STALE
    /// or UNREACHABLE).
    is_dirty: bool,
}

/// NUD module per-device state.
#[derive(Debug)]
pub struct NudState<I: Ip, D: LinkDevice, BT: NudBindingsTypes<D>> {
    // TODO(https://fxbug.dev/42076887): Key neighbors by `UnicastAddr`.
    neighbors: HashMap<SpecifiedAddr<I::Addr>, NeighborState<D, BT>>,
    gc_state: GarbageCollectionState<BT::Instant>,
    timer_heap: TimerHeap<I, BT>,
}

impl<I: Ip, D: LinkDevice, BT: NudBindingsTypes<D>> NudState<I, D, BT> {
    /// Returns current neighbors.
    #[cfg(any(test, feature = "testutils"))]
    pub fn neighbors(&self) -> &HashMap<SpecifiedAddr<I::Addr>, NeighborState<D, BT>> {
        &self.neighbors
    }
}

impl<I: Ip, D: LinkDevice, BC: NudBindingsTypes<D> + TimerContext> NudState<I, D, BC> {
    /// Constructs a new `NudState` for `device_id`.
    pub fn new<
        DeviceId: WeakDeviceIdentifier,
        CC: CoreTimerContext<NudTimerId<I, D, DeviceId>, BC>,
    >(
        bindings_ctx: &mut BC,
        device_id: DeviceId,
    ) -> Self {
        Self {
            neighbors: Default::default(),
            gc_state: GarbageCollectionState { last_gc: None, is_dirty: false },
            timer_heap: TimerHeap::new::<_, _, CC>(bindings_ctx, device_id),
        }
    }
}

/// The bindings context for NUD.
pub trait NudBindingsContext<I: Ip, D: LinkDevice, DeviceId>:
    TimerContext
    + LinkResolutionContext<D>
    + EventContext<Event<D::Address, DeviceId, I, <Self as InstantBindingsTypes>::Instant>>
    + NudBindingsTypes<D>
{
}

impl<
    I: Ip,
    D: LinkDevice,
    DeviceId,
    BC: TimerContext
        + LinkResolutionContext<D>
        + EventContext<Event<D::Address, DeviceId, I, <Self as InstantBindingsTypes>::Instant>>
        + NudBindingsTypes<D>,
> NudBindingsContext<I, D, DeviceId> for BC
{
}

/// A marker trait for types provided by bindings to NUD.
pub trait NudBindingsTypes<D: LinkDevice>:
    LinkResolutionContext<D> + InstantBindingsTypes + TimerBindingsTypes + TxMetadataBindingsTypes
{
}

impl<BT, D> NudBindingsTypes<D> for BT
where
    D: LinkDevice,
    BT: LinkResolutionContext<D>
        + InstantBindingsTypes
        + TimerBindingsTypes
        + TxMetadataBindingsTypes,
{
}

/// An execution context that allows creating link resolution notifiers.
pub trait LinkResolutionContext<D: LinkDevice> {
    /// A notifier held by core that can be used to inform interested parties of
    /// the result of link address resolution.
    type Notifier: LinkResolutionNotifier<D>;
}

/// A notifier held by core that can be used to inform interested parties of the
/// result of link address resolution.
pub trait LinkResolutionNotifier<D: LinkDevice>: Debug + Sized + Send {
    /// The corresponding observer that can be used to observe the result of
    /// link address resolution.
    type Observer;

    /// Create a connected (notifier, observer) pair.
    fn new() -> (Self, Self::Observer);

    /// Signal to Bindings that link address resolution has completed for a
    /// neighbor.
    fn notify(self, result: Result<UnicastAddr<D::Address>, AddressResolutionFailed>);
}

/// The execution context for NUD for a link device.
pub trait NudContext<I: Ip, D: LinkDevice, BC: NudBindingsTypes<D>>: DeviceIdContext<D> {
    /// The inner configuration context.
    type ConfigCtx<'a>: NudConfigContext<I>;
    /// The inner send context.
    type SenderCtx<'a>: NudSenderContext<I, D, BC, DeviceId = Self::DeviceId>;

    /// Calls the function with a mutable reference to the NUD state and the
    /// core sender context.
    fn with_nud_state_mut_and_sender_ctx<
        O,
        F: FnOnce(&mut NudState<I, D, BC>, &mut Self::SenderCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with a mutable reference to the NUD state and NUD
    /// configuration for the device.
    fn with_nud_state_mut<O, F: FnOnce(&mut NudState<I, D, BC>, &mut Self::ConfigCtx<'_>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with an immutable reference to the NUD state.
    fn with_nud_state<O, F: FnOnce(&NudState<I, D, BC>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Sends a neighbor probe/solicitation message.
    ///
    /// If `remote_link_addr` is provided, the message will be unicasted to that
    /// address; if it is `None`, the message will be multicast.
    fn send_neighbor_solicitation(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        lookup_addr: SpecifiedAddr<I::Addr>,
        remote_link_addr: Option<UnicastAddr<D::Address>>,
    );
}

/// A marker trait to enable the blanket impl of [`NudContext`] for types
/// implementing [`DelegateNudContext`].
pub trait UseDelegateNudContext {}

/// Enables a blanket implementation of [`NudContext`] via delegate that can
/// wrap a mutable reference of `Self`.
///
/// The `UseDelegateNudContext` requirement here is steering users to to the
/// right thing to enable the blanket implementation.
pub trait DelegateNudContext<I: Ip>: UseDelegateNudContext + Sized {
    /// The delegate that implements [`NudContext`].
    type Delegate<T>: ref_cast::RefCast<From = T>;
    /// Wraps self into a mutable delegate reference.
    fn wrap(&mut self) -> &mut Self::Delegate<Self> {
        <Self::Delegate<Self> as ref_cast::RefCast>::ref_cast_mut(self)
    }
}

impl<I, D, BC, CC> NudContext<I, D, BC> for CC
where
    I: Ip,
    D: LinkDevice,
    BC: NudBindingsTypes<D>,
    CC: DelegateNudContext<I, Delegate<CC>: NudContext<I, D, BC, DeviceId = CC::DeviceId>>
        // This seems redundant with `DelegateNudContext` but it is required to
        // get the compiler happy.
        + UseDelegateNudContext
        + DeviceIdContext<D>,
{
    type ConfigCtx<'a> = <CC::Delegate<CC> as NudContext<I, D, BC>>::ConfigCtx<'a>;
    type SenderCtx<'a> = <CC::Delegate<CC> as NudContext<I, D, BC>>::SenderCtx<'a>;
    fn with_nud_state_mut_and_sender_ctx<
        O,
        F: FnOnce(&mut NudState<I, D, BC>, &mut Self::SenderCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.wrap().with_nud_state_mut_and_sender_ctx(device_id, cb)
    }

    fn with_nud_state_mut<O, F: FnOnce(&mut NudState<I, D, BC>, &mut Self::ConfigCtx<'_>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.wrap().with_nud_state_mut(device_id, cb)
    }
    fn with_nud_state<O, F: FnOnce(&NudState<I, D, BC>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.wrap().with_nud_state(device_id, cb)
    }
    fn send_neighbor_solicitation(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        lookup_addr: SpecifiedAddr<I::Addr>,
        remote_link_addr: Option<UnicastAddr<D::Address>>,
    ) {
        self.wrap().send_neighbor_solicitation(
            bindings_ctx,
            device_id,
            lookup_addr,
            remote_link_addr,
        )
    }
}

/// IP extension trait to support [`NudIcmpContext`].
pub trait NudIcmpIpExt: packet_formats::ip::IpExt {
    /// IP packet metadata needed when sending ICMP destination unreachable
    /// errors as a result of link-layer address resolution failure.
    type Metadata;

    /// Extracts IP-version specific metadata from `packet`.
    fn extract_metadata<B: SplitByteSlice>(packet: &Self::Packet<B>) -> Self::Metadata;
}

impl NudIcmpIpExt for Ipv4 {
    type Metadata = Ipv4FragmentType;

    fn extract_metadata<B: SplitByteSlice>(packet: &Ipv4Packet<B>) -> Self::Metadata {
        packet.fragment_type()
    }
}

impl NudIcmpIpExt for Ipv6 {
    type Metadata = ();

    fn extract_metadata<B: SplitByteSlice>(_packet: &Ipv6Packet<B>) -> Self::Metadata {
        ()
    }
}

/// The execution context which allows sending ICMP destination unreachable
/// errors, which needs to happen when address resolution fails.
pub trait NudIcmpContext<I: NudIcmpIpExt, D: LinkDevice, BC>: DeviceIdContext<D> {
    /// Send an ICMP destination unreachable error to `original_src_ip` as
    /// a result of `frame` being unable to be sent/forwarded due to link
    /// layer address resolution failure.
    ///
    /// `original_src_ip`, `original_dst_ip`, and `header_len` are all IP
    /// header fields from `frame`.
    fn send_icmp_dest_unreachable(
        &mut self,
        bindings_ctx: &mut BC,
        frame: Buf<Vec<u8>>,
        device_id: Option<&Self::DeviceId>,
        original_src_ip: SocketIpAddr<I::Addr>,
        original_dst_ip: SocketIpAddr<I::Addr>,
        header_len: usize,
        proto: I::Proto,
        metadata: I::Metadata,
    );
}

/// NUD configurations.
#[derive(Clone, Debug)]
pub struct NudUserConfig {
    /// The maximum number of unicast solicitations as defined in [RFC 4861
    /// section 10].
    ///
    /// [RFC 4861 section 10]: https://tools.ietf.org/html/rfc4861#section-10
    pub max_unicast_solicitations: NonZeroU16,
    /// The maximum number of multicast solicitations as defined in [RFC 4861
    /// section 10].
    ///
    /// [RFC 4861 section 10]: https://tools.ietf.org/html/rfc4861#section-10
    pub max_multicast_solicitations: NonZeroU16,
    /// The base value used for computing the duration a neighbor is considered
    /// reachable after receiving a reachability confirmation as defined in
    /// [RFC 4861 section 6.3.2].
    ///
    /// [RFC 4861 section 6.3.2]: https://tools.ietf.org/html/rfc4861#section-6.3.2
    pub base_reachable_time: NonZeroDuration,
    /// The time between retransmissions of neighbor probe messages to a neighbor
    /// when resolving the address or when probing the reachability of a neighbor
    /// as defined in [RFC 4861 section 6.3.2].
    ///
    /// [RFC 4861 section 6.3.2]: https://tools.ietf.org/html/rfc4861#section-6.3.2
    pub retrans_timer: NonZeroDuration,
}

impl Default for NudUserConfig {
    fn default() -> Self {
        NudUserConfig {
            max_unicast_solicitations: DEFAULT_MAX_UNICAST_SOLICIT,
            max_multicast_solicitations: DEFAULT_MAX_MULTICAST_SOLICIT,
            base_reachable_time: DEFAULT_BASE_REACHABLE_TIME,
            retrans_timer: DEFAULT_RETRANS_TIMER,
        }
    }
}

/// An update structure for [`NudUserConfig`].
///
/// Only fields with variant `Some` are updated.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct NudUserConfigUpdate {
    pub max_unicast_solicitations: Option<NonZeroU16>,
    pub max_multicast_solicitations: Option<NonZeroU16>,
    pub base_reachable_time: Option<NonZeroDuration>,
    pub retrans_timer: Option<NonZeroDuration>,
}

impl NudUserConfigUpdate {
    /// Applies the configuration returning a [`NudUserConfigUpdate`] with the
    /// changed fields populated.
    pub fn apply_and_take_previous(mut self, config: &mut NudUserConfig) -> Self {
        fn swap_if_set<T>(opt: &mut Option<T>, target: &mut T) {
            if let Some(opt) = opt.as_mut() {
                core::mem::swap(opt, target)
            }
        }
        let Self {
            max_unicast_solicitations,
            max_multicast_solicitations,
            base_reachable_time,
            retrans_timer,
        } = &mut self;
        swap_if_set(max_unicast_solicitations, &mut config.max_unicast_solicitations);
        swap_if_set(max_multicast_solicitations, &mut config.max_multicast_solicitations);
        swap_if_set(base_reachable_time, &mut config.base_reachable_time);
        swap_if_set(retrans_timer, &mut config.retrans_timer);

        self
    }
}

/// The execution context for NUD that allows accessing NUD configuration (such
/// as timer durations) for a particular device.
pub trait NudConfigContext<I: Ip> {
    /// The amount of time between retransmissions of neighbor probe messages.
    ///
    /// This corresponds to the configurable per-interface `RetransTimer` value
    /// used in NUD as defined in [RFC 4861 section 6.3.2].
    ///
    /// [RFC 4861 section 6.3.2]: https://datatracker.ietf.org/doc/html/rfc4861#section-6.3.2
    fn retransmit_timeout(&mut self) -> NonZeroDuration;

    /// Calls the callback with an immutable reference to NUD configurations.
    fn with_nud_user_config<O, F: FnOnce(&NudUserConfig) -> O>(&mut self, cb: F) -> O;

    /// Returns the maximum number of unicast solicitations.
    fn max_unicast_solicit(&mut self) -> NonZeroU16 {
        self.with_nud_user_config(|NudUserConfig { max_unicast_solicitations, .. }| {
            *max_unicast_solicitations
        })
    }

    /// Returns the maximum number of multicast solicitations.
    fn max_multicast_solicit(&mut self) -> NonZeroU16 {
        self.with_nud_user_config(|NudUserConfig { max_multicast_solicitations, .. }| {
            *max_multicast_solicitations
        })
    }

    /// Returns the base reachable time, the duration a neighbor is considered
    /// reachable after receiving a reachability confirmation.
    fn base_reachable_time(&mut self) -> NonZeroDuration {
        self.with_nud_user_config(|NudUserConfig { base_reachable_time, .. }| *base_reachable_time)
    }

    /// Amount of time from the moment a host becomes reachable before the
    /// entry can overridden.
    fn override_lock_time(&mut self) -> Duration;
}

/// The execution context for NUD for a link device that allows sending IP
/// packets to specific neighbors.
pub trait NudSenderContext<I: Ip, D: LinkDevice, BC: NudBindingsTypes<D>>:
    NudConfigContext<I> + DeviceIdContext<D>
{
    /// Send an IP frame to the neighbor with the specified link address.
    fn send_ip_packet_to_neighbor_link_addr<S>(
        &mut self,
        bindings_ctx: &mut BC,
        neighbor_link_addr: UnicastAddr<D::Address>,
        body: S,
        meta: BC::TxMetadata,
    ) -> Result<(), SendFrameError<S>>
    where
        S: NetworkSerializer,
        S::Buffer: BufferMut;
}

/// An implementation of NUD for the IP layer.
pub trait NudIpHandler<I: Ip, BC>: DeviceIdContext<AnyDevice> {
    /// Handles an incoming neighbor probe message.
    ///
    /// For IPv6, this can be an NDP Neighbor Solicitation or an NDP Router
    /// Advertisement message.
    fn handle_neighbor_probe(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        link_addr: &[u8],
    );

    /// Handles an incoming neighbor confirmation message.
    ///
    /// For IPv6, this can be an NDP Neighbor Advertisement.
    fn handle_neighbor_confirmation(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        link_addr: Option<&[u8]>,
        flags: ConfirmationFlags,
    );

    /// Clears the neighbor table.
    fn flush_neighbor_table(&mut self, bindings_ctx: &mut BC, device_id: &Self::DeviceId);
}

/// Specifies the link-layer address of a neighbor.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LinkResolutionResult<A, Observer> {
    /// The destination is a known neighbor with the given link-layer address.
    Resolved(A),
    /// The destination is pending neighbor resolution.
    Pending(Observer),
}

/// An implementation of NUD for a link device.
pub trait NudHandler<I: Ip, D: LinkDevice, BC: NudBindingsTypes<D>>: DeviceIdContext<D> {
    /// Sets a dynamic neighbor's entry state to the specified values in
    /// response to the source packet.
    fn handle_neighbor_update(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        // TODO(https://fxbug.dev/42076887): Use IPv4 subnet information to
        // disallow the address with all host bits equal to 0, and the
        // subnet broadcast addresses with all host bits equal to 1.
        // TODO(https://fxbug.dev/42083952): Use NeighborAddr when available.
        neighbor: SpecifiedAddr<I::Addr>,
        source: DynamicNeighborUpdateSource<D::Address>,
    );

    /// Clears the neighbor table.
    fn flush(&mut self, bindings_ctx: &mut BC, device_id: &Self::DeviceId);

    /// Send an IP packet to the neighbor.
    ///
    /// If the neighbor's link address is not known, link address resolution
    /// is performed.
    fn send_ip_packet_to_neighbor<S>(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        body: S,
        meta: BC::TxMetadata,
    ) -> Result<(), SendFrameError<S>>
    where
        S: NetworkSerializer,
        S::Buffer: BufferMut;
}

enum TransmitProbe<A> {
    Multicast,
    Unicast(A),
}

impl<
    I: NudIcmpIpExt,
    D: LinkDevice,
    BC: NudBindingsContext<I, D, CC::DeviceId>,
    CC: NudContext<I, D, BC> + NudIcmpContext<I, D, BC> + CounterContext<NudCounters<I>>,
> HandleableTimer<CC, BC> for NudTimerId<I, D, CC::WeakDeviceId>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, _: BC::UniqueTimerId) {
        let Self { device_id, timer_type, _marker: PhantomData } = self;
        let Some(device_id) = device_id.upgrade() else {
            return;
        };
        match timer_type {
            NudTimerType::Neighbor => handle_neighbor_timer(core_ctx, bindings_ctx, device_id),
            NudTimerType::GarbageCollection => collect_garbage(core_ctx, bindings_ctx, device_id),
        }
    }
}

fn handle_neighbor_timer<I, D, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: CC::DeviceId,
) where
    I: NudIcmpIpExt,
    D: LinkDevice,
    BC: NudBindingsContext<I, D, CC::DeviceId>,
    CC: NudContext<I, D, BC> + NudIcmpContext<I, D, BC> + CounterContext<NudCounters<I>>,
{
    enum Action<L, A, M> {
        TransmitProbe { probe: TransmitProbe<L>, to: A },
        SendIcmpDestUnreachable(VecDeque<(Buf<Vec<u8>>, M)>),
    }
    let action = core_ctx.with_nud_state_mut(
        &device_id,
        |NudState { neighbors, gc_state, timer_heap }, core_ctx| {
            let (lookup_addr, event) = timer_heap.pop_neighbor(bindings_ctx)?;
            let num_entries = neighbors.len();
            let mut entry = match neighbors.entry(lookup_addr) {
                Entry::Occupied(entry) => entry,
                Entry::Vacant(_) => panic!("timer fired for invalid entry"),
            };

            match entry.get_mut() {
                NeighborState::Dynamic(DynamicNeighborState::Incomplete(incomplete)) => {
                    assert_eq!(event, NudEvent::RetransmitMulticastProbe);

                    if incomplete.schedule_timer_if_should_retransmit(
                        core_ctx,
                        bindings_ctx,
                        timer_heap,
                        lookup_addr,
                    ) {
                        Some(Action::TransmitProbe {
                            probe: TransmitProbe::Multicast,
                            to: lookup_addr,
                        })
                    } else {
                        // Failed to complete neighbor resolution and no more probes to send.
                        // Subsequent traffic to this neighbor will recreate the entry and restart
                        // address resolution.
                        //
                        // TODO(https://fxbug.dev/42082448): consider retaining this neighbor entry in
                        // a sentinel `Failed` state, equivalent to its having been discarded except
                        // for debugging/observability purposes.
                        debug!("neighbor resolution failed for {lookup_addr}; removing entry");
                        let Incomplete {
                            transmit_counter: _,
                            ref mut pending_frames,
                            notifiers: _,
                            _marker,
                        } = assert_matches!(
                            entry.remove(),
                            NeighborState::Dynamic(DynamicNeighborState::Incomplete(incomplete))
                                => incomplete
                        );
                        let pending_frames = core::mem::take(pending_frames);
                        bindings_ctx.on_event(Event::removed(
                            &device_id,
                            lookup_addr,
                            bindings_ctx.now(),
                        ));
                        Some(Action::SendIcmpDestUnreachable(pending_frames))
                    }
                }
                NeighborState::Dynamic(DynamicNeighborState::Probe(probe)) => {
                    assert_eq!(event, NudEvent::RetransmitUnicastProbe);

                    let Probe { link_address, transmit_counter: _ } = probe;
                    let link_address = *link_address;
                    if probe.schedule_timer_if_should_retransmit(
                        core_ctx,
                        bindings_ctx,
                        timer_heap,
                        lookup_addr,
                    ) {
                        Some(Action::TransmitProbe {
                            probe: TransmitProbe::Unicast(link_address),
                            to: lookup_addr,
                        })
                    } else {
                        let unreachable = probe.enter_unreachable(
                            bindings_ctx,
                            timer_heap,
                            num_entries,
                            gc_state,
                        );
                        *entry.get_mut() =
                            NeighborState::Dynamic(DynamicNeighborState::Unreachable(unreachable));
                        let event_state = entry.get_mut().to_event_state();
                        let event = Event::changed(
                            &device_id,
                            event_state,
                            lookup_addr,
                            bindings_ctx.now(),
                        );
                        bindings_ctx.on_event(event);
                        None
                    }
                }
                NeighborState::Dynamic(DynamicNeighborState::Unreachable(unreachable)) => {
                    assert_eq!(event, NudEvent::RetransmitMulticastProbe);
                    unreachable
                        .handle_timer(core_ctx, bindings_ctx, timer_heap, &device_id, lookup_addr)
                        .map(|probe| Action::TransmitProbe { probe, to: lookup_addr })
                }
                NeighborState::Dynamic(DynamicNeighborState::Reachable(Reachable {
                    link_address,
                    last_confirmed_at,
                })) => {
                    assert_eq!(event, NudEvent::ReachableTime);
                    let link_address = *link_address;

                    let expiration =
                        last_confirmed_at.saturating_add(core_ctx.base_reachable_time().get());
                    if expiration > bindings_ctx.now() {
                        timer_heap.schedule_neighbor_at(
                            bindings_ctx,
                            expiration,
                            lookup_addr,
                            NudEvent::ReachableTime,
                        );
                    } else {
                        // Per [RFC 4861 section 7.3.3]:
                        //
                        //   When ReachableTime milliseconds have passed since receipt of the last
                        //   reachability confirmation for a neighbor, the Neighbor Cache entry's
                        //   state changes from REACHABLE to STALE.
                        //
                        // [RFC 4861 section 7.3.3]: https://tools.ietf.org/html/rfc4861#section-7.3.3
                        *entry.get_mut() =
                            NeighborState::Dynamic(DynamicNeighborState::Stale(Stale {
                                link_address,
                            }));
                        let event_state = entry.get_mut().to_event_state();
                        let event = Event::changed(
                            &device_id,
                            event_state,
                            lookup_addr,
                            bindings_ctx.now(),
                        );
                        bindings_ctx.on_event(event);

                        // This entry is deemed discardable now that it is not in active use;
                        // schedule garbage collection for the neighbor table if we are currently
                        // over the maximum amount of entries.
                        timer_heap.maybe_schedule_gc(bindings_ctx, num_entries, gc_state);
                    }

                    None
                }
                NeighborState::Dynamic(DynamicNeighborState::Delay(delay)) => {
                    assert_eq!(event, NudEvent::DelayFirstProbe);

                    // Per [RFC 4861 section 7.3.3]:
                    //
                    //   If the entry is still in the DELAY state when the timer expires, the
                    //   entry's state changes to PROBE.
                    //
                    // [RFC 4861 section 7.3.3]: https://tools.ietf.org/html/rfc4861#section-7.3.3
                    let probe @ Probe { link_address, transmit_counter: _ } =
                        delay.enter_probe(core_ctx, bindings_ctx, timer_heap, lookup_addr);
                    *entry.get_mut() = NeighborState::Dynamic(DynamicNeighborState::Probe(probe));
                    let event_state = entry.get_mut().to_event_state();
                    bindings_ctx.on_event(Event::changed(
                        &device_id,
                        event_state,
                        lookup_addr,
                        bindings_ctx.now(),
                    ));

                    Some(Action::TransmitProbe {
                        probe: TransmitProbe::Unicast(link_address),
                        to: lookup_addr,
                    })
                }
                state @ (NeighborState::Static(_)
                | NeighborState::Dynamic(DynamicNeighborState::Stale(_))) => {
                    panic!("timer unexpectedly fired in state {state:?}")
                }
            }
        },
    );

    match action {
        Some(Action::SendIcmpDestUnreachable(mut pending_frames)) => {
            for (mut frame, meta) in pending_frames.drain(..) {
                // This frame is being dropped from the pending NUD queue, we
                // can release its tx metadata.
                core::mem::drop(meta);

                // TODO(https://fxbug.dev/323585811): Avoid needing to parse the packet to get
                // IP header fields by extracting them from the serializer passed into the NUD
                // layer and storing them alongside the pending frames instead.
                let Some((packet, original_src_ip, original_dst_ip)) = frame
                    .parse_mut::<I::Packet<_>>()
                    .map_err(|e| {
                        warn!("not sending ICMP dest unreachable due to parsing error: {:?}", e);
                    })
                    .ok()
                    .and_then(|packet| {
                        let original_src_ip = SocketIpAddr::new(packet.src_ip())?;
                        let original_dst_ip = SocketIpAddr::new(packet.dst_ip())?;
                        Some((packet, original_src_ip, original_dst_ip))
                    })
                    .or_else(|| {
                        core_ctx.counters().icmp_dest_unreachable_dropped.increment();
                        None
                    })
                else {
                    continue;
                };
                let header_metadata = I::extract_metadata(&packet);
                let header_len = packet.parse_metadata().header_len();
                let proto = packet.proto();
                let metadata = packet.parse_metadata();
                core::mem::drop(packet);
                frame.undo_parse(metadata);
                core_ctx.send_icmp_dest_unreachable(
                    bindings_ctx,
                    frame,
                    // Provide the device ID if `original_src_ip`, the address the ICMP error
                    // is destined for, is link-local. Note that if this address is link-local,
                    // it should be an address assigned to one of our own interfaces, because the
                    // link-local subnet should always be on-link according to RFC 5942 Section 3:
                    //
                    //   The link-local prefix is effectively considered a permanent entry on the
                    //   Prefix List.
                    //
                    // Even if the link-local subnet is off-link, passing the device ID is never
                    // incorrect because link-local traffic will never be forwarded, and
                    // there is only ever one link and thus interface involved.
                    original_src_ip.as_ref().must_have_zone().then_some(&device_id),
                    original_src_ip,
                    original_dst_ip,
                    header_len,
                    proto,
                    header_metadata,
                );
            }
        }
        Some(Action::TransmitProbe { probe, to }) => {
            let remote_link_addr = match probe {
                TransmitProbe::Multicast => None,
                TransmitProbe::Unicast(link_addr) => Some(link_addr),
            };
            core_ctx.send_neighbor_solicitation(bindings_ctx, &device_id, to, remote_link_addr);
        }
        None => {}
    }
}

impl<I: Ip, D: LinkDevice, BC: NudBindingsContext<I, D, CC::DeviceId>, CC: NudContext<I, D, BC>>
    NudHandler<I, D, BC> for CC
{
    fn handle_neighbor_update(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &CC::DeviceId,
        neighbor: SpecifiedAddr<I::Addr>,
        source: DynamicNeighborUpdateSource<D::Address>,
    ) {
        debug!("received neighbor {:?} from {}", source, neighbor);
        self.with_nud_state_mut_and_sender_ctx(
            device_id,
            |NudState { neighbors, gc_state, timer_heap }, core_ctx| {
                let num_entries = neighbors.len();
                match neighbors.get_mut(&neighbor) {
                    None => match source {
                        DynamicNeighborUpdateSource::Probe { link_address } => {
                            // Per [RFC 4861 section 7.2.3] ("Receipt of Neighbor Solicitations"):
                            //
                            //   If an entry does not already exist, the node SHOULD create a new
                            //   one and set its reachability state to STALE as specified in Section
                            //   7.3.3.
                            //
                            // [RFC 4861 section 7.2.3]: https://tools.ietf.org/html/rfc4861#section-7.2.3
                            let result = insert_new_entry(
                                neighbors,
                                gc_state,
                                timer_heap,
                                bindings_ctx,
                                device_id,
                                neighbor,
                                NeighborState::Dynamic(DynamicNeighborState::Stale(Stale {
                                    link_address,
                                })),
                            );
                            match result {
                                Ok(_entry) => {}
                                Err(TableFullError { entry }) => {
                                    debug!("Neighbor table full; failed to insert {entry:?}");
                                    return;
                                }
                            }

                            // This entry is not currently in active use; if we are currently over
                            // the maximum amount of entries, schedule garbage collection.
                            timer_heap.maybe_schedule_gc(bindings_ctx, neighbors.len(), gc_state);
                        }
                        // Per [RFC 4861 section 7.2.5] ("Receipt of Neighbor Advertisements"):
                        //
                        //   If no entry exists, the advertisement SHOULD be silently discarded.
                        //   There is no need to create an entry if none exists, since the
                        //   recipient has apparently not initiated any communication with the
                        //   target.
                        //
                        // [RFC 4861 section 7.2.5]: https://tools.ietf.org/html/rfc4861#section-7.2.5
                        DynamicNeighborUpdateSource::Confirmation { .. } => {}
                    },
                    Some(entry) => match entry {
                        NeighborState::Dynamic(e) => match source {
                            DynamicNeighborUpdateSource::Probe { link_address } => e.handle_probe(
                                core_ctx,
                                bindings_ctx,
                                timer_heap,
                                device_id,
                                neighbor,
                                link_address,
                                num_entries,
                                gc_state,
                            ),
                            DynamicNeighborUpdateSource::Confirmation { link_address, flags } => e
                                .handle_confirmation(
                                    core_ctx,
                                    bindings_ctx,
                                    timer_heap,
                                    device_id,
                                    neighbor,
                                    link_address,
                                    flags,
                                    num_entries,
                                    gc_state,
                                ),
                        },
                        NeighborState::Static(_) => {}
                    },
                }
            },
        );
    }

    fn flush(&mut self, bindings_ctx: &mut BC, device_id: &Self::DeviceId) {
        self.with_nud_state_mut(
            device_id,
            |NudState { neighbors, gc_state: _, timer_heap }, _config| {
                neighbors.drain().for_each(|(neighbor, state)| {
                    match state {
                        NeighborState::Dynamic(mut entry) => {
                            entry.cancel_timer(bindings_ctx, timer_heap, neighbor);
                        }
                        NeighborState::Static(_) => {}
                    }
                    bindings_ctx.on_event(Event::removed(device_id, neighbor, bindings_ctx.now()));
                });
            },
        );
    }

    fn send_ip_packet_to_neighbor<S>(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        lookup_addr: SpecifiedAddr<I::Addr>,
        body: S,
        meta: BC::TxMetadata,
    ) -> Result<(), SendFrameError<S>>
    where
        S: NetworkSerializer,
        S::Buffer: BufferMut,
    {
        let do_multicast_solicit = self.with_nud_state_mut_and_sender_ctx(
            device_id,
            |NudState { neighbors, gc_state, timer_heap },
             core_ctx|
             -> Result<_, SendFrameError<S>> {
                match neighbors.get_mut(&lookup_addr) {
                    None => {
                        let incomplete =
                            Incomplete::new(core_ctx, bindings_ctx, timer_heap, lookup_addr);
                        let result = insert_new_entry(
                            neighbors,
                            gc_state,
                            timer_heap,
                            bindings_ctx,
                            device_id,
                            lookup_addr,
                            NeighborState::Dynamic(DynamicNeighborState::Incomplete(incomplete)),
                        );
                        match result {
                            Err(TableFullError { entry }) => {
                                debug!("Neighbor table full; failed to insert {entry:?}");
                                return Err(ErrorAndSerializer {
                                    serializer: body,
                                    error: SendFrameErrorReason::AddressResolutionFailed,
                                });
                            }
                            Ok(mut entry) => {
                                let dynamic = assert_matches!(
                                    entry.get_mut(),
                                    NeighborState::Dynamic(d) => d,
                                    "newly inserted entry must still be dynamic"
                                );
                                let incomplete = assert_matches!(
                                    dynamic,
                                    DynamicNeighborState::Incomplete(i) => i,
                                    "newly inserted entry must still be incomplete"
                                );
                                // Queue the packet and unwind on failure.
                                match incomplete.queue_packet(body, meta) {
                                    Ok(()) => Ok(true),
                                    Err(e) => {
                                        dynamic.cancel_timer(bindings_ctx, timer_heap, lookup_addr);
                                        let _entry = entry.remove();
                                        Err(e.err_into())
                                    }
                                }
                            }
                        }
                    }
                    Some(entry) => {
                        match entry {
                            NeighborState::Static(link_address) => {
                                // Send the IP packet while holding the NUD lock to prevent a
                                // potential ordering violation.
                                //
                                // If we drop the NUD lock before sending out this packet, another
                                // thread could take the NUD lock and send a packet *before* this
                                // packet is sent out, resulting in out-of-order transmission to the
                                // device.
                                core_ctx.send_ip_packet_to_neighbor_link_addr(
                                    bindings_ctx,
                                    *link_address,
                                    body,
                                    meta,
                                )?;

                                Ok(false)
                            }
                            NeighborState::Dynamic(e) => {
                                let do_multicast_solicit = e.handle_packet_queued_to_send(
                                    core_ctx,
                                    bindings_ctx,
                                    timer_heap,
                                    device_id,
                                    lookup_addr,
                                    body,
                                    meta,
                                )?;

                                Ok(do_multicast_solicit)
                            }
                        }
                    }
                }
            },
        )?;

        if do_multicast_solicit {
            self.send_neighbor_solicitation(
                bindings_ctx,
                &device_id,
                lookup_addr,
                /* multicast */ None,
            );
        }

        Ok(())
    }
}

pub(crate) struct TableFullError<E> {
    entry: E,
}

/// Attempts to insert a new entry into the neighbor table.
///
/// If the table is full, the garbage collector will be run synchronously in
/// an attempt to free up space. If space becomes available, the entry will be
/// inserted, otherwise a `TableFullError` is returned.
///
/// Upon successful insertion, the `Added` event is emitted to bindings and a
/// the newly inserted entry is returned.
///
/// # Panics
///
/// May panic if the entry already exists (depending on whether the garbage
/// collector needs to run, and whether the existing entry can be discarded).
pub(crate) fn insert_new_entry<
    'a,
    I: Ip,
    D: LinkDevice,
    DeviceId: StrongDeviceIdentifier,
    BC: NudBindingsContext<I, D, DeviceId>,
>(
    neighbors: &'a mut HashMap<SpecifiedAddr<I::Addr>, NeighborState<D, BC>>,
    gc_state: &mut GarbageCollectionState<BC::Instant>,
    timer_heap: &mut TimerHeap<I, BC>,
    bindings_ctx: &mut BC,
    device_id: &DeviceId,
    ip: SpecifiedAddr<I::Addr>,
    entry: NeighborState<D, BC>,
) -> Result<
    OccupiedEntry<'a, SpecifiedAddr<I::Addr>, NeighborState<D, BC>>,
    TableFullError<NeighborState<D, BC>>,
> {
    if neighbors.len() >= MAX_ENTRIES {
        // If the garbage collector is already scheduled, cancel it on a best
        // effort basis. This may race with the timer firing, but there's no
        // real harm of that happening (e.g. it would result in a single
        // spurious GC run).
        timer_heap.cancel_gc(bindings_ctx);
        collect_garbage_inner(neighbors, gc_state, timer_heap, bindings_ctx, device_id);
    }

    if neighbors.len() >= MAX_ENTRIES {
        return Err(TableFullError { entry });
    }

    match neighbors.entry(ip) {
        Entry::Occupied(_) => panic!("neighbor entry unexpectedly existed"),
        Entry::Vacant(e) => {
            let event_state = entry.to_event_state();
            let entry = e.insert_entry(entry);
            let event = Event::added(device_id, event_state, ip, bindings_ctx.now());
            bindings_ctx.on_event(event);
            Ok(entry)
        }
    }
}

/// Confirm upper-layer forward reachability to the specified neighbor through
/// the specified device.
pub fn confirm_reachable<I, D, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    neighbor: SpecifiedAddr<I::Addr>,
) where
    I: Ip,
    D: LinkDevice,
    BC: NudBindingsContext<I, D, CC::DeviceId>,
    CC: NudContext<I, D, BC>,
{
    core_ctx.with_nud_state_mut_and_sender_ctx(
        device_id,
        |NudState { neighbors, timer_heap, .. }, core_ctx| {
            match neighbors.entry(neighbor) {
                Entry::Vacant(_) => {
                    debug!(
                        "got an upper-layer confirmation for non-existent neighbor entry {}",
                        neighbor
                    );
                }
                Entry::Occupied(e) => match e.into_mut() {
                    NeighborState::Static(_) => {}
                    NeighborState::Dynamic(e) => {
                        // Per [RFC 4861 section 7.3.3]:
                        //
                        //   When a reachability confirmation is received (either through upper-
                        //   layer advice or a solicited Neighbor Advertisement), an entry's state
                        //   changes to REACHABLE.  The one exception is that upper-layer advice has
                        //   no effect on entries in the INCOMPLETE state (e.g., for which no link-
                        //   layer address is cached).
                        //
                        // [RFC 4861 section 7.3.3]: https://tools.ietf.org/html/rfc4861#section-7.3.3
                        let link_address = match e {
                            DynamicNeighborState::Incomplete(_) => return,
                            DynamicNeighborState::Reachable(Reachable {
                                link_address,
                                last_confirmed_at: _,
                            })
                            | DynamicNeighborState::Stale(Stale { link_address })
                            | DynamicNeighborState::Delay(Delay { link_address })
                            | DynamicNeighborState::Probe(Probe {
                                link_address,
                                transmit_counter: _,
                            })
                            | DynamicNeighborState::Unreachable(Unreachable {
                                link_address,
                                mode: _,
                            }) => *link_address,
                        };
                        e.enter_reachable(
                            core_ctx,
                            bindings_ctx,
                            timer_heap,
                            device_id,
                            neighbor,
                            link_address,
                        );
                    }
                },
            }
        },
    );
}

/// Performs a linear scan of the neighbor table, discarding enough entries to
/// bring the total size under `GC_THRESHOLD` if possible.
///
/// Static neighbor entries are never discarded, nor are any entries that are
/// considered to be in use, which is defined as an entry in REACHABLE,
/// INCOMPLETE, DELAY, or PROBE. In other words, the only entries eligible to be
/// discarded are those in STALE or UNREACHABLE. This is reasonable because all
/// other states represent entries to which we have either recently sent packets
/// (REACHABLE, DELAY, PROBE), or which we are actively trying to resolve and
/// for which we have recently queued outgoing packets (INCOMPLETE).
fn collect_garbage_inner<I, D, DeviceId, BC>(
    neighbors: &mut HashMap<SpecifiedAddr<I::Addr>, NeighborState<D, BC>>,
    gc_state: &mut GarbageCollectionState<BC::Instant>,
    timer_heap: &mut TimerHeap<I, BC>,
    bindings_ctx: &mut BC,
    device_id: &DeviceId,
) where
    I: Ip,
    D: LinkDevice,
    DeviceId: StrongDeviceIdentifier,
    BC: NudBindingsContext<I, D, DeviceId>,
{
    let GarbageCollectionState { last_gc, is_dirty } = gc_state;
    // Short circuit if we know there are no discardable entries in the table.
    if !*is_dirty {
        return;
    }

    let max_to_remove = neighbors.len().saturating_sub(GC_THRESHOLD);
    if max_to_remove == 0 {
        return;
    }

    let mut is_still_dirty = false;

    // Define an ordering by priority for garbage collection, such that lower
    // numbers correspond to higher usefulness and therefore lower likelihood of
    // being discarded.
    //
    // TODO(https://fxbug.dev/42075782): once neighbor entries hold a timestamp
    // tracking when they were last updated, consider using this timestamp to break
    // ties between entries in the same state, so that we discard less recently
    // updated entries before more recently updated ones.
    fn gc_priority<D: LinkDevice, BT: NudBindingsTypes<D>>(
        state: &DynamicNeighborState<D, BT>,
    ) -> usize {
        match state {
            DynamicNeighborState::Incomplete(_)
            | DynamicNeighborState::Reachable(_)
            | DynamicNeighborState::Delay(_)
            | DynamicNeighborState::Probe(_) => unreachable!(
                "the netstack should only ever discard STALE or UNREACHABLE entries; \
                    found {:?}",
                state,
            ),
            DynamicNeighborState::Stale(_) => 0,
            DynamicNeighborState::Unreachable(Unreachable {
                link_address: _,
                mode: UnreachableMode::Backoff { probes_sent: _, packet_sent: _ },
            }) => 1,
            DynamicNeighborState::Unreachable(Unreachable {
                link_address: _,
                mode: UnreachableMode::WaitingForPacketSend,
            }) => 2,
        }
    }

    struct SortEntry<'a, K: Eq, D: LinkDevice, BT: NudBindingsTypes<D>> {
        key: K,
        state: &'a mut DynamicNeighborState<D, BT>,
    }

    impl<K: Eq, D: LinkDevice, BT: NudBindingsTypes<D>> PartialEq for SortEntry<'_, K, D, BT> {
        fn eq(&self, other: &Self) -> bool {
            self.key == other.key && gc_priority(self.state) == gc_priority(other.state)
        }
    }
    impl<K: Eq, D: LinkDevice, BT: NudBindingsTypes<D>> Eq for SortEntry<'_, K, D, BT> {}
    impl<K: Eq, D: LinkDevice, BT: NudBindingsTypes<D>> Ord for SortEntry<'_, K, D, BT> {
        fn cmp(&self, other: &Self) -> core::cmp::Ordering {
            // Sort in reverse order so `BinaryHeap` will function as a min-heap rather than
            // a max-heap. This means it will maintain the minimum (i.e. most useful) entry
            // at the top of the heap.
            gc_priority(self.state).cmp(&gc_priority(other.state)).reverse()
        }
    }
    impl<K: Eq, D: LinkDevice, BT: NudBindingsTypes<D>> PartialOrd for SortEntry<'_, K, D, BT> {
        fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
            Some(self.cmp(&other))
        }
    }

    let mut entries_to_remove = BinaryHeap::with_capacity(max_to_remove);
    for (ip, neighbor) in neighbors.iter_mut() {
        match neighbor {
            NeighborState::Static(_) => {
                // Don't discard static entries.
                continue;
            }
            NeighborState::Dynamic(state) => {
                match state {
                    DynamicNeighborState::Incomplete(_)
                    | DynamicNeighborState::Reachable(_)
                    | DynamicNeighborState::Delay(_)
                    | DynamicNeighborState::Probe(_) => {
                        // Don't discard in-use entries.
                        continue;
                    }
                    DynamicNeighborState::Stale(_) | DynamicNeighborState::Unreachable(_) => {
                        // Unconditionally insert the first `max_to_remove` entries.
                        if entries_to_remove.len() < max_to_remove {
                            entries_to_remove.push(SortEntry { key: ip, state });
                            continue;
                        }
                        // If we exceed `max_to_remove`, the table will still
                        // have discardable entries after this run. Prioritize
                        // the removal of neighbors that are less useful (based
                        // on their ordering).
                        is_still_dirty = true;
                        let minimum =
                            entries_to_remove.peek().expect("heap should have at least 1 entry");
                        let candidate = SortEntry { key: ip, state };
                        if &candidate > minimum {
                            let _: SortEntry<'_, _, _, _> = entries_to_remove.pop().unwrap();
                            entries_to_remove.push(candidate);
                        }
                    }
                }
            }
        }
    }

    let entries_to_remove = entries_to_remove
        .into_iter()
        .map(|SortEntry { key: neighbor, state }| {
            state.cancel_timer(bindings_ctx, timer_heap, *neighbor);
            *neighbor
        })
        .collect::<Vec<_>>();

    for neighbor in entries_to_remove {
        assert_matches!(neighbors.remove(&neighbor), Some(_));
        bindings_ctx.on_event(Event::removed(device_id, neighbor, bindings_ctx.now()));
    }

    *last_gc = Some(bindings_ctx.now());
    *is_dirty = is_still_dirty;
}

fn collect_garbage<I, D, CC, BC>(core_ctx: &mut CC, bindings_ctx: &mut BC, device_id: CC::DeviceId)
where
    I: Ip,
    D: LinkDevice,
    BC: NudBindingsContext<I, D, CC::DeviceId>,
    CC: NudContext<I, D, BC>,
{
    core_ctx.with_nud_state_mut(&device_id, |NudState { neighbors, gc_state, timer_heap }, _| {
        collect_garbage_inner(neighbors, gc_state, timer_heap, bindings_ctx, &device_id);
    })
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use ip_test_macro::ip_test;
    use net_declare::{net_ip_v4, net_ip_v6};
    use net_types::UnicastAddr;
    use net_types::ip::{Ipv4Addr, Ipv6Addr};
    use netstack3_base::testutil::{
        FakeBindingsCtx, FakeCoreCtx, FakeInstant, FakeLinkAddress, FakeLinkDevice,
        FakeLinkDeviceId, FakeTimerCtxExt as _, FakeTxMetadata, FakeWeakDeviceId,
    };
    use netstack3_base::{
        CtxPair, InstantContext, IntoCoreTimerCtx, SendFrameContext as _, SendFrameErrorReason,
    };
    use netstack3_hashmap::HashSet;
    use test_case::test_case;

    use super::*;
    use crate::internal::device::nud::api::{NeighborApi, StaticNeighborInsertionError};
    use packet::NestableSerializer as _;

    struct FakeNudContext<I: Ip, D: LinkDevice> {
        state: NudState<I, D, FakeBindingsCtxImpl<I>>,
        counters: NudCounters<I>,
    }

    struct FakeConfigContext {
        retrans_timer: NonZeroDuration,
        nud_config: NudUserConfig,
    }

    struct FakeCoreCtxImpl<I: Ip> {
        nud: FakeNudContext<I, FakeLinkDevice>,
        inner: FakeInnerCtxImpl<I>,
    }

    type FakeInnerCtxImpl<I> =
        FakeCoreCtx<FakeConfigContext, FakeNudMessageMeta<I>, FakeLinkDeviceId>;

    #[derive(Debug, PartialEq, Eq)]
    enum FakeNudMessageMeta<I: Ip> {
        NeighborSolicitation {
            lookup_addr: SpecifiedAddr<I::Addr>,
            remote_link_addr: Option<UnicastAddr<FakeLinkAddress>>,
        },
        IpFrame {
            dst_link_address: UnicastAddr<FakeLinkAddress>,
        },
        IcmpDestUnreachable,
    }

    type FakeBindingsCtxImpl<I> = FakeBindingsCtx<
        NudTimerId<I, FakeLinkDevice, FakeWeakDeviceId<FakeLinkDeviceId>>,
        Event<FakeLinkAddress, FakeLinkDeviceId, I, FakeInstant>,
        (),
        (),
    >;

    impl<I: Ip> FakeCoreCtxImpl<I> {
        fn new(bindings_ctx: &mut FakeBindingsCtxImpl<I>) -> Self {
            Self {
                nud: {
                    FakeNudContext {
                        state: NudState::new::<_, IntoCoreTimerCtx>(
                            bindings_ctx,
                            FakeWeakDeviceId(FakeLinkDeviceId),
                        ),
                        counters: Default::default(),
                    }
                },
                inner: FakeInnerCtxImpl::with_state(FakeConfigContext {
                    retrans_timer: ONE_SECOND,
                    // Use different values from the defaults in tests so we get
                    // coverage that the config is used everywhere and not the
                    // defaults.
                    nud_config: NudUserConfig {
                        max_unicast_solicitations: NonZeroU16::new(4).unwrap(),
                        max_multicast_solicitations: NonZeroU16::new(5).unwrap(),
                        base_reachable_time: NonZeroDuration::from_secs(23).unwrap(),
                        retrans_timer: NonZeroDuration::from_secs(3).unwrap(),
                    },
                }),
            }
        }
    }

    fn new_context<I: Ip>() -> CtxPair<FakeCoreCtxImpl<I>, FakeBindingsCtxImpl<I>> {
        CtxPair::with_default_bindings_ctx(|bindings_ctx| FakeCoreCtxImpl::<I>::new(bindings_ctx))
    }

    impl<I: Ip> DeviceIdContext<FakeLinkDevice> for FakeCoreCtxImpl<I> {
        type DeviceId = FakeLinkDeviceId;
        type WeakDeviceId = FakeWeakDeviceId<FakeLinkDeviceId>;
    }

    impl<I: Ip> NudContext<I, FakeLinkDevice, FakeBindingsCtxImpl<I>> for FakeCoreCtxImpl<I> {
        type ConfigCtx<'a> = FakeConfigContext;

        type SenderCtx<'a> = FakeInnerCtxImpl<I>;

        fn with_nud_state_mut_and_sender_ctx<
            O,
            F: FnOnce(
                &mut NudState<I, FakeLinkDevice, FakeBindingsCtxImpl<I>>,
                &mut Self::SenderCtx<'_>,
            ) -> O,
        >(
            &mut self,
            _device_id: &Self::DeviceId,
            cb: F,
        ) -> O {
            cb(&mut self.nud.state, &mut self.inner)
        }

        fn with_nud_state_mut<
            O,
            F: FnOnce(
                &mut NudState<I, FakeLinkDevice, FakeBindingsCtxImpl<I>>,
                &mut Self::ConfigCtx<'_>,
            ) -> O,
        >(
            &mut self,
            &FakeLinkDeviceId: &FakeLinkDeviceId,
            cb: F,
        ) -> O {
            cb(&mut self.nud.state, &mut self.inner.state)
        }

        fn with_nud_state<
            O,
            F: FnOnce(&NudState<I, FakeLinkDevice, FakeBindingsCtxImpl<I>>) -> O,
        >(
            &mut self,
            &FakeLinkDeviceId: &FakeLinkDeviceId,
            cb: F,
        ) -> O {
            cb(&self.nud.state)
        }

        fn send_neighbor_solicitation(
            &mut self,
            bindings_ctx: &mut FakeBindingsCtxImpl<I>,
            &FakeLinkDeviceId: &FakeLinkDeviceId,
            lookup_addr: SpecifiedAddr<I::Addr>,
            remote_link_addr: Option<UnicastAddr<FakeLinkAddress>>,
        ) {
            self.inner
                .send_frame(
                    bindings_ctx,
                    FakeNudMessageMeta::NeighborSolicitation { lookup_addr, remote_link_addr },
                    Buf::new(Vec::new(), ..),
                )
                .unwrap()
        }
    }

    impl<I: NudIcmpIpExt> NudIcmpContext<I, FakeLinkDevice, FakeBindingsCtxImpl<I>>
        for FakeCoreCtxImpl<I>
    {
        fn send_icmp_dest_unreachable(
            &mut self,
            bindings_ctx: &mut FakeBindingsCtxImpl<I>,
            frame: Buf<Vec<u8>>,
            _device_id: Option<&Self::DeviceId>,
            _original_src_ip: SocketIpAddr<I::Addr>,
            _original_dst_ip: SocketIpAddr<I::Addr>,
            _header_len: usize,
            _proto: I::Proto,
            _metadata: I::Metadata,
        ) {
            self.inner
                .send_frame(bindings_ctx, FakeNudMessageMeta::IcmpDestUnreachable, frame)
                .unwrap()
        }
    }

    impl<I: Ip> CounterContext<NudCounters<I>> for FakeCoreCtxImpl<I> {
        fn counters(&self) -> &NudCounters<I> {
            &self.nud.counters
        }
    }

    impl<I: Ip> NudConfigContext<I> for FakeConfigContext {
        fn retransmit_timeout(&mut self) -> NonZeroDuration {
            self.retrans_timer
        }

        fn with_nud_user_config<O, F: FnOnce(&NudUserConfig) -> O>(&mut self, cb: F) -> O {
            cb(&self.nud_config)
        }

        fn override_lock_time(&mut self) -> Duration {
            Duration::ZERO
        }
    }

    impl<I: Ip> NudSenderContext<I, FakeLinkDevice, FakeBindingsCtxImpl<I>> for FakeInnerCtxImpl<I> {
        fn send_ip_packet_to_neighbor_link_addr<S>(
            &mut self,
            bindings_ctx: &mut FakeBindingsCtxImpl<I>,
            dst_link_address: UnicastAddr<FakeLinkAddress>,
            body: S,
            _tx_meta: FakeTxMetadata,
        ) -> Result<(), SendFrameError<S>>
        where
            S: NetworkSerializer,
            S::Buffer: BufferMut,
        {
            self.send_frame(bindings_ctx, FakeNudMessageMeta::IpFrame { dst_link_address }, body)
        }
    }

    impl<I: Ip> NudConfigContext<I> for FakeInnerCtxImpl<I> {
        fn retransmit_timeout(&mut self) -> NonZeroDuration {
            <FakeConfigContext as NudConfigContext<I>>::retransmit_timeout(&mut self.state)
        }

        fn with_nud_user_config<O, F: FnOnce(&NudUserConfig) -> O>(&mut self, cb: F) -> O {
            <FakeConfigContext as NudConfigContext<I>>::with_nud_user_config(&mut self.state, cb)
        }

        fn override_lock_time(&mut self) -> Duration {
            <FakeConfigContext as NudConfigContext<I>>::override_lock_time(&mut self.state)
        }
    }

    const ONE_SECOND: NonZeroDuration = NonZeroDuration::from_secs(1).unwrap();

    #[track_caller]
    fn check_lookup_has<I: Ip>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        lookup_addr: SpecifiedAddr<I::Addr>,
        expected_link_addr: UnicastAddr<FakeLinkAddress>,
    ) {
        let entry = assert_matches!(
            core_ctx.nud.state.neighbors.get(&lookup_addr),
            Some(entry @ (
                NeighborState::Dynamic(
                    DynamicNeighborState::Reachable (Reachable { link_address, last_confirmed_at: _ })
                    | DynamicNeighborState::Stale (Stale { link_address })
                    | DynamicNeighborState::Delay (Delay { link_address })
                    | DynamicNeighborState::Probe (Probe { link_address, transmit_counter: _ })
                    | DynamicNeighborState::Unreachable (Unreachable { link_address, mode: _ })
                )
                | NeighborState::Static(link_address)
            )) => {
                assert_eq!(link_address, &expected_link_addr);
                entry
            }
        );
        match entry {
            NeighborState::Dynamic(DynamicNeighborState::Incomplete { .. }) => {
                unreachable!("entry must be static, REACHABLE, or STALE")
            }
            NeighborState::Dynamic(DynamicNeighborState::Reachable { .. }) => {
                core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
                    bindings_ctx,
                    [(
                        lookup_addr,
                        NudEvent::ReachableTime,
                        core_ctx.inner.base_reachable_time().get(),
                    )],
                );
            }
            NeighborState::Dynamic(DynamicNeighborState::Delay { .. }) => {
                core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
                    bindings_ctx,
                    [(lookup_addr, NudEvent::DelayFirstProbe, DELAY_FIRST_PROBE_TIME.get())],
                );
            }
            NeighborState::Dynamic(DynamicNeighborState::Probe { .. }) => {
                core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
                    bindings_ctx,
                    [(
                        lookup_addr,
                        NudEvent::RetransmitUnicastProbe,
                        core_ctx.inner.state.retrans_timer.get(),
                    )],
                );
            }
            NeighborState::Dynamic(DynamicNeighborState::Unreachable(Unreachable {
                link_address: _,
                mode,
            })) => {
                let instant = match mode {
                    UnreachableMode::WaitingForPacketSend => None,
                    mode @ UnreachableMode::Backoff { .. } => {
                        let duration =
                            mode.next_backoff_retransmit_timeout::<I, _>(&mut core_ctx.inner.state);
                        Some(bindings_ctx.now() + duration.get())
                    }
                };
                if let Some(instant) = instant {
                    core_ctx.nud.state.timer_heap.neighbor.assert_timers([(
                        lookup_addr,
                        NudEvent::RetransmitUnicastProbe,
                        instant,
                    )]);
                }
            }
            NeighborState::Dynamic(DynamicNeighborState::Stale { .. })
            | NeighborState::Static(_) => bindings_ctx.timers.assert_no_timers_installed(),
        }
    }

    trait TestIpExt: NudIcmpIpExt {
        const LOOKUP_ADDR1: SpecifiedAddr<Self::Addr>;
        const LOOKUP_ADDR2: SpecifiedAddr<Self::Addr>;
        const LOOKUP_ADDR3: SpecifiedAddr<Self::Addr>;
    }

    impl TestIpExt for Ipv4 {
        // Safe because the address is non-zero.
        const LOOKUP_ADDR1: SpecifiedAddr<Ipv4Addr> =
            unsafe { SpecifiedAddr::new_unchecked(net_ip_v4!("192.168.0.1")) };
        const LOOKUP_ADDR2: SpecifiedAddr<Ipv4Addr> =
            unsafe { SpecifiedAddr::new_unchecked(net_ip_v4!("192.168.0.2")) };
        const LOOKUP_ADDR3: SpecifiedAddr<Ipv4Addr> =
            unsafe { SpecifiedAddr::new_unchecked(net_ip_v4!("192.168.0.3")) };
    }

    impl TestIpExt for Ipv6 {
        // Safe because the address is non-zero.
        const LOOKUP_ADDR1: SpecifiedAddr<Ipv6Addr> =
            unsafe { SpecifiedAddr::new_unchecked(net_ip_v6!("fe80::1")) };
        const LOOKUP_ADDR2: SpecifiedAddr<Ipv6Addr> =
            unsafe { SpecifiedAddr::new_unchecked(net_ip_v6!("fe80::2")) };
        const LOOKUP_ADDR3: SpecifiedAddr<Ipv6Addr> =
            unsafe { SpecifiedAddr::new_unchecked(net_ip_v6!("fe80::3")) };
    }

    const LINK_ADDR1: UnicastAddr<FakeLinkAddress> =
        unsafe { UnicastAddr::new_unchecked(FakeLinkAddress([2])) };
    const LINK_ADDR2: UnicastAddr<FakeLinkAddress> =
        unsafe { UnicastAddr::new_unchecked(FakeLinkAddress([4])) };
    const LINK_ADDR3: UnicastAddr<FakeLinkAddress> =
        unsafe { UnicastAddr::new_unchecked(FakeLinkAddress([6])) };

    impl<I: Ip, L: LinkDevice> NudTimerId<I, L, FakeWeakDeviceId<FakeLinkDeviceId>> {
        fn neighbor() -> Self {
            Self {
                device_id: FakeWeakDeviceId(FakeLinkDeviceId),
                timer_type: NudTimerType::Neighbor,
                _marker: PhantomData,
            }
        }

        fn garbage_collection() -> Self {
            Self {
                device_id: FakeWeakDeviceId(FakeLinkDeviceId),
                timer_type: NudTimerType::GarbageCollection,
                _marker: PhantomData,
            }
        }
    }

    fn queue_ip_packet_to_unresolved_neighbor<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        neighbor: SpecifiedAddr<I::Addr>,
        pending_frames: &mut VecDeque<Buf<Vec<u8>>>,
        body: u8,
        expect_event: bool,
    ) {
        let body = [body];
        assert_eq!(
            NudHandler::send_ip_packet_to_neighbor(
                core_ctx,
                bindings_ctx,
                &FakeLinkDeviceId,
                neighbor,
                Buf::new(body, ..),
                FakeTxMetadata::default(),
            ),
            Ok(())
        );

        let max_multicast_solicit = core_ctx.inner.max_multicast_solicit().get();

        pending_frames.push_back(Buf::new(body.to_vec(), ..));

        assert_neighbor_state_with_ip(
            core_ctx,
            bindings_ctx,
            neighbor,
            DynamicNeighborState::Incomplete(Incomplete {
                transmit_counter: NonZeroU16::new(max_multicast_solicit - 1),
                pending_frames: pending_frames
                    .iter()
                    .cloned()
                    .map(|buf| (buf, FakeTxMetadata::default()))
                    .collect(),
                notifiers: Vec::new(),
                _marker: PhantomData,
            }),
            expect_event.then_some(ExpectedEvent::Added),
        );

        core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
            bindings_ctx,
            [(neighbor, NudEvent::RetransmitMulticastProbe, ONE_SECOND.get())],
        );
    }

    fn init_incomplete_neighbor_with_ip<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        ip_address: SpecifiedAddr<I::Addr>,
        take_probe: bool,
    ) -> VecDeque<Buf<Vec<u8>>> {
        let mut pending_frames = VecDeque::new();
        queue_ip_packet_to_unresolved_neighbor(
            core_ctx,
            bindings_ctx,
            ip_address,
            &mut pending_frames,
            1,
            true, /* expect_event */
        );
        if take_probe {
            assert_neighbor_probe_sent_for_ip(core_ctx, ip_address, None);
        }
        pending_frames
    }

    fn init_incomplete_neighbor<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        take_probe: bool,
    ) -> VecDeque<Buf<Vec<u8>>> {
        init_incomplete_neighbor_with_ip(core_ctx, bindings_ctx, I::LOOKUP_ADDR1, take_probe)
    }

    fn init_stale_neighbor_with_ip<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        ip_address: SpecifiedAddr<I::Addr>,
        link_address: UnicastAddr<FakeLinkAddress>,
    ) {
        NudHandler::handle_neighbor_update(
            core_ctx,
            bindings_ctx,
            &FakeLinkDeviceId,
            ip_address,
            DynamicNeighborUpdateSource::Probe { link_address },
        );
        assert_neighbor_state_with_ip(
            core_ctx,
            bindings_ctx,
            ip_address,
            DynamicNeighborState::Stale(Stale { link_address }),
            Some(ExpectedEvent::Added),
        );
    }

    fn init_stale_neighbor<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        link_address: UnicastAddr<FakeLinkAddress>,
    ) {
        init_stale_neighbor_with_ip(core_ctx, bindings_ctx, I::LOOKUP_ADDR1, link_address);
    }

    fn init_reachable_neighbor_with_ip<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        ip_address: SpecifiedAddr<I::Addr>,
        link_address: UnicastAddr<FakeLinkAddress>,
    ) {
        let queued_frame =
            init_incomplete_neighbor_with_ip(core_ctx, bindings_ctx, ip_address, true);
        NudHandler::handle_neighbor_update(
            core_ctx,
            bindings_ctx,
            &FakeLinkDeviceId,
            ip_address,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: Some(link_address),
                flags: ConfirmationFlags { solicited_flag: true, override_flag: false },
            },
        );
        assert_neighbor_state_with_ip(
            core_ctx,
            bindings_ctx,
            ip_address,
            DynamicNeighborState::Reachable(Reachable {
                link_address,
                last_confirmed_at: bindings_ctx.now(),
            }),
            Some(ExpectedEvent::Changed),
        );
        assert_pending_frame_sent(core_ctx, queued_frame, link_address);
    }

    fn init_reachable_neighbor<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        link_address: UnicastAddr<FakeLinkAddress>,
    ) {
        init_reachable_neighbor_with_ip(core_ctx, bindings_ctx, I::LOOKUP_ADDR1, link_address);
    }

    fn init_delay_neighbor_with_ip<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        ip_address: SpecifiedAddr<I::Addr>,
        link_address: UnicastAddr<FakeLinkAddress>,
    ) {
        init_stale_neighbor_with_ip(core_ctx, bindings_ctx, ip_address, link_address);
        assert_eq!(
            NudHandler::send_ip_packet_to_neighbor(
                core_ctx,
                bindings_ctx,
                &FakeLinkDeviceId,
                ip_address,
                Buf::new([1], ..),
                FakeTxMetadata::default(),
            ),
            Ok(())
        );
        assert_neighbor_state_with_ip(
            core_ctx,
            bindings_ctx,
            ip_address,
            DynamicNeighborState::Delay(Delay { link_address }),
            Some(ExpectedEvent::Changed),
        );
        assert_eq!(
            core_ctx.inner.take_frames(),
            vec![(FakeNudMessageMeta::IpFrame { dst_link_address: LINK_ADDR1 }, vec![1])],
        );
    }

    fn init_delay_neighbor<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        link_address: UnicastAddr<FakeLinkAddress>,
    ) {
        init_delay_neighbor_with_ip(core_ctx, bindings_ctx, I::LOOKUP_ADDR1, link_address);
    }

    fn init_probe_neighbor_with_ip<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        ip_address: SpecifiedAddr<I::Addr>,
        link_address: UnicastAddr<FakeLinkAddress>,
        take_probe: bool,
    ) {
        init_delay_neighbor_with_ip(core_ctx, bindings_ctx, ip_address, link_address);
        let max_unicast_solicit = core_ctx.inner.max_unicast_solicit().get();
        core_ctx.nud.state.timer_heap.neighbor.assert_top(&ip_address, &NudEvent::DelayFirstProbe);
        assert_eq!(
            bindings_ctx.trigger_timers_for(DELAY_FIRST_PROBE_TIME.into(), core_ctx),
            [NudTimerId::neighbor()]
        );
        assert_neighbor_state_with_ip(
            core_ctx,
            bindings_ctx,
            ip_address,
            DynamicNeighborState::Probe(Probe {
                link_address,
                transmit_counter: NonZeroU16::new(max_unicast_solicit - 1),
            }),
            Some(ExpectedEvent::Changed),
        );
        if take_probe {
            assert_neighbor_probe_sent_for_ip(core_ctx, ip_address, Some(LINK_ADDR1));
        }
    }

    fn init_probe_neighbor<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        link_address: UnicastAddr<FakeLinkAddress>,
        take_probe: bool,
    ) {
        init_probe_neighbor_with_ip(
            core_ctx,
            bindings_ctx,
            I::LOOKUP_ADDR1,
            link_address,
            take_probe,
        );
    }

    fn init_unreachable_neighbor_with_ip<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        ip_address: SpecifiedAddr<I::Addr>,
        link_address: UnicastAddr<FakeLinkAddress>,
    ) {
        init_probe_neighbor_with_ip(core_ctx, bindings_ctx, ip_address, link_address, false);
        let retransmit_timeout = core_ctx.inner.retransmit_timeout();
        let max_unicast_solicit = core_ctx.inner.max_unicast_solicit().get();
        for _ in 0..max_unicast_solicit {
            assert_neighbor_probe_sent_for_ip(core_ctx, ip_address, Some(LINK_ADDR1));
            assert_eq!(
                bindings_ctx.trigger_timers_for(retransmit_timeout.into(), core_ctx),
                [NudTimerId::neighbor()]
            );
        }
        assert_neighbor_state_with_ip(
            core_ctx,
            bindings_ctx,
            ip_address,
            DynamicNeighborState::Unreachable(Unreachable {
                link_address,
                mode: UnreachableMode::WaitingForPacketSend,
            }),
            Some(ExpectedEvent::Changed),
        );
    }

    fn init_unreachable_neighbor<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        link_address: UnicastAddr<FakeLinkAddress>,
    ) {
        init_unreachable_neighbor_with_ip(core_ctx, bindings_ctx, I::LOOKUP_ADDR1, link_address);
    }

    #[derive(PartialEq, Eq, Debug, Clone, Copy)]
    enum InitialState {
        Incomplete,
        Stale,
        Reachable,
        Delay,
        Probe,
        Unreachable,
    }

    fn init_neighbor_in_state<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        state: InitialState,
    ) -> DynamicNeighborState<FakeLinkDevice, FakeBindingsCtxImpl<I>> {
        match state {
            InitialState::Incomplete => {
                let _: VecDeque<Buf<Vec<u8>>> =
                    init_incomplete_neighbor(core_ctx, bindings_ctx, true);
            }
            InitialState::Reachable => {
                init_reachable_neighbor(core_ctx, bindings_ctx, LINK_ADDR1);
            }
            InitialState::Stale => {
                init_stale_neighbor(core_ctx, bindings_ctx, LINK_ADDR1);
            }
            InitialState::Delay => {
                init_delay_neighbor(core_ctx, bindings_ctx, LINK_ADDR1);
            }
            InitialState::Probe => {
                init_probe_neighbor(core_ctx, bindings_ctx, LINK_ADDR1, true);
            }
            InitialState::Unreachable => {
                init_unreachable_neighbor(core_ctx, bindings_ctx, LINK_ADDR1);
            }
        }
        assert_matches!(core_ctx.nud.state.neighbors.get(&I::LOOKUP_ADDR1),
            Some(NeighborState::Dynamic(state)) => state.clone()
        )
    }

    #[track_caller]
    fn init_static_neighbor_with_ip<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        ip_address: SpecifiedAddr<I::Addr>,
        link_address: UnicastAddr<FakeLinkAddress>,
        expected_event: ExpectedEvent,
    ) {
        let mut ctx = CtxPair { core_ctx, bindings_ctx };
        NeighborApi::new(&mut ctx)
            .insert_static_entry(&FakeLinkDeviceId, *ip_address, link_address)
            .unwrap();
        assert_eq!(
            ctx.bindings_ctx.take_events(),
            [Event {
                device: FakeLinkDeviceId,
                addr: ip_address,
                kind: match expected_event {
                    ExpectedEvent::Added => EventKind::Added(EventState::Static(link_address)),
                    ExpectedEvent::Changed => EventKind::Changed(EventState::Static(link_address)),
                },
                at: ctx.bindings_ctx.now(),
            }],
        );
    }

    #[track_caller]
    fn init_static_neighbor<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        link_address: UnicastAddr<FakeLinkAddress>,
        expected_event: ExpectedEvent,
    ) {
        init_static_neighbor_with_ip(
            core_ctx,
            bindings_ctx,
            I::LOOKUP_ADDR1,
            link_address,
            expected_event,
        );
    }

    #[track_caller]
    fn delete_neighbor<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
    ) {
        let mut ctx = CtxPair { core_ctx, bindings_ctx };
        NeighborApi::new(&mut ctx)
            .remove_entry(&FakeLinkDeviceId, *I::LOOKUP_ADDR1)
            .expect("neighbor entry should exist");
        assert_eq!(
            ctx.bindings_ctx.take_events(),
            [Event::removed(&FakeLinkDeviceId, I::LOOKUP_ADDR1, ctx.bindings_ctx.now())],
        );
    }

    #[track_caller]
    fn assert_neighbor_state<I: TestIpExt>(
        core_ctx: &FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        state: DynamicNeighborState<FakeLinkDevice, FakeBindingsCtxImpl<I>>,
        event_kind: Option<ExpectedEvent>,
    ) {
        assert_neighbor_state_with_ip(core_ctx, bindings_ctx, I::LOOKUP_ADDR1, state, event_kind);
    }

    #[derive(Clone, Copy, Debug)]
    enum ExpectedEvent {
        Added,
        Changed,
    }

    #[track_caller]
    fn assert_neighbor_state_with_ip<I: TestIpExt>(
        core_ctx: &FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        neighbor: SpecifiedAddr<I::Addr>,
        state: DynamicNeighborState<FakeLinkDevice, FakeBindingsCtxImpl<I>>,
        expected_event: Option<ExpectedEvent>,
    ) {
        if let Some(expected_event) = expected_event {
            let event_state = EventState::Dynamic(state.to_event_dynamic_state());
            assert_eq!(
                bindings_ctx.take_events(),
                [Event {
                    device: FakeLinkDeviceId,
                    addr: neighbor,
                    kind: match expected_event {
                        ExpectedEvent::Added => EventKind::Added(event_state),
                        ExpectedEvent::Changed => EventKind::Changed(event_state),
                    },
                    at: bindings_ctx.now(),
                }],
            );
        }

        assert_eq!(
            core_ctx.nud.state.neighbors.get(&neighbor),
            Some(&NeighborState::Dynamic(state))
        );
    }

    #[track_caller]
    fn assert_pending_frame_sent<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        pending_frames: VecDeque<Buf<Vec<u8>>>,
        link_address: UnicastAddr<FakeLinkAddress>,
    ) {
        assert_eq!(
            core_ctx.inner.take_frames(),
            pending_frames
                .into_iter()
                .map(|f| (
                    FakeNudMessageMeta::IpFrame { dst_link_address: link_address },
                    f.as_ref().to_vec(),
                ))
                .collect::<Vec<_>>()
        );
    }

    #[track_caller]
    fn assert_neighbor_probe_sent_for_ip<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        ip_address: SpecifiedAddr<I::Addr>,
        link_address: Option<UnicastAddr<FakeLinkAddress>>,
    ) {
        assert_eq!(
            core_ctx.inner.take_frames(),
            [(
                FakeNudMessageMeta::NeighborSolicitation {
                    lookup_addr: ip_address,
                    remote_link_addr: link_address,
                },
                Vec::new()
            )]
        );
    }

    #[track_caller]
    fn assert_neighbor_probe_sent<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        link_address: Option<UnicastAddr<FakeLinkAddress>>,
    ) {
        assert_neighbor_probe_sent_for_ip(core_ctx, I::LOOKUP_ADDR1, link_address);
    }

    #[track_caller]
    fn assert_neighbor_removed_with_ip<I: TestIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
        neighbor: SpecifiedAddr<I::Addr>,
    ) {
        super::testutil::assert_neighbor_unknown(core_ctx, FakeLinkDeviceId, neighbor);
        assert_eq!(
            bindings_ctx.take_events(),
            [Event::removed(&FakeLinkDeviceId, neighbor, bindings_ctx.now())],
        );
    }

    #[ip_test(I)]
    fn serialization_failure_doesnt_schedule_timer<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Try to send a packet for which serialization will fail due to a size
        // constraint.
        let packet = Buf::new([0; 2], ..).with_size_limit(1);

        let err = assert_matches!(
            NudHandler::send_ip_packet_to_neighbor(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeLinkDeviceId,
                I::LOOKUP_ADDR1,
                packet,
                FakeTxMetadata::default(),
            ),
            Err(ErrorAndSerializer { error, serializer: _ }) => error
        );
        assert_eq!(err, SendFrameErrorReason::SizeConstraintsViolation);

        // The neighbor should not be inserted in the table, a probe should not be sent,
        // and no retransmission timer should be scheduled.
        super::testutil::assert_neighbor_unknown(&mut core_ctx, FakeLinkDeviceId, I::LOOKUP_ADDR1);
        assert_eq!(core_ctx.inner.take_frames(), []);
        bindings_ctx.timers.assert_no_timers_installed();
    }

    #[ip_test(I)]
    fn incomplete_to_stale_on_probe<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor in INCOMPLETE.
        let queued_frame = init_incomplete_neighbor(&mut core_ctx, &mut bindings_ctx, true);

        // Handle an incoming probe from that neighbor.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Probe { link_address: LINK_ADDR1 },
        );

        // Neighbor should now be in STALE, per RFC 4861 section 7.2.3.
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Stale(Stale { link_address: LINK_ADDR1 }),
            Some(ExpectedEvent::Changed),
        );
        assert_pending_frame_sent(&mut core_ctx, queued_frame, LINK_ADDR1);
    }

    #[ip_test(I)]
    #[test_case(true, true; "solicited override")]
    #[test_case(true, false; "solicited non-override")]
    #[test_case(false, true; "unsolicited override")]
    #[test_case(false, false; "unsolicited non-override")]
    fn incomplete_on_confirmation<I: TestIpExt>(solicited_flag: bool, override_flag: bool) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor in INCOMPLETE.
        let queued_frame = init_incomplete_neighbor(&mut core_ctx, &mut bindings_ctx, true);

        // Handle an incoming confirmation from that neighbor.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: Some(LINK_ADDR1),
                flags: ConfirmationFlags { solicited_flag, override_flag },
            },
        );

        let expected_state = if solicited_flag {
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR1,
                last_confirmed_at: bindings_ctx.now(),
            })
        } else {
            DynamicNeighborState::Stale(Stale { link_address: LINK_ADDR1 })
        };
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            expected_state,
            Some(ExpectedEvent::Changed),
        );
        assert_pending_frame_sent(&mut core_ctx, queued_frame, LINK_ADDR1);
    }

    #[ip_test(I)]
    fn reachable_to_stale_on_timeout<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor in REACHABLE.
        init_reachable_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1);

        // After reachable time, neighbor should transition to STALE.
        assert_eq!(
            bindings_ctx
                .trigger_timers_for(core_ctx.inner.base_reachable_time().into(), &mut core_ctx,),
            [NudTimerId::neighbor()]
        );
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Stale(Stale { link_address: LINK_ADDR1 }),
            Some(ExpectedEvent::Changed),
        );
    }

    #[ip_test(I)]
    #[test_case(InitialState::Reachable, true; "reachable with different address")]
    #[test_case(InitialState::Reachable, false; "reachable with same address")]
    #[test_case(InitialState::Stale, true; "stale with different address")]
    #[test_case(InitialState::Stale, false; "stale with same address")]
    #[test_case(InitialState::Delay, true; "delay with different address")]
    #[test_case(InitialState::Delay, false; "delay with same address")]
    #[test_case(InitialState::Probe, true; "probe with different address")]
    #[test_case(InitialState::Probe, false; "probe with same address")]
    #[test_case(InitialState::Unreachable, true; "unreachable with different address")]
    #[test_case(InitialState::Unreachable, false; "unreachable with same address")]
    fn transition_to_stale_on_probe_with_different_address<I: TestIpExt>(
        initial_state: InitialState,
        update_link_address: bool,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor.
        let initial_state = init_neighbor_in_state(&mut core_ctx, &mut bindings_ctx, initial_state);

        // Handle an incoming probe, possibly with an updated link address.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Probe {
                link_address: if update_link_address { LINK_ADDR2 } else { LINK_ADDR1 },
            },
        );

        // If the link address was updated, the neighbor should now be in STALE with the
        // new link address, per RFC 4861 section 7.2.3.
        //
        // If the link address is the same, the entry should remain in its initial
        // state.
        let expected_state = if update_link_address {
            DynamicNeighborState::Stale(Stale { link_address: LINK_ADDR2 })
        } else {
            initial_state
        };
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            expected_state,
            update_link_address.then_some(ExpectedEvent::Changed),
        );
    }

    #[ip_test(I)]
    #[test_case(InitialState::Reachable, true; "reachable with override flag set")]
    #[test_case(InitialState::Reachable, false; "reachable with override flag not set")]
    #[test_case(InitialState::Stale, true; "stale with override flag set")]
    #[test_case(InitialState::Stale, false; "stale with override flag not set")]
    #[test_case(InitialState::Delay, true; "delay with override flag set")]
    #[test_case(InitialState::Delay, false; "delay with override flag not set")]
    #[test_case(InitialState::Probe, true; "probe with override flag set")]
    #[test_case(InitialState::Probe, false; "probe with override flag not set")]
    #[test_case(InitialState::Unreachable, true; "unreachable with override flag set")]
    #[test_case(InitialState::Unreachable, false; "unreachable with override flag not set")]
    fn transition_to_reachable_on_solicited_confirmation_same_address<I: TestIpExt>(
        initial_state: InitialState,
        override_flag: bool,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor.
        let _ = init_neighbor_in_state(&mut core_ctx, &mut bindings_ctx, initial_state);

        // Handle an incoming solicited confirmation.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: Some(LINK_ADDR1),
                flags: ConfirmationFlags { solicited_flag: true, override_flag },
            },
        );

        // Neighbor should now be in REACHABLE, per RFC 4861 section 7.2.5.
        let now = bindings_ctx.now();
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR1,
                last_confirmed_at: now,
            }),
            (initial_state != InitialState::Reachable).then_some(ExpectedEvent::Changed),
        );
    }

    #[ip_test(I)]
    #[test_case(InitialState::Reachable; "reachable")]
    #[test_case(InitialState::Stale; "stale")]
    #[test_case(InitialState::Delay; "delay")]
    #[test_case(InitialState::Probe; "probe")]
    #[test_case(InitialState::Unreachable; "unreachable")]
    fn transition_to_stale_on_unsolicited_override_confirmation_with_different_address<
        I: TestIpExt,
    >(
        initial_state: InitialState,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor.
        let _ = init_neighbor_in_state(&mut core_ctx, &mut bindings_ctx, initial_state);

        // Handle an incoming unsolicited override confirmation with a different link address.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: Some(LINK_ADDR2),
                flags: ConfirmationFlags { solicited_flag: false, override_flag: true },
            },
        );

        // Neighbor should now be in STALE, per RFC 4861 section 7.2.5.
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Stale(Stale { link_address: LINK_ADDR2 }),
            Some(ExpectedEvent::Changed),
        );
    }

    #[ip_test(I)]
    #[test_case(InitialState::Reachable, true; "reachable with override flag set")]
    #[test_case(InitialState::Reachable, false; "reachable with override flag not set")]
    #[test_case(InitialState::Stale, true; "stale with override flag set")]
    #[test_case(InitialState::Stale, false; "stale with override flag not set")]
    #[test_case(InitialState::Delay, true; "delay with override flag set")]
    #[test_case(InitialState::Delay, false; "delay with override flag not set")]
    #[test_case(InitialState::Probe, true; "probe with override flag set")]
    #[test_case(InitialState::Probe, false; "probe with override flag not set")]
    #[test_case(InitialState::Unreachable, true; "unreachable with override flag set")]
    #[test_case(InitialState::Unreachable, false; "unreachable with override flag not set")]
    fn noop_on_unsolicited_confirmation_with_same_address<I: TestIpExt>(
        initial_state: InitialState,
        override_flag: bool,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor.
        let expected_state =
            init_neighbor_in_state(&mut core_ctx, &mut bindings_ctx, initial_state);

        // Handle an incoming unsolicited confirmation with the same link address.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: Some(LINK_ADDR1),
                flags: ConfirmationFlags { solicited_flag: false, override_flag },
            },
        );

        // Neighbor should not have been updated.
        assert_neighbor_state(&core_ctx, &mut bindings_ctx, expected_state, None);
    }

    #[ip_test(I)]
    #[test_case(InitialState::Reachable; "reachable")]
    #[test_case(InitialState::Stale; "stale")]
    #[test_case(InitialState::Delay; "delay")]
    #[test_case(InitialState::Probe; "probe")]
    #[test_case(InitialState::Unreachable; "unreachable")]
    fn transition_to_reachable_on_solicited_override_confirmation_with_different_address<
        I: TestIpExt,
    >(
        initial_state: InitialState,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor.
        let _ = init_neighbor_in_state(&mut core_ctx, &mut bindings_ctx, initial_state);

        // Handle an incoming solicited override confirmation with a different link address.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: Some(LINK_ADDR2),
                flags: ConfirmationFlags { solicited_flag: true, override_flag: true },
            },
        );

        // Neighbor should now be in REACHABLE, per RFC 4861 section 7.2.5.
        let now = bindings_ctx.now();
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR2,
                last_confirmed_at: now,
            }),
            Some(ExpectedEvent::Changed),
        );
    }

    #[ip_test(I)]
    fn reachable_to_reachable_on_probe_with_same_address<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor in REACHABLE.
        init_reachable_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1);

        // Handle an incoming probe with the same link address.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Probe { link_address: LINK_ADDR1 },
        );

        // Neighbor should still be in REACHABLE with the same link address.
        let now = bindings_ctx.now();
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR1,
                last_confirmed_at: now,
            }),
            None,
        );
    }

    #[ip_test(I)]
    #[test_case(true; "solicited")]
    #[test_case(false; "unsolicited")]
    fn reachable_to_stale_on_non_override_confirmation_with_different_address<I: TestIpExt>(
        solicited_flag: bool,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor in REACHABLE.
        init_reachable_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1);

        // Handle an incoming non-override confirmation with a different link address.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: Some(LINK_ADDR2),
                flags: ConfirmationFlags { override_flag: false, solicited_flag },
            },
        );

        // Neighbor should now be in STALE, with the *same* link address as was
        // previously cached, per RFC 4861 section 7.2.5.
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Stale(Stale { link_address: LINK_ADDR1 }),
            Some(ExpectedEvent::Changed),
        );
    }

    #[ip_test(I)]
    #[test_case(InitialState::Stale, true; "stale solicited")]
    #[test_case(InitialState::Stale, false; "stale unsolicited")]
    #[test_case(InitialState::Delay, true; "delay solicited")]
    #[test_case(InitialState::Delay, false; "delay unsolicited")]
    #[test_case(InitialState::Probe, true; "probe solicited")]
    #[test_case(InitialState::Probe, false; "probe unsolicited")]
    #[test_case(InitialState::Unreachable, true; "unreachable solicited")]
    #[test_case(InitialState::Unreachable, false; "unreachable unsolicited")]
    fn noop_on_non_override_confirmation_with_different_address<I: TestIpExt>(
        initial_state: InitialState,
        solicited_flag: bool,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor.
        let initial_state = init_neighbor_in_state(&mut core_ctx, &mut bindings_ctx, initial_state);

        // Handle an incoming non-override confirmation with a different link address.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: Some(LINK_ADDR2),
                flags: ConfirmationFlags { override_flag: false, solicited_flag },
            },
        );

        // Neighbor should still be in the original state; the link address should *not*
        // have been updated.
        assert_neighbor_state(&core_ctx, &mut bindings_ctx, initial_state, None);
    }

    #[ip_test(I)]
    fn stale_to_delay_on_packet_sent<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor in STALE.
        init_stale_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1);

        // Send a packet to the neighbor.
        let body = 1;
        assert_eq!(
            NudHandler::send_ip_packet_to_neighbor(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeLinkDeviceId,
                I::LOOKUP_ADDR1,
                Buf::new([body], ..),
                FakeTxMetadata::default(),
            ),
            Ok(())
        );

        // Neighbor should be in DELAY.
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Delay(Delay { link_address: LINK_ADDR1 }),
            Some(ExpectedEvent::Changed),
        );
        core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
            &mut bindings_ctx,
            [(I::LOOKUP_ADDR1, NudEvent::DelayFirstProbe, DELAY_FIRST_PROBE_TIME.get())],
        );
        assert_pending_frame_sent(
            &mut core_ctx,
            VecDeque::from([Buf::new(vec![body], ..)]),
            LINK_ADDR1,
        );
    }

    #[ip_test(I)]
    #[test_case(InitialState::Delay,
                NudEvent::DelayFirstProbe;
                "delay to probe")]
    #[test_case(InitialState::Probe,
                NudEvent::RetransmitUnicastProbe;
                "probe retransmit unicast probe")]
    fn delay_or_probe_to_probe_on_timeout<I: TestIpExt>(
        initial_state: InitialState,
        expected_initial_event: NudEvent,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor.
        let _ = init_neighbor_in_state(&mut core_ctx, &mut bindings_ctx, initial_state);

        let max_unicast_solicit = core_ctx.inner.max_unicast_solicit().get();

        // If the neighbor started in DELAY, then after DELAY_FIRST_PROBE_TIME, the
        // neighbor should transition to PROBE and send out a unicast probe.
        //
        // If the neighbor started in PROBE, then after RetransTimer expires, the
        // neighbor should remain in PROBE and retransmit a unicast probe.
        let (time, transmit_counter) = match initial_state {
            InitialState::Delay => {
                (DELAY_FIRST_PROBE_TIME, NonZeroU16::new(max_unicast_solicit - 1))
            }
            InitialState::Probe => {
                (core_ctx.inner.state.retrans_timer, NonZeroU16::new(max_unicast_solicit - 2))
            }
            other => unreachable!("test only covers DELAY and PROBE, got {:?}", other),
        };
        core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
            &mut bindings_ctx,
            [(I::LOOKUP_ADDR1, expected_initial_event, time.get())],
        );
        assert_eq!(
            bindings_ctx.trigger_timers_for(time.into(), &mut core_ctx,),
            [NudTimerId::neighbor()]
        );
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Probe(Probe { link_address: LINK_ADDR1, transmit_counter }),
            (initial_state != InitialState::Probe).then_some(ExpectedEvent::Changed),
        );
        core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
            &mut bindings_ctx,
            [(
                I::LOOKUP_ADDR1,
                NudEvent::RetransmitUnicastProbe,
                core_ctx.inner.state.retrans_timer.get(),
            )],
        );
        assert_neighbor_probe_sent(&mut core_ctx, Some(LINK_ADDR1));
    }

    #[ip_test(I)]
    fn unreachable_probes_with_exponential_backoff_while_packets_sent<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        init_unreachable_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1);

        let retrans_timer = core_ctx.inner.retransmit_timeout().get();
        let timer_id = NudTimerId::neighbor();

        // No multicast probes should be transmitted even after the retransmit timeout.
        assert_eq!(bindings_ctx.trigger_timers_for(retrans_timer, &mut core_ctx,), []);
        assert_eq!(core_ctx.inner.take_frames(), []);

        // Send a packet and ensure that we also transmit a multicast probe.
        const BODY: u8 = 0x33;
        assert_eq!(
            NudHandler::send_ip_packet_to_neighbor(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeLinkDeviceId,
                I::LOOKUP_ADDR1,
                Buf::new([BODY], ..),
                FakeTxMetadata::default(),
            ),
            Ok(())
        );
        assert_eq!(
            core_ctx.inner.take_frames(),
            [
                (FakeNudMessageMeta::IpFrame { dst_link_address: LINK_ADDR1 }, vec![BODY]),
                (
                    FakeNudMessageMeta::NeighborSolicitation {
                        lookup_addr: I::LOOKUP_ADDR1,
                        remote_link_addr: /* multicast */ None,
                    },
                    Vec::new()
                )
            ]
        );

        let next_backoff_timer = |core_ctx: &mut FakeCoreCtxImpl<I>, probes_sent| {
            UnreachableMode::Backoff {
                probes_sent: NonZeroU32::new(probes_sent).unwrap(),
                packet_sent: /* unused */ false,
            }
            .next_backoff_retransmit_timeout::<I, _>(&mut core_ctx.inner.state)
            .get()
        };

        const ITERATIONS: u8 = 2;
        for i in 1..ITERATIONS {
            let probes_sent = u32::from(i);

            // Send another packet before the retransmit timer expires: only the packet
            // should be sent (not a probe), and the `packet_sent` flag should be set.
            assert_eq!(
                NudHandler::send_ip_packet_to_neighbor(
                    &mut core_ctx,
                    &mut bindings_ctx,
                    &FakeLinkDeviceId,
                    I::LOOKUP_ADDR1,
                    Buf::new([BODY + i], ..),
                    FakeTxMetadata::default(),
                ),
                Ok(())
            );
            assert_eq!(
                core_ctx.inner.take_frames(),
                [(FakeNudMessageMeta::IpFrame { dst_link_address: LINK_ADDR1 }, vec![BODY + i])]
            );

            // Fast forward until the current retransmit timer should fire, taking
            // exponential backoff into account. Another multicast probe should be
            // transmitted and a new timer should be scheduled (backing off further) because
            // a packet was recently sent.
            assert_eq!(
                bindings_ctx.trigger_timers_for(
                    next_backoff_timer(&mut core_ctx, probes_sent),
                    &mut core_ctx,
                ),
                [timer_id]
            );
            assert_neighbor_probe_sent(&mut core_ctx, /* multicast */ None);
            bindings_ctx.timers.assert_timers_installed([(
                timer_id,
                bindings_ctx.now() + next_backoff_timer(&mut core_ctx, probes_sent + 1),
            )]);
        }

        // If no more packets are sent, no multicast probes should be transmitted even
        // after the next backoff timer expires.
        let current_timer = next_backoff_timer(&mut core_ctx, u32::from(ITERATIONS));
        assert_eq!(bindings_ctx.trigger_timers_for(current_timer, &mut core_ctx,), [timer_id]);
        assert_eq!(core_ctx.inner.take_frames(), []);
        bindings_ctx.timers.assert_no_timers_installed();

        // Finally, if another packet is sent, we resume transmitting multicast probes
        // and "reset" the exponential backoff.
        assert_eq!(
            NudHandler::send_ip_packet_to_neighbor(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeLinkDeviceId,
                I::LOOKUP_ADDR1,
                Buf::new([BODY], ..),
                FakeTxMetadata::default(),
            ),
            Ok(())
        );
        assert_eq!(
            core_ctx.inner.take_frames(),
            [
                (FakeNudMessageMeta::IpFrame { dst_link_address: LINK_ADDR1 }, vec![BODY]),
                (
                    FakeNudMessageMeta::NeighborSolicitation {
                        lookup_addr: I::LOOKUP_ADDR1,
                        remote_link_addr: /* multicast */ None,
                    },
                    Vec::new()
                )
            ]
        );
        bindings_ctx.timers.assert_timers_installed([(
            timer_id,
            bindings_ctx.now() + next_backoff_timer(&mut core_ctx, 1),
        )]);
    }

    #[ip_test(I)]
    #[test_case(true; "solicited confirmation")]
    #[test_case(false; "unsolicited confirmation")]
    fn confirmation_should_not_create_entry<I: TestIpExt>(solicited_flag: bool) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        let link_address = Some(LINK_ADDR1);
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address,
                flags: ConfirmationFlags { solicited_flag, override_flag: false },
            },
        );
        assert_eq!(core_ctx.nud.state.neighbors, HashMap::new());
    }

    #[ip_test(I)]
    #[test_case(true; "set_with_dynamic")]
    #[test_case(false; "set_with_static")]
    fn pending_frames<I: TestIpExt>(dynamic: bool) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        assert_eq!(core_ctx.inner.take_frames(), []);

        // Send up to the maximum number of pending frames to some neighbor
        // which requires resolution. This should cause all frames to be queued
        // pending resolution completion.
        const MAX_PENDING_FRAMES_U8: u8 = MAX_PENDING_FRAMES as u8;
        let expected_pending_frames = (0..MAX_PENDING_FRAMES_U8)
            .map(|i| (Buf::new(vec![i], ..), FakeTxMetadata::default()))
            .collect::<VecDeque<_>>();

        for (body, meta) in expected_pending_frames.iter() {
            assert_eq!(
                NudHandler::send_ip_packet_to_neighbor(
                    &mut core_ctx,
                    &mut bindings_ctx,
                    &FakeLinkDeviceId,
                    I::LOOKUP_ADDR1,
                    body.clone(),
                    meta.clone(),
                ),
                Ok(())
            );
        }
        let max_multicast_solicit = core_ctx.inner.max_multicast_solicit().get();
        // Should have only sent out a single neighbor probe message.
        assert_neighbor_probe_sent(&mut core_ctx, None);
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Incomplete(Incomplete {
                transmit_counter: NonZeroU16::new(max_multicast_solicit - 1),
                pending_frames: expected_pending_frames.clone(),
                notifiers: Vec::new(),
                _marker: PhantomData,
            }),
            Some(ExpectedEvent::Added),
        );

        // The next frame should be dropped.
        assert_eq!(
            NudHandler::send_ip_packet_to_neighbor(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeLinkDeviceId,
                I::LOOKUP_ADDR1,
                Buf::new([123], ..),
                FakeTxMetadata::default(),
            ),
            Ok(())
        );
        assert_eq!(core_ctx.inner.take_frames(), []);
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Incomplete(Incomplete {
                transmit_counter: NonZeroU16::new(max_multicast_solicit - 1),
                pending_frames: expected_pending_frames.clone(),
                notifiers: Vec::new(),
                _marker: PhantomData,
            }),
            None,
        );

        // Completing resolution should result in all queued packets being sent.
        if dynamic {
            NudHandler::handle_neighbor_update(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeLinkDeviceId,
                I::LOOKUP_ADDR1,
                DynamicNeighborUpdateSource::Confirmation {
                    link_address: Some(LINK_ADDR1),
                    flags: ConfirmationFlags { solicited_flag: true, override_flag: false },
                },
            );
            core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
                &mut bindings_ctx,
                [(
                    I::LOOKUP_ADDR1,
                    NudEvent::ReachableTime,
                    core_ctx.inner.base_reachable_time().get(),
                )],
            );
            let last_confirmed_at = bindings_ctx.now();
            assert_neighbor_state(
                &core_ctx,
                &mut bindings_ctx,
                DynamicNeighborState::Reachable(Reachable {
                    link_address: LINK_ADDR1,
                    last_confirmed_at,
                }),
                Some(ExpectedEvent::Changed),
            );
        } else {
            init_static_neighbor(
                &mut core_ctx,
                &mut bindings_ctx,
                LINK_ADDR1,
                ExpectedEvent::Changed,
            );
            bindings_ctx.timers.assert_no_timers_installed();
        }
        assert_eq!(
            core_ctx.inner.take_frames(),
            expected_pending_frames
                .into_iter()
                .map(|(p, FakeTxMetadata)| (
                    FakeNudMessageMeta::IpFrame { dst_link_address: LINK_ADDR1 },
                    p.as_ref().to_vec()
                ))
                .collect::<Vec<_>>()
        );
    }

    #[ip_test(I)]
    fn static_neighbor<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        init_static_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1, ExpectedEvent::Added);
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.inner.take_frames(), []);
        check_lookup_has(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR1, LINK_ADDR1);

        // Dynamic entries should not overwrite static entries.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Probe { link_address: LINK_ADDR2 },
        );
        check_lookup_has(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR1, LINK_ADDR1);

        delete_neighbor(&mut core_ctx, &mut bindings_ctx);

        let neighbors = &core_ctx.nud.state.neighbors;
        assert!(neighbors.is_empty(), "neighbor table should be empty: {neighbors:?}");
    }

    #[ip_test(I)]
    fn dynamic_neighbor<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        init_stale_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1);
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.inner.take_frames(), []);
        check_lookup_has(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR1, LINK_ADDR1);

        // Dynamic entries may be overwritten by new dynamic entries.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Probe { link_address: LINK_ADDR2 },
        );
        check_lookup_has(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR1, LINK_ADDR2);
        assert_eq!(core_ctx.inner.take_frames(), []);
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Stale(Stale { link_address: LINK_ADDR2 }),
            Some(ExpectedEvent::Changed),
        );

        // A static entry may overwrite a dynamic entry.
        init_static_neighbor_with_ip(
            &mut core_ctx,
            &mut bindings_ctx,
            I::LOOKUP_ADDR1,
            LINK_ADDR3,
            ExpectedEvent::Changed,
        );
        check_lookup_has(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR1, LINK_ADDR3);
        assert_eq!(core_ctx.inner.take_frames(), []);
    }

    #[ip_test(I)]
    fn send_solicitation_on_lookup<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.inner.take_frames(), []);

        let mut pending_frames = VecDeque::new();

        queue_ip_packet_to_unresolved_neighbor(
            &mut core_ctx,
            &mut bindings_ctx,
            I::LOOKUP_ADDR1,
            &mut pending_frames,
            1,
            true, /* expect_event */
        );
        assert_neighbor_probe_sent(&mut core_ctx, None);

        queue_ip_packet_to_unresolved_neighbor(
            &mut core_ctx,
            &mut bindings_ctx,
            I::LOOKUP_ADDR1,
            &mut pending_frames,
            2,
            false, /* expect_event */
        );
        assert_eq!(core_ctx.inner.take_frames(), []);

        // Complete link resolution.
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: Some(LINK_ADDR1),
                flags: ConfirmationFlags { solicited_flag: true, override_flag: false },
            },
        );
        check_lookup_has(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR1, LINK_ADDR1);

        let now = bindings_ctx.now();
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR1,
                last_confirmed_at: now,
            }),
            Some(ExpectedEvent::Changed),
        );
        assert_eq!(
            core_ctx.inner.take_frames(),
            pending_frames
                .into_iter()
                .map(|f| (
                    FakeNudMessageMeta::IpFrame { dst_link_address: LINK_ADDR1 },
                    f.as_ref().to_vec(),
                ))
                .collect::<Vec<_>>()
        );
    }

    #[ip_test(I)]
    fn solicitation_failure_in_incomplete<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.inner.take_frames(), []);

        let pending_frames = init_incomplete_neighbor(&mut core_ctx, &mut bindings_ctx, false);

        let timer_id = NudTimerId::neighbor();

        let retrans_timer = core_ctx.inner.retransmit_timeout().get();
        let max_multicast_solicit = core_ctx.inner.max_multicast_solicit().get();

        for i in 1..=max_multicast_solicit {
            assert_neighbor_state(
                &core_ctx,
                &mut bindings_ctx,
                DynamicNeighborState::Incomplete(Incomplete {
                    transmit_counter: NonZeroU16::new(max_multicast_solicit - i),
                    pending_frames: pending_frames
                        .iter()
                        .cloned()
                        .map(|b| (b, FakeTxMetadata::default()))
                        .collect(),
                    notifiers: Vec::new(),
                    _marker: PhantomData,
                }),
                None,
            );

            bindings_ctx
                .timers
                .assert_timers_installed([(timer_id, bindings_ctx.now() + ONE_SECOND.get())]);
            assert_neighbor_probe_sent(&mut core_ctx, /* multicast */ None);

            assert_eq!(bindings_ctx.trigger_timers_for(retrans_timer, &mut core_ctx,), [timer_id]);
        }

        // The neighbor entry should have been removed.
        assert_neighbor_removed_with_ip(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR1);
        bindings_ctx.timers.assert_no_timers_installed();

        // The ICMP destination unreachable error sent as a result of solicitation failure
        // will be dropped because the packets pending address resolution in this test
        // is not a valid IP packet.
        assert_eq!(core_ctx.inner.take_frames(), []);
        assert_eq!(core_ctx.counters().as_ref().icmp_dest_unreachable_dropped.get(), 1);
    }

    #[ip_test(I)]
    fn solicitation_failure_in_probe<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.inner.take_frames(), []);

        init_probe_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1, false);

        let timer_id = NudTimerId::neighbor();
        let retrans_timer = core_ctx.inner.retransmit_timeout().get();
        let max_unicast_solicit = core_ctx.inner.max_unicast_solicit().get();
        for i in 1..=max_unicast_solicit {
            assert_neighbor_state(
                &core_ctx,
                &mut bindings_ctx,
                DynamicNeighborState::Probe(Probe {
                    transmit_counter: NonZeroU16::new(max_unicast_solicit - i),
                    link_address: LINK_ADDR1,
                }),
                None,
            );

            bindings_ctx
                .timers
                .assert_timers_installed([(timer_id, bindings_ctx.now() + ONE_SECOND.get())]);
            assert_neighbor_probe_sent(&mut core_ctx, Some(LINK_ADDR1));

            assert_eq!(bindings_ctx.trigger_timers_for(retrans_timer, &mut core_ctx,), [timer_id]);
        }

        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Unreachable(Unreachable {
                link_address: LINK_ADDR1,
                mode: UnreachableMode::WaitingForPacketSend,
            }),
            Some(ExpectedEvent::Changed),
        );
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.inner.take_frames(), []);
    }

    #[ip_test(I)]
    fn flush_entries<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.inner.take_frames(), []);

        init_static_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1, ExpectedEvent::Added);
        init_stale_neighbor_with_ip(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR2, LINK_ADDR2);
        let pending_frames = init_incomplete_neighbor_with_ip(
            &mut core_ctx,
            &mut bindings_ctx,
            I::LOOKUP_ADDR3,
            true,
        );
        let pending_frames =
            pending_frames.into_iter().map(|b| (b, FakeTxMetadata::default())).collect();

        let max_multicast_solicit = core_ctx.inner.max_multicast_solicit().get();
        assert_eq!(
            core_ctx.nud.state.neighbors,
            HashMap::from([
                (I::LOOKUP_ADDR1, NeighborState::Static(LINK_ADDR1)),
                (
                    I::LOOKUP_ADDR2,
                    NeighborState::Dynamic(DynamicNeighborState::Stale(Stale {
                        link_address: LINK_ADDR2,
                    })),
                ),
                (
                    I::LOOKUP_ADDR3,
                    NeighborState::Dynamic(DynamicNeighborState::Incomplete(Incomplete {
                        transmit_counter: NonZeroU16::new(max_multicast_solicit - 1),
                        pending_frames,
                        notifiers: Vec::new(),
                        _marker: PhantomData,
                    })),
                ),
            ]),
        );
        core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
            &mut bindings_ctx,
            [(I::LOOKUP_ADDR3, NudEvent::RetransmitMulticastProbe, ONE_SECOND.get())],
        );

        // Flushing the table should clear all entries (dynamic and static) and timers.
        NudHandler::flush(&mut core_ctx, &mut bindings_ctx, &FakeLinkDeviceId);
        let neighbors = &core_ctx.nud.state.neighbors;
        assert!(neighbors.is_empty(), "neighbor table should be empty: {:?}", neighbors);
        assert_eq!(
            bindings_ctx.take_events().into_iter().collect::<HashSet<_>>(),
            [I::LOOKUP_ADDR1, I::LOOKUP_ADDR2, I::LOOKUP_ADDR3]
                .into_iter()
                .map(|addr| { Event::removed(&FakeLinkDeviceId, addr, bindings_ctx.now()) })
                .collect(),
        );
        bindings_ctx.timers.assert_no_timers_installed();
    }

    #[ip_test(I)]
    fn delete_dynamic_entry<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.inner.take_frames(), []);

        init_reachable_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1);
        check_lookup_has(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR1, LINK_ADDR1);

        delete_neighbor(&mut core_ctx, &mut bindings_ctx);

        // Entry should be removed and timer cancelled.
        let neighbors = &core_ctx.nud.state.neighbors;
        assert!(neighbors.is_empty(), "neighbor table should be empty: {neighbors:?}");
        bindings_ctx.timers.assert_no_timers_installed();
    }

    #[ip_test(I)]
    #[test_case(InitialState::Reachable; "reachable neighbor")]
    #[test_case(InitialState::Stale; "stale neighbor")]
    #[test_case(InitialState::Delay; "delay neighbor")]
    #[test_case(InitialState::Probe; "probe neighbor")]
    #[test_case(InitialState::Unreachable; "unreachable neighbor")]
    fn resolve_cached_linked_addr<I: TestIpExt>(initial_state: InitialState) {
        let mut ctx = new_context::<I>();
        ctx.bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(ctx.core_ctx.inner.take_frames(), []);

        let _ = init_neighbor_in_state(&mut ctx.core_ctx, &mut ctx.bindings_ctx, initial_state);

        let link_addr = assert_matches!(
            NeighborApi::new(ctx.as_mut()).resolve_link_addr(
                &FakeLinkDeviceId,
                &I::LOOKUP_ADDR1,
            ),
            LinkResolutionResult::Resolved(addr) => addr
        );
        assert_eq!(link_addr, LINK_ADDR1);
        if initial_state == InitialState::Stale {
            assert_eq!(
                ctx.bindings_ctx.take_events(),
                [Event::changed(
                    &FakeLinkDeviceId,
                    EventState::Dynamic(EventDynamicState::Delay(LINK_ADDR1)),
                    I::LOOKUP_ADDR1,
                    ctx.bindings_ctx.now(),
                )],
            );
        }
    }

    enum ResolutionSuccess {
        Confirmation,
        StaticEntryAdded,
    }

    #[ip_test(I)]
    #[test_case(ResolutionSuccess::Confirmation; "incomplete entry timed out")]
    #[test_case(ResolutionSuccess::StaticEntryAdded; "incomplete entry removed from table")]
    fn dynamic_neighbor_resolution_success<I: TestIpExt>(reason: ResolutionSuccess) {
        let mut ctx = new_context::<I>();

        let observers = (0..10)
            .map(|_| {
                let observer = assert_matches!(
                    NeighborApi::new(ctx.as_mut()).resolve_link_addr(
                        &FakeLinkDeviceId,
                        &I::LOOKUP_ADDR1,
                    ),
                    LinkResolutionResult::Pending(observer) => observer
                );
                assert_eq!(*observer.lock(), None);
                observer
            })
            .collect::<Vec<_>>();
        let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
        let max_multicast_solicit = core_ctx.inner.max_multicast_solicit().get();

        // We should have initialized an incomplete neighbor and sent a neighbor probe
        // to attempt resolution.
        assert_neighbor_state(
            core_ctx,
            bindings_ctx,
            DynamicNeighborState::Incomplete(Incomplete {
                transmit_counter: NonZeroU16::new(max_multicast_solicit - 1),
                pending_frames: VecDeque::new(),
                // NB: notifiers is not checked for equality.
                notifiers: Vec::new(),
                _marker: PhantomData,
            }),
            Some(ExpectedEvent::Added),
        );
        assert_neighbor_probe_sent(core_ctx, /* multicast */ None);

        match reason {
            ResolutionSuccess::Confirmation => {
                // Complete neighbor resolution with an incoming neighbor confirmation.
                NudHandler::handle_neighbor_update(
                    core_ctx,
                    bindings_ctx,
                    &FakeLinkDeviceId,
                    I::LOOKUP_ADDR1,
                    DynamicNeighborUpdateSource::Confirmation {
                        link_address: Some(LINK_ADDR1),
                        flags: ConfirmationFlags { solicited_flag: true, override_flag: false },
                    },
                );
                let now = bindings_ctx.now();
                assert_neighbor_state(
                    core_ctx,
                    bindings_ctx,
                    DynamicNeighborState::Reachable(Reachable {
                        link_address: LINK_ADDR1,
                        last_confirmed_at: now,
                    }),
                    Some(ExpectedEvent::Changed),
                );
            }
            ResolutionSuccess::StaticEntryAdded => {
                init_static_neighbor(core_ctx, bindings_ctx, LINK_ADDR1, ExpectedEvent::Changed);
                assert_eq!(
                    core_ctx.nud.state.neighbors.get(&I::LOOKUP_ADDR1),
                    Some(&NeighborState::Static(LINK_ADDR1))
                );
            }
        }

        // Each observer should have been notified of successful link resolution.
        for observer in observers {
            assert_eq!(*observer.lock(), Some(Ok(LINK_ADDR1)));
        }
    }

    enum ResolutionFailure {
        Timeout,
        Removed,
    }

    #[ip_test(I)]
    #[test_case(ResolutionFailure::Timeout; "incomplete entry timed out")]
    #[test_case(ResolutionFailure::Removed; "incomplete entry removed from table")]
    fn dynamic_neighbor_resolution_failure<I: TestIpExt>(reason: ResolutionFailure) {
        let mut ctx = new_context::<I>();

        let observers = (0..10)
            .map(|_| {
                let observer = assert_matches!(
                    NeighborApi::new(ctx.as_mut()).resolve_link_addr(
                        &FakeLinkDeviceId,
                        &I::LOOKUP_ADDR1,
                    ),
                    LinkResolutionResult::Pending(observer) => observer
                );
                assert_eq!(*observer.lock(), None);
                observer
            })
            .collect::<Vec<_>>();

        let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
        let max_multicast_solicit = core_ctx.inner.max_multicast_solicit().get();

        // We should have initialized an incomplete neighbor and sent a neighbor probe
        // to attempt resolution.
        assert_neighbor_state(
            core_ctx,
            bindings_ctx,
            DynamicNeighborState::Incomplete(Incomplete {
                transmit_counter: NonZeroU16::new(max_multicast_solicit - 1),
                pending_frames: VecDeque::new(),
                // NB: notifiers is not checked for equality.
                notifiers: Vec::new(),
                _marker: PhantomData,
            }),
            Some(ExpectedEvent::Added),
        );
        assert_neighbor_probe_sent(core_ctx, /* multicast */ None);

        match reason {
            ResolutionFailure::Timeout => {
                // Wait until neighbor resolution exceeds its maximum probe retransmits and
                // times out.
                for _ in 1..=max_multicast_solicit {
                    let retrans_timer = core_ctx.inner.retransmit_timeout().get();
                    assert_eq!(
                        bindings_ctx.trigger_timers_for(retrans_timer, core_ctx),
                        [NudTimerId::neighbor()]
                    );
                }
            }
            ResolutionFailure::Removed => {
                // Flush the neighbor table so the entry is removed.
                NudHandler::flush(core_ctx, bindings_ctx, &FakeLinkDeviceId);
            }
        }

        assert_neighbor_removed_with_ip(core_ctx, bindings_ctx, I::LOOKUP_ADDR1);
        // Each observer should have been notified of link resolution failure.
        for observer in observers {
            assert_eq!(*observer.lock(), Some(Err(AddressResolutionFailed)));
        }
    }

    #[ip_test(I)]
    #[test_case(InitialState::Incomplete, false; "incomplete neighbor")]
    #[test_case(InitialState::Reachable, true; "reachable neighbor")]
    #[test_case(InitialState::Stale, true; "stale neighbor")]
    #[test_case(InitialState::Delay, true; "delay neighbor")]
    #[test_case(InitialState::Probe, true; "probe neighbor")]
    #[test_case(InitialState::Unreachable, true; "unreachable neighbor")]
    fn upper_layer_confirmation<I: TestIpExt>(
        initial_state: InitialState,
        should_transition_to_reachable: bool,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let base_reachable_time = core_ctx.inner.base_reachable_time().get();

        let initial = init_neighbor_in_state(&mut core_ctx, &mut bindings_ctx, initial_state);

        confirm_reachable(&mut core_ctx, &mut bindings_ctx, &FakeLinkDeviceId, I::LOOKUP_ADDR1);

        if !should_transition_to_reachable {
            assert_neighbor_state(&core_ctx, &mut bindings_ctx, initial, None);
            return;
        }

        // Neighbor should have transitioned to REACHABLE and scheduled a timer.
        let now = bindings_ctx.now();
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR1,
                last_confirmed_at: now,
            }),
            (initial_state != InitialState::Reachable).then_some(ExpectedEvent::Changed),
        );
        core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
            &mut bindings_ctx,
            [(I::LOOKUP_ADDR1, NudEvent::ReachableTime, base_reachable_time)],
        );

        // Advance the clock by less than ReachableTime and confirm reachability again.
        // The existing timer should not have been rescheduled; only the entry's
        // `last_confirmed_at` timestamp should have been updated.
        bindings_ctx.timers.instant.sleep(base_reachable_time / 2);
        confirm_reachable(&mut core_ctx, &mut bindings_ctx, &FakeLinkDeviceId, I::LOOKUP_ADDR1);
        let now = bindings_ctx.now();
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR1,
                last_confirmed_at: now,
            }),
            None,
        );
        core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
            &mut bindings_ctx,
            [(I::LOOKUP_ADDR1, NudEvent::ReachableTime, base_reachable_time / 2)],
        );

        // When the original timer eventually does expire, a new timer should be
        // scheduled based on when the entry was last confirmed.
        assert_eq!(
            bindings_ctx.trigger_timers_for(base_reachable_time / 2, &mut core_ctx,),
            [NudTimerId::neighbor()]
        );
        let now = bindings_ctx.now();
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR1,
                last_confirmed_at: now - base_reachable_time / 2,
            }),
            None,
        );

        core_ctx.nud.state.timer_heap.neighbor.assert_timers_after(
            &mut bindings_ctx,
            [(I::LOOKUP_ADDR1, NudEvent::ReachableTime, base_reachable_time / 2)],
        );

        // When *that* timer fires, if the entry has not been confirmed since it was
        // scheduled, it should move into STALE.
        assert_eq!(
            bindings_ctx.trigger_timers_for(base_reachable_time / 2, &mut core_ctx,),
            [NudTimerId::neighbor()]
        );
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Stale(Stale { link_address: LINK_ADDR1 }),
            Some(ExpectedEvent::Changed),
        );
        bindings_ctx.timers.assert_no_timers_installed();
    }

    fn generate_ip_addr<I: Ip>(i: usize) -> SpecifiedAddr<I::Addr> {
        I::map_ip_out(
            i,
            |i| {
                let start = u32::from_be_bytes(net_ip_v4!("192.168.0.1").ipv4_bytes());
                let bytes = (start + u32::try_from(i).unwrap()).to_be_bytes();
                SpecifiedAddr::new(Ipv4Addr::new(bytes)).unwrap()
            },
            |i| {
                let start = u128::from_be_bytes(net_ip_v6!("fe80::1").ipv6_bytes());
                let bytes = (start + u128::try_from(i).unwrap()).to_be_bytes();
                SpecifiedAddr::new(Ipv6Addr::from_bytes(bytes)).unwrap()
            },
        )
    }

    #[ip_test(I)]
    fn garbage_collection_retains_static_entries<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Add `GC_THRESHOLD` STALE dynamic neighbors and `GC_THRESHOLD` static
        // neighbors to the neighbor table, interleaved to avoid accidental
        // behavior re: insertion order.
        for i in 0..GC_THRESHOLD * 2 {
            if i % 2 == 0 {
                init_stale_neighbor_with_ip(
                    &mut core_ctx,
                    &mut bindings_ctx,
                    generate_ip_addr::<I>(i),
                    LINK_ADDR1,
                );
            } else {
                init_static_neighbor_with_ip(
                    &mut core_ctx,
                    &mut bindings_ctx,
                    generate_ip_addr::<I>(i),
                    LINK_ADDR1,
                    ExpectedEvent::Added,
                );
            }
        }
        assert_eq!(core_ctx.nud.state.neighbors.len(), GC_THRESHOLD * 2);

        // Perform GC, and ensure that only the dynamic entries are discarded.
        assert_eq!(core_ctx.nud.state.gc_state.is_dirty, true);
        collect_garbage(&mut core_ctx, &mut bindings_ctx, FakeLinkDeviceId);
        assert_eq!(core_ctx.nud.state.gc_state.is_dirty, false);
        for event in bindings_ctx.take_events() {
            assert_matches!(event, Event {
                device,
                addr: _,
                kind,
                at,
            } => {
                assert_eq!(kind, EventKind::Removed);
                assert_eq!(device, FakeLinkDeviceId);
                assert_eq!(at, bindings_ctx.now());
            });
        }
        assert_eq!(core_ctx.nud.state.neighbors.len(), GC_THRESHOLD);
        for (_, neighbor) in core_ctx.nud.state.neighbors {
            assert_matches!(neighbor, NeighborState::Static(_));
        }
    }

    #[ip_test(I)]
    fn garbage_collection_retains_in_use_entries<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Add enough static entries that the NUD table is near maximum capacity.
        for i in 0..GC_THRESHOLD - 1 {
            init_static_neighbor_with_ip(
                &mut core_ctx,
                &mut bindings_ctx,
                generate_ip_addr::<I>(i),
                LINK_ADDR1,
                ExpectedEvent::Added,
            );
        }

        // Add a STALE entry...
        let stale_entry = generate_ip_addr::<I>(GC_THRESHOLD - 1);
        init_stale_neighbor_with_ip(&mut core_ctx, &mut bindings_ctx, stale_entry, LINK_ADDR1);
        // ...and a REACHABLE entry.
        let reachable_entry = generate_ip_addr::<I>(GC_THRESHOLD);
        init_reachable_neighbor_with_ip(
            &mut core_ctx,
            &mut bindings_ctx,
            reachable_entry,
            LINK_ADDR1,
        );

        // Perform GC, and ensure that the REACHABLE entry was retained.
        assert_eq!(core_ctx.nud.state.gc_state.is_dirty, true);
        collect_garbage(&mut core_ctx, &mut bindings_ctx, FakeLinkDeviceId);
        assert_eq!(core_ctx.nud.state.gc_state.is_dirty, false);
        super::testutil::assert_dynamic_neighbor_state(
            &mut core_ctx,
            FakeLinkDeviceId,
            reachable_entry,
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR1,
                last_confirmed_at: bindings_ctx.now(),
            }),
        );
        assert_neighbor_removed_with_ip(&mut core_ctx, &mut bindings_ctx, stale_entry);
    }

    #[ip_test(I)]
    fn is_still_dirty_after_garbage_collection<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Add enough STALE entries to trigger garbage collection.
        for i in 0..GC_THRESHOLD + 1 {
            init_stale_neighbor_with_ip(
                &mut core_ctx,
                &mut bindings_ctx,
                generate_ip_addr::<I>(i),
                LINK_ADDR1,
            );
        }

        // Perform GC, and ensure that the `is_dirty` is still set (because not
        // all STALE entries were removed).
        assert_eq!(core_ctx.nud.state.gc_state.is_dirty, true);
        collect_garbage(&mut core_ctx, &mut bindings_ctx, FakeLinkDeviceId);
        assert_eq!(core_ctx.nud.state.gc_state.is_dirty, true);
        assert_eq!(core_ctx.nud.state.neighbors.len(), GC_THRESHOLD);
        let events = bindings_ctx.take_events();
        let removed_event = assert_matches!(&events[..], [event] => event);
        assert_eq!(removed_event.kind, EventKind::Removed);
    }

    #[ip_test(I)]
    fn garbage_collection_triggered_on_new_stale_entry<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        // Pretend we just ran GC so the next pass will be scheduled after a delay.
        core_ctx.nud.state.gc_state.last_gc = Some(bindings_ctx.now());

        // Fill the neighbor table to maximum capacity with static entries.
        for i in 0..GC_THRESHOLD {
            init_static_neighbor_with_ip(
                &mut core_ctx,
                &mut bindings_ctx,
                generate_ip_addr::<I>(i),
                LINK_ADDR1,
                ExpectedEvent::Added,
            );
        }

        // Add a STALE neighbor entry to the table, which should trigger a GC run
        // because it pushes the size of the table over the max.
        init_stale_neighbor_with_ip(
            &mut core_ctx,
            &mut bindings_ctx,
            generate_ip_addr::<I>(GC_THRESHOLD + 1),
            LINK_ADDR1,
        );
        let expected_gc_time = bindings_ctx.now() + MIN_GARBAGE_COLLECTION_INTERVAL.get();
        bindings_ctx
            .timers
            .assert_some_timers_installed([(NudTimerId::garbage_collection(), expected_gc_time)]);

        // Advance the clock by less than the GC interval and add another STALE entry to
        // trigger GC again. The existing GC timer should not have been rescheduled
        // given a GC pass is already pending.
        bindings_ctx.timers.instant.sleep(ONE_SECOND.get());
        init_stale_neighbor_with_ip(
            &mut core_ctx,
            &mut bindings_ctx,
            generate_ip_addr::<I>(GC_THRESHOLD + 2),
            LINK_ADDR1,
        );
        bindings_ctx
            .timers
            .assert_some_timers_installed([(NudTimerId::garbage_collection(), expected_gc_time)]);
    }

    #[ip_test(I)]
    fn garbage_collection_triggered_on_transition_to_unreachable<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        // Pretend we just ran GC so the next pass will be scheduled after a delay.
        core_ctx.nud.state.gc_state.last_gc = Some(bindings_ctx.now());

        // Fill the neighbor table to maximum capacity.
        for i in 0..GC_THRESHOLD {
            init_static_neighbor_with_ip(
                &mut core_ctx,
                &mut bindings_ctx,
                generate_ip_addr::<I>(i),
                LINK_ADDR1,
                ExpectedEvent::Added,
            );
        }
        assert_eq!(core_ctx.nud.state.neighbors.len(), GC_THRESHOLD);

        // Add a dynamic neighbor entry to the table and transition it to the
        // UNREACHABLE state. This should trigger a GC run.
        init_unreachable_neighbor_with_ip(
            &mut core_ctx,
            &mut bindings_ctx,
            generate_ip_addr::<I>(GC_THRESHOLD),
            LINK_ADDR1,
        );
        let expected_gc_time =
            core_ctx.nud.state.gc_state.last_gc.unwrap() + MIN_GARBAGE_COLLECTION_INTERVAL.get();
        bindings_ctx
            .timers
            .assert_some_timers_installed([(NudTimerId::garbage_collection(), expected_gc_time)]);

        // Add a new entry and transition it to UNREACHABLE. The existing GC timer
        // should not have been rescheduled given a GC pass is already pending.
        init_unreachable_neighbor_with_ip(
            &mut core_ctx,
            &mut bindings_ctx,
            generate_ip_addr::<I>(GC_THRESHOLD + 1),
            LINK_ADDR1,
        );
        bindings_ctx
            .timers
            .assert_some_timers_installed([(NudTimerId::garbage_collection(), expected_gc_time)]);
    }

    #[ip_test(I)]
    fn garbage_collection_not_triggered_on_new_incomplete_entry<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Fill the neighbor table to maximum capacity with static entries.
        for i in 0..GC_THRESHOLD {
            init_static_neighbor_with_ip(
                &mut core_ctx,
                &mut bindings_ctx,
                generate_ip_addr::<I>(i),
                LINK_ADDR1,
                ExpectedEvent::Added,
            );
        }
        assert_eq!(core_ctx.nud.state.neighbors.len(), GC_THRESHOLD);

        let _: VecDeque<Buf<Vec<u8>>> = init_incomplete_neighbor_with_ip(
            &mut core_ctx,
            &mut bindings_ctx,
            generate_ip_addr::<I>(GC_THRESHOLD),
            true,
        );
        assert_eq!(
            bindings_ctx.timers.scheduled_instant(&mut core_ctx.nud.state.timer_heap.gc),
            None
        );
    }

    #[ip_test(I)]
    fn confirmation_processed_even_if_no_target_link_layer_addr<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Initialize a neighbor in STALE.
        init_stale_neighbor_with_ip(&mut core_ctx, &mut bindings_ctx, I::LOOKUP_ADDR1, LINK_ADDR1);

        // Receive a neighbor confirmation that omits the target link-layer address
        // option. Because we have a cached link-layer address, we should still process
        // the confirmation (updating the neighbor to REACHABLE).
        NudHandler::handle_neighbor_update(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeLinkDeviceId,
            I::LOOKUP_ADDR1,
            DynamicNeighborUpdateSource::Confirmation {
                link_address: None,
                flags: ConfirmationFlags { solicited_flag: true, override_flag: false },
            },
        );
        let now = bindings_ctx.now();
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Reachable(Reachable {
                link_address: LINK_ADDR1,
                last_confirmed_at: now,
            }),
            Some(ExpectedEvent::Changed),
        );
    }

    #[ip_test(I)]
    #[test_case(InitialState::Stale; "stale")]
    #[test_case(InitialState::Reachable; "reachable")]
    #[test_case(InitialState::Delay; "delay")]
    #[test_case(InitialState::Unreachable; "unreachable")]
    fn enter_probe_from_dynamic_state<I: TestIpExt>(initial: InitialState) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        let _ = init_neighbor_in_state(&mut core_ctx, &mut bindings_ctx, initial);

        let neighbor = core_ctx.nud.state.neighbors.get_mut(&I::LOOKUP_ADDR1).unwrap();
        let result = neighbor.enter_probe(
            &mut core_ctx.inner.state,
            &mut bindings_ctx,
            &mut core_ctx.nud.state.timer_heap,
            I::LOOKUP_ADDR1,
            &FakeLinkDeviceId,
        );

        let max_unicast_probes = core_ctx.inner.max_unicast_solicit().get();
        assert_matches!(result, Ok(Some(LINK_ADDR1)));
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Probe(Probe {
                link_address: LINK_ADDR1,
                transmit_counter: Some(NonZeroU16::new(max_unicast_probes - 1).unwrap()),
            }),
            Some(ExpectedEvent::Changed),
        );
    }

    #[ip_test(I)]
    fn enter_probe_from_static_state<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        init_static_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1, ExpectedEvent::Added);

        let neighbor = core_ctx.nud.state.neighbors.get_mut(&I::LOOKUP_ADDR1).unwrap();
        let result = neighbor.enter_probe(
            &mut core_ctx.inner.state,
            &mut bindings_ctx,
            &mut core_ctx.nud.state.timer_heap,
            I::LOOKUP_ADDR1,
            &FakeLinkDeviceId,
        );

        let max_unicast_probes = core_ctx.inner.max_unicast_solicit().get();
        assert_matches!(result, Ok(Some(LINK_ADDR1)));
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Probe(Probe {
                link_address: LINK_ADDR1,
                transmit_counter: Some(NonZeroU16::new(max_unicast_probes - 1).unwrap()),
            }),
            Some(ExpectedEvent::Changed),
        );
    }

    #[ip_test(I)]
    fn enter_probe_from_probe_state<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        init_probe_neighbor(&mut core_ctx, &mut bindings_ctx, LINK_ADDR1, false);

        let neighbor = core_ctx.nud.state.neighbors.get_mut(&I::LOOKUP_ADDR1).unwrap();
        let result = neighbor.enter_probe(
            &mut core_ctx.inner.state,
            &mut bindings_ctx,
            &mut core_ctx.nud.state.timer_heap,
            I::LOOKUP_ADDR1,
            &FakeLinkDeviceId,
        );

        let max_unicast_probes = core_ctx.inner.max_unicast_solicit().get();
        assert_matches!(result, Ok(None)); // No new probe should be transmitted.
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Probe(Probe {
                link_address: LINK_ADDR1,
                transmit_counter: Some(NonZeroU16::new(max_unicast_probes - 1).unwrap()),
            }),
            None, // No event should be generated.
        );
    }

    #[ip_test(I)]
    fn enter_probe_from_incomplete_state<I: TestIpExt>() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        let pending_frames = init_incomplete_neighbor(&mut core_ctx, &mut bindings_ctx, false);

        let neighbor = core_ctx.nud.state.neighbors.get_mut(&I::LOOKUP_ADDR1).unwrap();
        let result = neighbor.enter_probe(
            &mut core_ctx.inner.state,
            &mut bindings_ctx,
            &mut core_ctx.nud.state.timer_heap,
            I::LOOKUP_ADDR1,
            &FakeLinkDeviceId,
        );

        let max_multicast_solicit = core_ctx.inner.max_multicast_solicit().get();
        assert_matches!(result, Err(EnterProbeError::LinkAddressUnknown));
        assert_neighbor_state(
            &core_ctx,
            &mut bindings_ctx,
            DynamicNeighborState::Incomplete(Incomplete {
                transmit_counter: NonZeroU16::new(max_multicast_solicit - 1),
                pending_frames: pending_frames
                    .iter()
                    .cloned()
                    .map(|b| (b, FakeTxMetadata::default()))
                    .collect(),
                notifiers: Vec::new(),
                _marker: PhantomData,
            }),
            None, // No event should be generated.
        );
    }

    /// Various ways a neighbor entry could be inserted to exercise table full
    /// conditions.
    enum InsertMethod {
        ResolveLinkAddr,
        InsertStaticEntry,
        NeighborUpdate,
        SendIpPacket,
    }

    impl InsertMethod {
        fn insert<I: TestIpExt>(
            &self,
            context: &mut CtxPair<&mut FakeCoreCtxImpl<I>, &mut FakeBindingsCtxImpl<I>>,
            link_address: UnicastAddr<FakeLinkAddress>,
            ip: SpecifiedAddr<I::Addr>,
            expect_err: bool,
        ) {
            match self {
                Self::ResolveLinkAddr => {
                    let result =
                        NeighborApi::new(context).resolve_link_addr(&FakeLinkDeviceId, &ip);
                    let pending = assert_matches!(
                        result, LinkResolutionResult::Pending(pending) => pending
                    );
                    if expect_err {
                        assert_matches!(
                            pending.lock().as_ref(),
                            Some(Err(AddressResolutionFailed))
                        );
                    } else {
                        assert_matches!(pending.lock().as_ref(), None, "should not be notified");
                    }
                }
                Self::InsertStaticEntry => {
                    let result = NeighborApi::new(context).insert_static_entry(
                        &FakeLinkDeviceId,
                        *ip,
                        link_address,
                    );
                    if expect_err {
                        assert_eq!(result, Err(StaticNeighborInsertionError::TableFull))
                    } else {
                        assert_eq!(result, Ok(()))
                    }
                }
                Self::NeighborUpdate => {
                    let CtxPair { core_ctx, bindings_ctx } = context;
                    NudHandler::handle_neighbor_update(
                        *core_ctx,
                        *bindings_ctx,
                        &FakeLinkDeviceId,
                        ip,
                        DynamicNeighborUpdateSource::Probe { link_address },
                    );
                    // NB: ignore `expect_err` because errors aren't observable
                    // on `handle_neighbor_update`.
                }
                Self::SendIpPacket => {
                    let CtxPair { core_ctx, bindings_ctx } = context;
                    let packet = Buf::new([0; 10], ..);
                    let result = NudHandler::send_ip_packet_to_neighbor(
                        *core_ctx,
                        *bindings_ctx,
                        &FakeLinkDeviceId,
                        ip,
                        packet,
                        FakeTxMetadata::default(),
                    );
                    if expect_err {
                        assert_matches!(
                            result,
                            Err(ErrorAndSerializer {error, ..})
                            if error == SendFrameErrorReason::AddressResolutionFailed
                        );
                    } else {
                        assert_matches!(result, Ok(()));
                    }
                }
            }
        }
    }

    // Verify that a full neighbor table does not allow adding new neighbors.
    #[ip_test(I)]
    #[test_case(InsertMethod::ResolveLinkAddr; "resolve_link_addr")]
    #[test_case(InsertMethod::InsertStaticEntry; "insert_static_entry")]
    #[test_case(InsertMethod::NeighborUpdate; "neighbor_update")]
    #[test_case(InsertMethod::SendIpPacket; "send_ip_packet")]
    fn neighbor_table_max_entries_enforced<I: TestIpExt>(insert_method: InsertMethod) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        for i in 0..MAX_ENTRIES {
            // NB: Static entries will not be discardable.
            init_static_neighbor_with_ip(
                &mut core_ctx,
                &mut bindings_ctx,
                generate_ip_addr::<I>(i),
                LINK_ADDR1,
                ExpectedEvent::Added,
            );
            assert_eq!(core_ctx.nud.state.neighbors.len(), i + 1);
        }

        // Attempting to insert an entry beyond the limit should fail.
        insert_method.insert(
            &mut CtxPair { core_ctx: &mut core_ctx, bindings_ctx: &mut bindings_ctx },
            LINK_ADDR1,
            generate_ip_addr::<I>(MAX_ENTRIES),
            true, /* expect_err */
        );
        assert_eq!(bindings_ctx.take_events(), []);
        assert_eq!(core_ctx.nud.state.neighbors.len(), MAX_ENTRIES);
    }

    // Verify that a full neighbor table will run the garbage collector to free
    // discardable entries.
    #[ip_test(I)]
    #[test_case(InsertMethod::ResolveLinkAddr; "resolve_link_addr")]
    #[test_case(InsertMethod::InsertStaticEntry; "insert_static_entry")]
    #[test_case(InsertMethod::NeighborUpdate; "neighbor_update")]
    #[test_case(InsertMethod::SendIpPacket; "send_ip_packet")]
    fn insert_new_entry_may_collect_garbage<I: TestIpExt>(insert_method: InsertMethod) {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        for i in 0..MAX_ENTRIES {
            // NB: STALE entries will be discardable.
            init_stale_neighbor_with_ip(
                &mut core_ctx,
                &mut bindings_ctx,
                generate_ip_addr::<I>(i),
                LINK_ADDR1,
            );
            assert_eq!(core_ctx.nud.state.neighbors.len(), i + 1);
        }

        // Attempting to insert an entry beyond the limit should trigger
        // garbage collection.
        insert_method.insert(
            &mut CtxPair { core_ctx: &mut core_ctx, bindings_ctx: &mut bindings_ctx },
            LINK_ADDR1,
            generate_ip_addr::<I>(MAX_ENTRIES),
            false, /* expect_err */
        );
        assert_eq!(core_ctx.nud.state.neighbors.len(), GC_THRESHOLD + 1);
        let mut events = bindings_ctx.take_events();
        // Expect 1 `Added` event and several `Removed` events.
        let add_event = events.pop().expect("should have added event");
        assert_eq!(add_event.addr, generate_ip_addr::<I>(MAX_ENTRIES));
        assert_matches!(add_event.kind, EventKind::Added(_));
        for _ in 0..(MAX_ENTRIES - GC_THRESHOLD) {
            assert_matches!(events.pop(), Some(Event{kind, ..}) if kind == EventKind::Removed);
        }
        assert_matches!(&events[..], []);
    }
}
