// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Duplicate Address Detection.

use core::convert::Infallible as Never;
use core::fmt::Debug;
use core::mem;
use core::num::{NonZero, NonZeroU16};
use core::ops::RangeInclusive;
use core::time::Duration;

use arrayvec::ArrayVec;
use assert_matches::assert_matches;
use derivative::Derivative;
use log::debug;
use net_types::MulticastAddr;
use net_types::ip::{
    GenericOverIp, Ip, IpVersion, IpVersionMarker, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr,
};
use netstack3_base::{
    AnyDevice, CoreTimerContext, DeviceIdContext, EventContext, HandleableTimer,
    InstantBindingsTypes, IpAddressId as _, IpDeviceAddressIdContext, RngContext,
    StrongDeviceIdentifier as _, TimerBindingsTypes, TimerContext, WeakDeviceIdentifier,
};
use packet_formats::icmp::ndp::NeighborSolicitation;
use packet_formats::icmp::ndp::options::{MIN_NONCE_LENGTH, NdpNonce};
use packet_formats::utils::NonZeroDuration;
use rand::RngExt as _;

use crate::internal::device::nud::DEFAULT_MAX_MULTICAST_SOLICIT;
use crate::internal::device::{IpAddressState, IpDeviceEvent, IpDeviceIpExt, WeakIpAddressId};

/// An IP Extension trait supporting Duplicate Address Detection.
pub trait DadIpExt: Ip {
    /// Whether or not DAD should be performed by default of a newly installed
    /// address.
    const DEFAULT_DAD_ENABLED: bool;

    /// Data that accompanies a sent DAD probe/announcement.
    type SendData: Debug;
    /// Data that accompanies a received DAD packet.
    type ReceivedPacketData<'a>;
    /// State held for tentative addresses.
    type TentativeState: Debug + Default + Send + Sync;
    /// State held for announcing addresses.
    type AnnouncingState: Debug + Send + Sync;
    /// Metadata associated with the result of handling and incoming DAD packet.
    type IncomingPacketResultMeta;
    /// Data used to determine the interval to wait between DAD probes.
    type RetransmitTimerData;

    /// Generate data to accompany a sent probe from the tentative state.
    fn generate_sent_probe_data<BC: RngContext>(
        state: &mut Self::TentativeState,
        addr: Self::Addr,
        bindings_ctx: &mut BC,
    ) -> Self::SendData;

    /// Generate the delay to wait before the next DAD probe.
    fn get_retransmission_interval<BC: RngContext>(
        data: &Self::RetransmitTimerData,
        bindings_ctx: &mut BC,
        probes_remaining: &Option<NonZeroU16>,
    ) -> NonZeroDuration;

    /// Handles an incoming DAD packet while in the tentative state.
    fn handle_incoming_packet_while_tentative<'a>(
        data: Self::ReceivedPacketData<'a>,
        state: &mut Self::TentativeState,
        dad_transmits_remaining: &mut Option<NonZeroU16>,
    ) -> Self::IncomingPacketResultMeta;

    /// Handles an incoming DAD packet while in the assigned or announcing
    /// state.
    ///
    /// `ran_dad`: indicates whether this address is assigned because DAD
    /// completed successfully, or because DAD was disabled and skipped.
    ///
    /// Returns whether the address should be removed as a result of this
    /// packet.
    fn handle_incoming_packet_while_assigned<'a>(
        data: Self::ReceivedPacketData<'a>,
        ran_dad: bool,
    ) -> bool;
}

impl DadIpExt for Ipv4 {
    /// In the context of IPv4 addresses, DAD refers to Address Conflict Detection
    /// (ACD) as specified in RFC 5227.
    ///
    /// This value is set to false, which is out of compliance with RFC 5227. As per
    /// section 2.1:
    ///   Before beginning to use an IPv4 address (whether received from manual
    ///   configuration, DHCP, or some other means), a host implementing this
    ///   specification MUST test to see if the address is already in use.
    ///
    /// However, we believe that disabling DAD for IPv4 addresses by default is more
    /// inline with industry expectations. For example, Linux does not implement
    /// DAD for IPv4 addresses at all: applications that want to prevent duplicate
    /// IPv4 addresses must implement the ACD specification themselves (e.g.
    /// dhclient, a common DHCP client on Linux).
    const DEFAULT_DAD_ENABLED: bool = false;

    type SendData = Ipv4DadSendData;
    type ReceivedPacketData<'a> = Ipv4DadAddressInfo;
    type TentativeState = ();
    type AnnouncingState = Ipv4AnnouncingDadState;
    type IncomingPacketResultMeta = ();
    type RetransmitTimerData = ();

    fn generate_sent_probe_data<BC: RngContext>(
        _state: &mut (),
        addr: Ipv4Addr,
        _bindings_ctx: &mut BC,
    ) -> Self::SendData {
        Ipv4DadSendData { target_ip: addr, probe_type: Ipv4SentProbeType::Probe }
    }

    fn get_retransmission_interval<BC: RngContext>(
        _data: &(),
        bindings_ctx: &mut BC,
        probes_remaining: &Option<NonZeroU16>,
    ) -> NonZeroDuration {
        match probes_remaining {
            // If we have more probes to send, use the delay *between* probes.
            Some(_) => ipv4_dad_probe_interval(bindings_ctx),
            // Otherwise, use the delay before the announcing period.
            None => IPV4_ANNOUNCE_WAIT,
        }
    }

    fn handle_incoming_packet_while_tentative<'a>(
        _data: Ipv4DadAddressInfo,
        _state: &mut (),
        _dad_transmits_remaining: &mut Option<NonZeroU16>,
    ) -> () {
        // TODO(https://fxbug.dev/416523141): Once we support ratelimiting, this
        // would be an appropriate place to acknowledge that a conflict has
        // occurred.
    }

    fn handle_incoming_packet_while_assigned<'a>(data: Ipv4DadAddressInfo, ran_dad: bool) -> bool {
        match IPV4_ADDRESS_DEFENSE_STRATEGY {
            // Forfeit the address, only if 1) DAD was enabled on the address
            // and 2) the conflict was for the source address of the ARP packet
            AddressDefenseStrategy::ForfeitAddress => {
                ran_dad && data == Ipv4DadAddressInfo::SourceAddr
            }
        }
    }
}

impl DadIpExt for Ipv6 {
    /// True, as per RFC 4862, Section 5.4:
    ///   Duplicate Address Detection MUST be performed on all unicast
    ///   addresses prior to assigning them to an interface, regardless of
    ///   whether they are obtained through stateless autoconfiguration,
    ///   DHCPv6, or manual configuration.
    const DEFAULT_DAD_ENABLED: bool = true;

    type SendData = Ipv6DadSendData;
    type ReceivedPacketData<'a> = Option<NdpNonce<&'a [u8]>>;
    type TentativeState = Ipv6TentativeDadState;
    // Dad for IPv6 addresses does not have an announcing period.
    type AnnouncingState = Never;
    type IncomingPacketResultMeta = Ipv6PacketResultMetadata;
    type RetransmitTimerData = NonZeroDuration;

    fn generate_sent_probe_data<BC: RngContext>(
        state: &mut Ipv6TentativeDadState,
        addr: Ipv6Addr,
        bindings_ctx: &mut BC,
    ) -> Self::SendData {
        let Ipv6TentativeDadState {
            nonces,
            added_extra_transmits_after_detecting_looped_back_ns: _,
        } = state;
        Ipv6DadSendData {
            dst_ip: addr.to_solicited_node_address(),
            message: NeighborSolicitation::new(addr),
            nonce: nonces.evicting_create_and_store_nonce(bindings_ctx.rng()),
        }
    }

    fn get_retransmission_interval<BC: RngContext>(
        data: &NonZeroDuration,
        _bindings_ctx: &mut BC,
        _probes_remaining: &Option<NonZeroU16>,
    ) -> NonZeroDuration {
        *data
    }

    fn handle_incoming_packet_while_tentative<'a>(
        data: Option<NdpNonce<&'a [u8]>>,
        state: &mut Ipv6TentativeDadState,
        dad_transmits_remaining: &mut Option<NonZeroU16>,
    ) -> Ipv6PacketResultMetadata {
        // Check if the incoming nonce matches stored nonces.
        let Ipv6TentativeDadState { nonces, added_extra_transmits_after_detecting_looped_back_ns } =
            state;
        let matched_nonce = data.is_some_and(|nonce| nonces.contains(nonce.bytes()));
        if matched_nonce
            && !core::mem::replace(added_extra_transmits_after_detecting_looped_back_ns, true)
        {
            // Detected a looped-back DAD neighbor solicitation.
            // Per RFC 7527, we should send MAX_MULTICAST_SOLICIT more DAD probes.
            *dad_transmits_remaining = Some(
                DEFAULT_MAX_MULTICAST_SOLICIT
                    .saturating_add(dad_transmits_remaining.map(NonZero::get).unwrap_or(0)),
            );
        }
        Ipv6PacketResultMetadata { matched_nonce }
    }

    fn handle_incoming_packet_while_assigned<'a>(
        _data: Option<NdpNonce<&'a [u8]>>,
        _ran_dad: bool,
    ) -> bool {
        // IPv6 doesn't have a notion of ongoing address conflict detection.
        // Once we've assigned an address, we refuse to forfeit it.
        false
    }
}

/// Additional data about a received IPv4 DAD packet.
#[derive(Debug, PartialEq)]
pub enum Ipv4DadAddressInfo {
    /// The address in question was the source address of the ARP packet.
    SourceAddr,
    /// The address in question was the target address of the ARP packet.
    TargetAddr,
}

/// The type of IPv4 DAD probe to send.
#[derive(Debug, PartialEq)]
pub enum Ipv4SentProbeType {
    /// An "ARP Probe" as specified in RFC 5227 Section 1.1.
    Probe,
    /// An "ARP Announcement" as specified in RFC 5227 Section 1.1.
    Announcement,
}

/// The data needed to send an IPv4 DAD probe/announcement.
#[derive(Debug)]
pub struct Ipv4DadSendData {
    /// The Ipv4Addr to probe.
    target_ip: Ipv4Addr,
    /// The type of DAD probe to send
    probe_type: Ipv4SentProbeType,
}

impl Ipv4DadSendData {
    /// Returns the sender and target IP addresses to send in the ARP request.
    pub fn into_sender_and_target_addr(self) -> (Ipv4Addr, Ipv4Addr) {
        let Self { target_ip, probe_type } = self;
        let sender_ip = match probe_type {
            // As per RFC 5227 section 2.1.1 for "ARP probes":
            //   The 'sender IP address' field MUST be set to all zeroes.
            Ipv4SentProbeType::Probe => Ipv4::UNSPECIFIED_ADDRESS,
            // As per RFC 5227 section 2.3 for "ARP announcements":
            //   The sender and target IP addresses are both set to the
            //   host's newly selected IPv4 address
            Ipv4SentProbeType::Announcement => target_ip.clone(),
        };
        // As per RFC 5227 section 2.1.1:
        //   The 'target IP address' field MUST be set to the address
        //   being probed.
        (sender_ip, target_ip)
    }
}

/// The data needed to send an IPv6 DAD probe.
#[derive(Debug)]
pub struct Ipv6DadSendData {
    /// The destination address of the probe.
    pub dst_ip: MulticastAddr<Ipv6Addr>,
    /// The Neighbor Solicication to send.
    pub message: NeighborSolicitation,
    /// The nonce to accompany the neighbor solicitation.
    pub nonce: OwnedNdpNonce,
}

/// A timer ID for duplicate address detection.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct DadTimerId<I: Ip, D: WeakDeviceIdentifier, A: WeakIpAddressId<I::Addr>> {
    pub(crate) device_id: D,
    pub(crate) addr: A,
    _marker: IpVersionMarker<I>,
}

impl<I: Ip, D: WeakDeviceIdentifier, A: WeakIpAddressId<I::Addr>> DadTimerId<I, D, A> {
    pub(super) fn device_id(&self) -> &D {
        let Self { device_id, addr: _, _marker } = self;
        device_id
    }

    /// Creates a new [`DadTimerId`]  for `device_id` and `addr`.
    #[cfg(any(test, feature = "testutils"))]
    pub fn new(device_id: D, addr: A) -> Self {
        Self { device_id, addr, _marker: IpVersionMarker::new() }
    }
}

/// A reference to the DAD address state.
pub struct DadAddressStateRef<'a, I: DadIpExt, CC, BT: DadBindingsTypes> {
    /// A mutable reference to an address' state.
    pub dad_state: &'a mut DadState<I, BT>,
    /// The execution context available with the address's DAD state.
    pub core_ctx: &'a mut CC,
}

/// Holds references to state associated with duplicate address detection.
pub struct DadStateRef<'a, I: DadIpExt, CC, BT: DadBindingsTypes> {
    /// A reference to the DAD address state.
    pub state: DadAddressStateRef<'a, I, CC, BT>,
    /// Data used to calculate the time between DAD probe retransmissions.
    pub retrans_timer_data: &'a I::RetransmitTimerData,
    /// The maximum number of DAD messages to send.
    pub max_dad_transmits: &'a Option<NonZeroU16>,
}

/// The execution context while performing DAD.
pub trait DadAddressContext<I: Ip, BC>: IpDeviceAddressIdContext<I> {
    /// Calls the function with a mutable reference to the address's assigned
    /// flag.
    fn with_address_assigned<O, F: FnOnce(&mut bool) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        addr: &Self::AddressId,
        cb: F,
    ) -> O;

    /// Returns whether or not DAD should be performed for the given device and
    /// address.
    fn should_perform_dad(&mut self, device_id: &Self::DeviceId, addr: &Self::AddressId) -> bool;
}

/// Like [`DadAddressContext`], with additional IPv6 specific functionality.
pub trait Ipv6DadAddressContext<BC>: DadAddressContext<Ipv6, BC> {
    /// Joins the multicast group on the device.
    fn join_multicast_group(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        multicast_addr: MulticastAddr<Ipv6Addr>,
    );

    /// Leaves the multicast group on the device.
    fn leave_multicast_group(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        multicast_addr: MulticastAddr<Ipv6Addr>,
    );
}

/// The execution context for DAD.
pub trait DadContext<I: IpDeviceIpExt, BC: DadBindingsTypes>:
    IpDeviceAddressIdContext<I>
    + DeviceIdContext<AnyDevice>
    + CoreTimerContext<DadTimerId<I, Self::WeakDeviceId, Self::WeakAddressId>, BC>
{
    /// The inner address context.
    type DadAddressCtx<'a>: DadAddressContext<I, BC, DeviceId = Self::DeviceId, AddressId = Self::AddressId>;

    /// Calls the function with the DAD state associated with the address.
    fn with_dad_state<O, F: FnOnce(DadStateRef<'_, I, Self::DadAddressCtx<'_>, BC>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        addr: &Self::AddressId,
        cb: F,
    ) -> O;

    /// Sends a DAD probe.
    fn send_dad_probe(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        data: I::SendData,
    );
}

/// The various states DAD may be in for an address.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub enum DadState<I: DadIpExt, BT: DadBindingsTypes> {
    /// The address is assigned to an interface and can be considered bound to
    /// it (all packets destined to the address will be accepted).
    Assigned {
        /// Indicates whether the address is assigned because DAD completed
        /// successfully, or because DAD was disabled and skipped.
        ran_dad: bool,
    },

    /// Like [`Self::Assigned`], but the DAD engine is announcing to the network
    /// that we are using this address.
    ///
    /// This state corresponds to the announcing described in RFC 5227 section
    /// 2.3 and as such only applies to IPv4 addresses. The IPv6 DAD
    /// specification (RFC 4862 section 5.4) does not define an announcement
    /// period.
    Announcing {
        /// The timer used to schedule DAD operations in the future.
        timer: BT::Timer,
        /// Announcing state specific to the given IP version.
        ip_specific_state: I::AnnouncingState,
    },

    /// The address is considered unassigned to an interface for normal
    /// operations, but has the intention of being assigned in the future (e.g.
    /// once Duplicate Address Detection is completed).
    Tentative {
        /// The number of probes that still need to be sent. When `None`, there
        /// are no more probes to send and DAD may resolve.
        dad_transmits_remaining: Option<NonZeroU16>,
        /// The timer used to schedule DAD operations in the future.
        timer: BT::Timer,
        /// Probing state specific to the given IP version.
        ip_specific_state: I::TentativeState,
        /// The (optional) delay to wait before starting to send DAD probes.
        probe_wait: Option<Duration>,
    },

    /// The address has not yet been initialized.
    Uninitialized,
}

impl<I: DadIpExt, BT: DadBindingsTypes> DadState<I, BT> {
    pub(crate) fn is_assigned(&self) -> bool {
        match self {
            DadState::Assigned { .. } => true,
            // As per RFC 5227 section 2.3:
            //   The host may begin legitimately using the IP address
            //   immediately after sending the first of the two ARP
            //   Announcements.
            // As such, consider an address in Announcing to be assigned.
            DadState::Announcing { .. } => true,
            DadState::Tentative { .. } | DadState::Uninitialized => false,
        }
    }
}

/// Dad state specific to announcing IPv4 addresses.
#[derive(Debug)]
pub struct Ipv4AnnouncingDadState {
    /// The number of announcements that still need to be sent.
    pub announcements_remaining: NonZeroU16,
}

/// DAD state specific to tentative IPv6 addresses.
#[derive(Debug, Default)]
pub struct Ipv6TentativeDadState {
    /// The collection of observed nonces.
    ///
    /// Used to detect looped back Neighbor Solicitations.
    pub nonces: NonceCollection,
    /// Initialized to false, and exists as a sentinel so that extra
    /// transmits are added only after the first looped-back probe is
    /// detected.
    pub added_extra_transmits_after_detecting_looped_back_ns: bool,
}

// Chosen somewhat arbitrarily. It's unlikely we need to store many
// previously-used nonces given that we'll probably only ever see the most
// recently used nonce looped back at us.
const MAX_DAD_PROBE_NONCES_STORED: usize = 4;

/// Like [`NdpNonce`], but owns the underlying data.
pub type OwnedNdpNonce = [u8; MIN_NONCE_LENGTH];

/// A data structure storing a limited number of `NdpNonce`s.
#[derive(Default, Debug)]
pub struct NonceCollection {
    nonces: ArrayVec<OwnedNdpNonce, MAX_DAD_PROBE_NONCES_STORED>,
}

impl NonceCollection {
    /// Given an `rng` source, generates a new unique nonce and stores it,
    /// deleting the oldest nonce if there is no space remaining.
    pub fn evicting_create_and_store_nonce(&mut self, mut rng: impl rand::Rng) -> OwnedNdpNonce {
        let Self { nonces } = self;
        loop {
            let nonce: OwnedNdpNonce = rng.random();
            if nonces.iter().any(|stored_nonce| stored_nonce == &nonce) {
                continue;
            }

            if nonces.remaining_capacity() == 0 {
                let _: OwnedNdpNonce = nonces.remove(0);
            }
            nonces.push(nonce.clone());
            break nonce;
        }
    }

    /// Checks if `nonce` is in the collection.
    pub fn contains(&self, nonce: &[u8]) -> bool {
        if nonce.len() != MIN_NONCE_LENGTH {
            return false;
        }

        let Self { nonces } = self;
        nonces.iter().any(|stored_nonce| stored_nonce == &nonce)
    }
}

/// The bindings types for DAD.
pub trait DadBindingsTypes: TimerBindingsTypes + InstantBindingsTypes {}
impl<BT> DadBindingsTypes for BT where BT: TimerBindingsTypes + InstantBindingsTypes {}

/// The bindings execution context for DAD.
pub trait DadBindingsContext<I: Ip, D>:
    DadBindingsTypes + TimerContext + EventContext<IpDeviceEvent<D, I, Self::Instant>> + RngContext
{
}
impl<I: Ip, D, BC> DadBindingsContext<I, D> for BC where
    BC: DadBindingsTypes
        + TimerContext
        + EventContext<IpDeviceEvent<D, I, Self::Instant>>
        + RngContext
{
}

/// The result of handling an incoming DAD probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DadIncomingPacketResult<I: DadIpExt> {
    /// The probe's address is not assigned to ourself.
    Uninitialized,
    /// The probe's address is tentatively assigned to ourself.
    ///
    /// Includes IP specific `meta` related to handling this probe.
    Tentative { meta: I::IncomingPacketResultMeta },
    /// The probe's address is assigned to ourself.
    ///
    /// Includes `should_remove`, dictating whether the assigned address
    /// should be removed as a result of this probe.
    Assigned { should_remove: bool },
}

/// IPv6 specific metadata held by [`DadIncomingPacketResult`].
#[derive(Debug, PartialEq)]
pub struct Ipv6PacketResultMetadata {
    /// True if the incoming Neighbor Solicitation contained a nonce that
    /// matched a nonce previously sent by ourself. This indicates that the
    /// network looped back a Neighbor Solicitation from ourself.
    pub(crate) matched_nonce: bool,
}

/// An implementation for Duplicate Address Detection.
pub trait DadHandler<I: IpDeviceIpExt, BC: DadBindingsTypes>:
    DeviceIdContext<AnyDevice> + IpDeviceAddressIdContext<I>
{
    /// Initializes the DAD state for the given device and address.
    ///
    /// If DAD is required, the return value holds a [`StartDad`] token that
    /// can be used to start the DAD algorithm.
    fn initialize_duplicate_address_detection<
        'a,
        F: FnOnce(IpAddressState) -> IpDeviceEvent<Self::DeviceId, I, BC::Instant>,
    >(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &'a Self::DeviceId,
        addr: &'a Self::AddressId,
        into_bindings_event: F,
    ) -> NeedsDad<'a, Self::AddressId, Self::DeviceId>;

    /// Starts duplicate address detection.
    ///
    /// The provided [`StartDad`] token is proof that DAD is required for the
    /// address & device.
    fn start_duplicate_address_detection<'a>(
        &mut self,
        bindings_ctx: &mut BC,
        start_dad: StartDad<'_, Self::AddressId, Self::DeviceId>,
    );

    /// Stops duplicate address detection.
    ///
    /// Does nothing if DAD is not being performed on the address.
    fn stop_duplicate_address_detection(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr: &Self::AddressId,
    );

    /// Handles an incoming duplicate address detection packet.
    ///
    /// This packet is indicative of the sender (possibly ourselves, if the
    /// packet was looped back) performing duplicate address detection.
    ///
    /// The returned state indicates the address' state on ourself.
    fn handle_incoming_packet(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr: &Self::AddressId,
        data: I::ReceivedPacketData<'_>,
    ) -> DadIncomingPacketResult<I>;
}

/// Indicates whether DAD is needed for a given address on a given device.
#[derive(Debug)]
pub enum NeedsDad<'a, A, D> {
    No,
    Yes(StartDad<'a, A, D>),
}

/// Signals that DAD is allowed to run for the given address & device.
///
/// Inner members are private to ensure the type can only be constructed in the
/// current module, which ensures that duplicate address detection can only be
/// started after having checked that it's necessary.
#[derive(Debug)]
pub struct StartDad<'a, A, D> {
    address_id: &'a A,
    device_id: &'a D,
}

/// Initializes the DAD state for the given device and address.
fn initialize_duplicate_address_detection<
    'a,
    I: IpDeviceIpExt,
    BC: DadBindingsContext<I, CC::DeviceId>,
    CC: DadContext<I, BC>,
    F1: FnOnce(IpAddressState) -> IpDeviceEvent<CC::DeviceId, I, BC::Instant>,
    F2: FnOnce(&mut CC::DadAddressCtx<'_>, &mut BC, &CC::DeviceId, &CC::AddressId),
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &'a CC::DeviceId,
    addr: &'a CC::AddressId,
    into_bindings_event: F1,
    on_initialized_cb: F2,
) -> NeedsDad<'a, CC::AddressId, CC::DeviceId> {
    core_ctx.with_dad_state(
        device_id,
        addr,
        |DadStateRef { state, retrans_timer_data: _, max_dad_transmits }| {
            let DadAddressStateRef { dad_state, core_ctx } = state;
            let needs_dad = match (core_ctx.should_perform_dad(device_id, addr), max_dad_transmits)
            {
                // There are two mechanisms by which DAD may be disabled:
                //   1) The address has opted out of DAD, or
                //   2) the interface's `max_dad_transmits` is `None`.
                // In either case, the address immediately enters `Assigned`.
                (false, _) | (true, None) => {
                    *dad_state = DadState::Assigned { ran_dad: false };
                    core_ctx.with_address_assigned(device_id, addr, |assigned| *assigned = true);
                    NeedsDad::No
                }
                (true, Some(max_dad_transmits)) => {
                    // Note: IPv4 requires waiting before starting to send DAD
                    // probes, while IPv6 starts sending DAD probes immediately.
                    let probe_wait = match I::VERSION {
                        IpVersion::V4 => Some(ipv4_dad_probe_wait(bindings_ctx)),
                        IpVersion::V6 => None,
                    };
                    *dad_state = DadState::Tentative {
                        dad_transmits_remaining: Some(*max_dad_transmits),
                        timer: CC::new_timer(
                            bindings_ctx,
                            DadTimerId {
                                device_id: device_id.downgrade(),
                                addr: addr.downgrade(),
                                _marker: IpVersionMarker::new(),
                            },
                        ),
                        ip_specific_state: Default::default(),
                        probe_wait,
                    };
                    core_ctx.with_address_assigned(device_id, addr, |assigned| *assigned = false);
                    NeedsDad::Yes(StartDad { device_id, address_id: addr })
                }
            };

            // Run IP specific "on initialized" actions while holding the dad
            // state lock.
            on_initialized_cb(core_ctx, bindings_ctx, device_id, addr);

            // Dispatch the appropriate event to bindings while holding the dad
            // state lock.
            let address_state = match &needs_dad {
                NeedsDad::No => IpAddressState::Assigned,
                NeedsDad::Yes(_) => IpAddressState::Tentative,
            };
            bindings_ctx.on_event(into_bindings_event(address_state));

            needs_dad
        },
    )
}

fn do_duplicate_address_detection<
    I: IpDeviceIpExt,
    BC: DadBindingsContext<I, CC::DeviceId>,
    CC: DadContext<I, BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    addr: &CC::AddressId,
    timer_id: Option<BC::UniqueTimerId>,
) {
    let should_send_probe = core_ctx.with_dad_state(
        device_id,
        addr,
        |DadStateRef { state, retrans_timer_data, max_dad_transmits: _ }| {
            let DadAddressStateRef { dad_state, core_ctx } = state;
            match dad_state {
                DadState::Uninitialized => {
                    // Note: Starting DAD must have raced with stopping DAD.
                    // Short circuit DAD execution by returning `None`.
                    return None;
                }
                DadState::Assigned { .. } => {
                    panic!("cannot do DAD for an already assigned address; addr={addr:?}")
                }
                DadState::Tentative {
                    dad_transmits_remaining,
                    timer,
                    ip_specific_state,
                    probe_wait,
                } => {
                    if let Some(timer_id) = timer_id
                        && timer_id != bindings_ctx.unique_timer_id(timer)
                    {
                        // This timer was from a previous round of DAD for this
                        // (device, address). Just ignore it.
                        return None;
                    }

                    let (should_send_probe, state_change) = run_tentative_step::<I, CC, BC>(
                        core_ctx,
                        bindings_ctx,
                        device_id,
                        addr,
                        retrans_timer_data,
                        dad_transmits_remaining,
                        timer,
                        ip_specific_state,
                        probe_wait,
                    );
                    match state_change {
                        DadStateChangeFromTentative::None => {}
                        DadStateChangeFromTentative::ToAssigned => {
                            *dad_state = DadState::Assigned { ran_dad: true }
                        }
                        DadStateChangeFromTentative::ToAnnouncing { ip_specific_state } => {
                            // Note: Because of Rust semantics, we cannot
                            // convert directly from tentative to announcing.
                            // Instead we use uninitialized as an intermediary
                            // step to gain access to an owned `timer`.
                            let orig = mem::replace(dad_state, DadState::Uninitialized);
                            let timer = assert_matches!(
                                orig,
                                DadState::Tentative{timer, ..} => timer,
                                "state must be tentative"
                            );
                            *dad_state = DadState::Announcing { ip_specific_state, timer }
                        }
                    }
                    should_send_probe
                }
                DadState::Announcing { ip_specific_state, timer } => {
                    if let Some(timer_id) = timer_id
                        && timer_id != bindings_ctx.unique_timer_id(timer)
                    {
                        // This timer was from a previous round of DAD for this
                        // (device, address). Just ignore it.
                        return None;
                    }

                    #[derive(GenericOverIp)]
                    #[generic_over_ip(I, Ip)]
                    struct WrapIn<'a, I: DadIpExt>(&'a mut I::AnnouncingState, I::Addr);
                    #[derive(GenericOverIp)]
                    #[generic_over_ip(I, Ip)]
                    struct WrapOut<I: DadIpExt>(I::SendData, DadStateChangeFromAnnouncing);
                    let WrapOut(send_data, state_change) = I::map_ip(
                        WrapIn(ip_specific_state, addr.addr().addr()),
                        |WrapIn(ipv4_state, ipv4_addr)| {
                            let (send_data, state_change) = run_ipv4_announcing_step(
                                bindings_ctx,
                                device_id,
                                ipv4_addr,
                                ipv4_state,
                                timer,
                            );
                            WrapOut(send_data, state_change)
                        },
                        |WrapIn(ipv6_state, _ipv6_addr)| match *ipv6_state {},
                    );
                    match state_change {
                        DadStateChangeFromAnnouncing::None => {}
                        DadStateChangeFromAnnouncing::ToAssigned => {
                            *dad_state = DadState::Assigned { ran_dad: true }
                        }
                    }
                    Some(send_data)
                }
            }
        },
    );

    if let Some(probe_send_data) = should_send_probe {
        core_ctx.send_dad_probe(bindings_ctx, device_id, probe_send_data);
    }
}

/// A change in [`DadState`] variant that occurs from [`run_tentative_step()`].
enum DadStateChangeFromTentative<I: DadIpExt> {
    None,
    ToAssigned,
    ToAnnouncing { ip_specific_state: I::AnnouncingState },
}

/// Runs a single step of the DAD algorithm for an address in [`DadState::Tentative`].
///
/// Returns `Some(I::SendData)` if a DAD probe should be sent, and
/// [`DadStateChangeFromTentative`] if the state machine needs to transition
/// into a new state.
fn run_tentative_step<
    I: IpDeviceIpExt,
    CC: DadContext<I, BC>,
    BC: DadBindingsContext<I, CC::DeviceId>,
>(
    core_ctx: &mut CC::DadAddressCtx<'_>,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    addr: &CC::AddressId,
    retrans_timer_data: &I::RetransmitTimerData,
    dad_transmits_remaining: &mut Option<NonZeroU16>,
    timer: &mut BC::Timer,
    ip_specific_state: &mut I::TentativeState,
    probe_wait: &mut Option<Duration>,
) -> (Option<I::SendData>, DadStateChangeFromTentative<I>) {
    if let Some(probe_wait) = probe_wait.take() {
        // Schedule a timer and short circuit, deferring the first probe
        // until after the timer fires.
        schedule_dad_timer(bindings_ctx, timer, probe_wait, addr.addr(), device_id);
        return (None, DadStateChangeFromTentative::None);
    }

    match dad_transmits_remaining {
        None => {
            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct WrapOut<I: DadIpExt>(Option<I::SendData>, DadStateChangeFromTentative<I>);
            let WrapOut(send_data, state_change) = I::map_ip(
                addr.addr().addr(),
                // When there are no more probes to be sent, IPv4 addresses
                // should enter the announcing state, see RFC 5227, section 2.3.
                //   Having probed to determine that a desired address may be
                //   used safely, a host implementing this specification MUST
                //   then announce that it is commencing to use this address by
                //   broadcasting ANNOUNCE_NUM ARP Announcements, spaced
                //   ANNOUNCE_INTERVAL seconds apart.
                |ipv4_addr| {
                    let mut state =
                        Ipv4AnnouncingDadState { announcements_remaining: IPV4_DAD_ANNOUNCE_NUM };
                    let (send_data, state_change) = run_ipv4_announcing_step(
                        bindings_ctx,
                        device_id,
                        ipv4_addr,
                        &mut state,
                        timer,
                    );
                    // Generate the associated "FromTentative" state change,
                    // based on the "FromAnnouncing" state change.
                    let state_change = match state_change {
                        DadStateChangeFromAnnouncing::None => {
                            // Announcing did not finish. Record that the
                            // state is now Announcing.
                            DadStateChangeFromTentative::ToAnnouncing { ip_specific_state: state }
                        }
                        DadStateChangeFromAnnouncing::ToAssigned => {
                            // Announcing finished on the first step. Skip it
                            // and go directly to Assigned. In practice we
                            // should never hit this case as it only happens
                            // when `IPV4_DAD_ANNOUNCE_NUM` <= 1.
                            DadStateChangeFromTentative::ToAssigned
                        }
                    };
                    WrapOut(Some(send_data), state_change)
                },
                // IPv6 does not have any announcing period. Transition
                // directly to `DadState::Assigned` without sending any probes.
                |_ipv6_addr| WrapOut(None, DadStateChangeFromTentative::ToAssigned),
            );

            // Regardless of which state we're transitioning to (Announcing or
            // Assigned), the address should be considered assigned.
            // See RFC 5227's commentary on Announcing from section 2.3:
            //   The host may begin legitimately using the IP address
            //   immediately after sending the first of the two ARP
            //   Announcements.
            core_ctx.with_address_assigned(device_id, addr, |assigned| *assigned = true);
            bindings_ctx.on_event(IpDeviceEvent::AddressStateChanged {
                device: device_id.clone(),
                addr: addr.addr_sub().addr().into(),
                state: IpAddressState::Assigned,
            });
            return (send_data, state_change);
        }
        Some(non_zero_remaining) => {
            *dad_transmits_remaining = NonZeroU16::new(non_zero_remaining.get() - 1);

            // Delay sending subsequent DAD probes.
            //
            // For IPv4, per RFC 5227 Section 2.1.1
            //   each of these probe packets spaced randomly and
            //   uniformly, PROBE_MIN to PROBE_MAX seconds apart.
            //
            // And for IPv6, per RFC 4862 section 5.1,
            //   DupAddrDetectTransmits ...
            //      Autoconfiguration also assumes the presence of the variable
            //      RetransTimer as defined in [RFC4861]. For autoconfiguration
            //      purposes, RetransTimer specifies the delay between
            //      consecutive Neighbor Solicitation transmissions performed
            //      during Duplicate Address Detection (if
            //      DupAddrDetectTransmits is greater than 1), as well as the
            //      time a node waits after sending the last Neighbor
            //      Solicitation before ending the Duplicate Address Detection
            //      process.
            let retrans_interval = I::get_retransmission_interval(
                retrans_timer_data,
                bindings_ctx,
                dad_transmits_remaining,
            );
            schedule_dad_timer(bindings_ctx, timer, retrans_interval.get(), addr.addr(), device_id);
            debug!(
                "sending DAD probe for {}; {} remaining",
                addr.addr(),
                dad_transmits_remaining.map_or(0, NonZeroU16::get)
            );
            let send_data =
                I::generate_sent_probe_data(ip_specific_state, addr.addr().addr(), bindings_ctx);
            return (Some(send_data), DadStateChangeFromTentative::None);
        }
    }
}

/// A change in [`DadState`] variant that occurs from [`run_ipv4_announcing_step()`].
enum DadStateChangeFromAnnouncing {
    None,
    ToAssigned,
}

/// Runs a single step of the DAD algorithm for an address in [`DadState::Announcing`].
///
/// Implemented concretely for `Ipv4` rather than any `I` because IPv6 does not
/// support announcing.
///
/// Returns the `Ipv4DadSendData` for the announcement that should be sent, and
/// [`DadStateChangeFromAnnouncing`] if the state machine needs to transition
/// into a new new state.
fn run_ipv4_announcing_step<BC: TimerContext>(
    bindings_ctx: &mut BC,
    device_id: impl Debug,
    addr: Ipv4Addr,
    ip_specific_state: &mut Ipv4AnnouncingDadState,
    timer: &mut BC::Timer,
) -> (Ipv4DadSendData, DadStateChangeFromAnnouncing) {
    let Ipv4AnnouncingDadState { announcements_remaining } = ip_specific_state;

    let new_announcements_remaining = NonZeroU16::new(announcements_remaining.get() - 1);
    debug!(
        "sending DAD announcement for {}; {} remaining",
        addr,
        new_announcements_remaining.map_or(0, NonZeroU16::get)
    );

    let send_data =
        Ipv4DadSendData { target_ip: addr, probe_type: Ipv4SentProbeType::Announcement };

    let state_change = match new_announcements_remaining {
        // This is the final announcement, change to assigned
        None => DadStateChangeFromAnnouncing::ToAssigned,
        Some(non_zero_announcements_remaining) => {
            // There are more announcements to send after this one. Schedule
            // another timer.
            *announcements_remaining = non_zero_announcements_remaining;
            schedule_dad_timer(bindings_ctx, timer, IPV4_ANNOUNCE_INTERVAL.get(), addr, device_id);
            DadStateChangeFromAnnouncing::None
        }
    };
    (send_data, state_change)
}

/// Schedules a [`DadTimer`], panicking if it was already scheduled.
fn schedule_dad_timer<BC: TimerContext>(
    bindings_ctx: &mut BC,
    timer: &mut BC::Timer,
    duration: Duration,
    addr: impl Debug,
    device_id: impl Debug,
) {
    assert_eq!(
        bindings_ctx.schedule_timer(duration, timer),
        None,
        "Unexpected DAD timer; addr={:?}, device_id={:?}",
        addr,
        device_id
    );
}

/// Stop DAD for the given device and address.
fn stop_duplicate_address_detection<
    'a,
    I: IpDeviceIpExt,
    BC: DadBindingsContext<I, CC::DeviceId>,
    CC: DadContext<I, BC>,
    F: FnOnce(&mut CC::DadAddressCtx<'_>, &mut BC, &CC::DeviceId, &CC::AddressId),
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &'a CC::DeviceId,
    addr: &'a CC::AddressId,
    on_stopped_cb: F,
) {
    core_ctx.with_dad_state(
        device_id,
        addr,
        |DadStateRef { state, retrans_timer_data: _, max_dad_transmits: _ }| {
            let DadAddressStateRef { dad_state, core_ctx } = state;

            match dad_state {
                DadState::Assigned { .. } => {}
                DadState::Announcing { timer, .. } | DadState::Tentative { timer, .. } => {
                    // Generally we should have a timer installed in the
                    // tentative/announcing state, but we could be racing with
                    // the timer firing in bindings so we can't assert that it's
                    // installed here.
                    let _: Option<_> = bindings_ctx.cancel_timer(timer);
                }
                // No actions are needed to stop DAD from `Uninitialized`.
                DadState::Uninitialized => return,
            };

            // Undo the work we did when starting/performing DAD by putting
            // the address back into unassigned state.

            *dad_state = DadState::Uninitialized;
            core_ctx.with_address_assigned(device_id, addr, |assigned| *assigned = false);

            // Run IP specific "on stopped" actions while holding the dad state
            // lock.
            on_stopped_cb(core_ctx, bindings_ctx, device_id, addr)
        },
    )
}

// Handles an incoming packet for the given addr received on the given device.
fn handle_incoming_packet<
    'a,
    I: IpDeviceIpExt,
    BC: DadBindingsContext<I, CC::DeviceId>,
    CC: DadContext<I, BC>,
>(
    core_ctx: &mut CC,
    _bindings_ctx: &mut BC,
    device_id: &'a CC::DeviceId,
    addr: &'a CC::AddressId,
    data: I::ReceivedPacketData<'a>,
) -> DadIncomingPacketResult<I> {
    core_ctx.with_dad_state(
        device_id,
        addr,
        |DadStateRef { state, retrans_timer_data: _, max_dad_transmits: _ }| {
            let DadAddressStateRef { dad_state, core_ctx: _ } = state;
            match dad_state {
                DadState::Uninitialized => DadIncomingPacketResult::Uninitialized,
                DadState::Assigned { ran_dad } => DadIncomingPacketResult::Assigned {
                    should_remove: I::handle_incoming_packet_while_assigned(data, *ran_dad),
                },
                DadState::Announcing { .. } => {
                    // NB: If we're in the Announcing state, DAD must be enabled
                    // for the address.
                    let ran_dad = true;
                    DadIncomingPacketResult::Assigned {
                        should_remove: I::handle_incoming_packet_while_assigned(data, ran_dad),
                    }
                }
                DadState::Tentative {
                    dad_transmits_remaining,
                    timer: _,
                    ip_specific_state,
                    probe_wait: _,
                } => DadIncomingPacketResult::Tentative {
                    meta: I::handle_incoming_packet_while_tentative(
                        data,
                        ip_specific_state,
                        dad_transmits_remaining,
                    ),
                },
            }
        },
    )
}

impl<BC: DadBindingsContext<Ipv4, Self::DeviceId>, CC: DadContext<Ipv4, BC>> DadHandler<Ipv4, BC>
    for CC
{
    fn initialize_duplicate_address_detection<
        'a,
        F: FnOnce(IpAddressState) -> IpDeviceEvent<Self::DeviceId, Ipv4, BC::Instant>,
    >(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &'a Self::DeviceId,
        addr: &'a Self::AddressId,
        into_bindings_event: F,
    ) -> NeedsDad<'a, Self::AddressId, Self::DeviceId> {
        initialize_duplicate_address_detection(
            self,
            bindings_ctx,
            device_id,
            addr,
            into_bindings_event,
            // on_initialized_cb
            |_core_ctx, _bindings_ctx, _device_id, _addr| {},
        )
    }

    fn start_duplicate_address_detection<'a>(
        &mut self,
        bindings_ctx: &mut BC,
        start_dad: StartDad<'_, Self::AddressId, Self::DeviceId>,
    ) {
        let StartDad { device_id, address_id } = start_dad;
        do_duplicate_address_detection(self, bindings_ctx, device_id, address_id, None)
    }

    fn stop_duplicate_address_detection(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr: &Self::AddressId,
    ) {
        stop_duplicate_address_detection(
            self,
            bindings_ctx,
            device_id,
            addr,
            // on_stopped_cb
            |_core_ctx, _bindings_ctx, _device_id, _addr| {},
        )
    }

    /// Handles an incoming ARP packet.
    fn handle_incoming_packet(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr: &Self::AddressId,
        data: Ipv4DadAddressInfo,
    ) -> DadIncomingPacketResult<Ipv4> {
        handle_incoming_packet(self, bindings_ctx, device_id, addr, data)
    }
}

impl<BC: DadBindingsContext<Ipv6, Self::DeviceId>, CC: DadContext<Ipv6, BC>> DadHandler<Ipv6, BC>
    for CC
where
    for<'a> CC::DadAddressCtx<'a>: Ipv6DadAddressContext<BC>,
{
    fn initialize_duplicate_address_detection<
        'a,
        F: FnOnce(IpAddressState) -> IpDeviceEvent<Self::DeviceId, Ipv6, BC::Instant>,
    >(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &'a Self::DeviceId,
        addr: &'a Self::AddressId,
        into_bindings_event: F,
    ) -> NeedsDad<'a, Self::AddressId, Self::DeviceId> {
        initialize_duplicate_address_detection(
            self,
            bindings_ctx,
            device_id,
            addr,
            into_bindings_event,
            // on_initialized_cb
            |core_ctx, bindings_ctx, device_id, addr| {
                // As per RFC 4862 section 5.4.2,
                //
                //   Before sending a Neighbor Solicitation, an interface MUST
                //   join the all-nodes multicast address and the solicited-node
                //   multicast address of the tentative address.
                //
                // Note that:
                // * We join the all-nodes multicast address on interface
                //   enable.
                // * We join the solicited-node multicast address, even if the
                //   address is skipping DAD (and therefore, the tentative
                //   state).
                // * We join the solicited-node multicast address *after*
                //   initializing the address. If the address is tentative, it
                //   won't be used as the source for any outgoing MLD message.
                core_ctx.join_multicast_group(
                    bindings_ctx,
                    device_id,
                    addr.addr().addr().to_solicited_node_address(),
                );
            },
        )
    }

    fn start_duplicate_address_detection<'a>(
        &mut self,
        bindings_ctx: &mut BC,
        start_dad: StartDad<'_, Self::AddressId, Self::DeviceId>,
    ) {
        let StartDad { device_id, address_id } = start_dad;
        do_duplicate_address_detection(self, bindings_ctx, device_id, address_id, None)
    }

    fn stop_duplicate_address_detection(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr: &Self::AddressId,
    ) {
        stop_duplicate_address_detection(
            self,
            bindings_ctx,
            device_id,
            addr,
            // on_stopped_cb
            |core_ctx, bindings_ctx, device_id, addr| {
                // Undo the steps taken when DAD was initialized and leave the
                // solicited node multicast group. Note that we leave the
                // solicited-node multicast address *after* stopping dad. The
                // address will no longer be assigned and won't be used as the
                // source for any outgoing MLD message.
                core_ctx.leave_multicast_group(
                    bindings_ctx,
                    device_id,
                    addr.addr().addr().to_solicited_node_address(),
                );
            },
        )
    }

    /// Handles an incoming Neighbor Solicitation / Neighbor Advertisement.
    fn handle_incoming_packet(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        addr: &Self::AddressId,
        data: Option<NdpNonce<&[u8]>>,
    ) -> DadIncomingPacketResult<Ipv6> {
        handle_incoming_packet(self, bindings_ctx, device_id, addr, data)
    }
}

impl<I: IpDeviceIpExt, BC: DadBindingsContext<I, CC::DeviceId>, CC: DadContext<I, BC>>
    HandleableTimer<CC, BC> for DadTimerId<I, CC::WeakDeviceId, CC::WeakAddressId>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, timer_id: BC::UniqueTimerId) {
        let Self { device_id, addr, _marker } = self;
        let Some(device_id) = device_id.upgrade() else {
            return;
        };
        let Some(addr_id) = addr.upgrade() else {
            return;
        };
        do_duplicate_address_detection(core_ctx, bindings_ctx, &device_id, &addr_id, Some(timer_id))
    }
}

/// As defined in RFC 5227, section 1.1:
///  PROBE_MIN            1 second   (minimum delay until repeated probe)
///  PROBE_MAX            2 seconds  (maximum delay until repeated probe)
const IPV4_PROBE_RANGE: RangeInclusive<Duration> = Duration::from_secs(1)..=Duration::from_secs(2);

/// Generate the interval between repeated IPv4 DAD probes.
pub fn ipv4_dad_probe_interval<BC: RngContext>(bindings_ctx: &mut BC) -> NonZeroDuration {
    // As per RFC 5227, section 2.2.1:
    //   ... and should then send PROBE_NUM probe packets, each of these
    // probe packets spaced randomly and uniformly, PROBE_MIN to PROBE_MAX
    // seconds apart.
    let retrans_interval = bindings_ctx.rng().random_range(IPV4_PROBE_RANGE);
    // NB: Safe to unwrap because `IPV4_PROBE_RANGE` contains only non-zero
    // values.
    NonZeroDuration::new(retrans_interval).unwrap()
}

/// As defined in RFC 5227, section 1.1:
///   PROBE_WAIT           1 second   (initial random delay)
const IPV4_PROBE_WAIT_RANGE: RangeInclusive<Duration> = Duration::ZERO..=Duration::from_secs(1);

/// Generate the duration to wait before starting to send IPv4 DAD probes.
pub fn ipv4_dad_probe_wait<BC: RngContext>(bindings_ctx: &mut BC) -> Duration {
    // As per RFC 5227, section 2.2.1:
    //   When ready to begin probing, the host should then wait for a random
    //   time interval selected uniformly in the range zero to PROBE_WAIT
    //   seconds
    bindings_ctx.rng().random_range(IPV4_PROBE_WAIT_RANGE)
}

/// As defined in RFC 5227, section 1.1:
///   ANNOUNCE_WAIT        2 seconds  (delay before announcing)
const IPV4_ANNOUNCE_WAIT: NonZeroDuration = NonZeroDuration::new(Duration::from_secs(2)).unwrap();

/// As defined in RFC 5227, section 1.1:
///   ANNOUNCE_INTERVAL    2 seconds  (time between Announcement packets)
const IPV4_ANNOUNCE_INTERVAL: NonZeroDuration =
    NonZeroDuration::new(Duration::from_secs(2)).unwrap();

/// As per RFC 5227, section 1.1:
///   ANNOUNCE_NUM         2          (number of Announcement packets)
pub const IPV4_DAD_ANNOUNCE_NUM: NonZeroU16 = NonZeroU16::new(2).unwrap();

/// As per RFC 5227, section 2.4
///   To resolve the address conflict, a host MUST respond to a
///   conflicting ARP packet as described in either (a), (b), or
///  (c) below:
///
///   (a) [...] a host MAY elect to immediately cease using the address [...]
///   (b) [...] a host MAY elect to attempt to defend its address by recording
///       the time that the conflicting ARP packet was received, and then
///       broadcasting one single ARP Announcement. [...] Having done this, the
///       host can then continue to use the address normally without any further
///       special action.  However, if this is not the first conflicting ARP
///       packet the host has seen, and the time recorded for the previous
///       conflicting ARP packet is recent, within DEFEND_INTERVAL seconds, then
///       the host MUST immediately cease using this address [...].
///    (c) [...] a host may elect to defend its address indefinitely.  [...] If
///       the host has not seen any other conflicting ARP packets recently,
///       within the last DEFEND_INTERVAL seconds, then it MUST record the time
///       that the conflicting ARP packet was received, and then broadcast one
///       single ARP Announcement, giving its own IP and hardware addresses.
///       Having done this, the host can then continue to use the address
///       normally without any further special action.
enum AddressDefenseStrategy {
    /// Corresponds to Option A from RFC 5227, section 2.4.
    ForfeitAddress,
}

/// As per RFC 5227, section 2.4:
///   For most client machines that do not need a fixed IP address, immediately
///   requesting the configuring agent (human user, DHCP client, etc.) to
///   configure a new address as soon as the conflict is detected is the best
///   way to restore useful communication as quickly as possible.
///
/// Here, we follow the RFC's guidance for "client machines" and
/// implement option (a) by removing the address. One may be
/// concerned about getting bullied by another node on the
/// network into forfeiting our address, however, the RFC points
/// out later in section 5 that:
///
///   A malicious host may send fraudulent ARP packets on the network,
///   interfering with the correct operation of other hosts. For example, it is
///   easy for a host to answer all ARP Requests with Replies giving its own
///   hardware address, thereby claiming ownership of every address on the
///   network.  This specification makes this existing ARP vulnerability no
///   worse, and in some ways makes it better: instead of failing silently with
///   no indication why, hosts implementing this specification either attempt to
///   reconfigure automatically, or at least inform the human user of what is
///   happening.
// TODO(https://fxbug.dev/418220970): In the future "client machine" may not
// accurately describe our use case. Allow configuring the address defense
// strategy.
const IPV4_ADDRESS_DEFENSE_STRATEGY: AddressDefenseStrategy =
    AddressDefenseStrategy::ForfeitAddress;

#[cfg(test)]
mod tests {
    use core::ops::RangeBounds;
    use core::time::Duration;

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use net_types::ip::{AddrSubnet, GenericOverIp, IpAddress as _, IpVersion, Ipv4Addr};
    use net_types::{NonMappedAddr, NonMulticastAddr, SpecifiedAddr, UnicastAddr, Witness as _};
    use netstack3_base::testutil::{
        FakeBindingsCtx, FakeCoreCtx, FakeCryptoRng, FakeDeviceId, FakeInstant,
        FakeTimerCtxExt as _, FakeWeakAddressId, FakeWeakDeviceId,
    };
    use netstack3_base::{
        AssignedAddrIpExt, CtxPair, InstantContext as _, Ipv4DeviceAddr, Ipv6DeviceAddr,
        SendFrameContext as _, TimerHandler,
    };
    use netstack3_hashmap::hash_map::{Entry, HashMap};
    use packet::EmptyBuf;
    use test_case::test_case;

    use super::*;

    struct FakeDadAddressContext<I: IpDeviceIpExt> {
        addr: I::AssignedWitness,
        assigned: bool,
        // NB: DAD only joins multicast groups for IPv6.
        groups: HashMap<MulticastAddr<Ipv6Addr>, usize>,
        should_perform_dad: bool,
    }

    trait TestDadIpExt: IpDeviceIpExt {
        const DAD_ADDRESS: Self::AssignedWitness;

        const DEFAULT_RETRANS_TIMER_DATA: Self::RetransmitTimerData;

        const EXPECTED_NUM_ANNOUNCEMENTS: u16;

        // Returns the range of `FakeInstant` that a timer is expected
        // to be installed within.
        fn expected_timer_range(now: FakeInstant) -> impl RangeBounds<FakeInstant> + Debug;

        type DadHandlerCtx: DadHandler<
                Self,
                FakeBindingsCtxImpl<Self>,
                DeviceId = FakeDeviceId,
                AddressId = AddrSubnet<Self::Addr, Self::AssignedWitness>,
            > + AsRef<FakeDadContext<Self>>;

        // A trampoline method to get access to `DadHandler<I, _>`.
        //
        // Because the base implementation for DadHandler exists only for IPv4
        // and IPv6, we need this helper to access a DadHandler for any `I`.
        fn with_dad_handler<O, F: FnOnce(&mut Self::DadHandlerCtx) -> O>(
            core_ctx: &mut FakeCoreCtxImpl<Self>,
            cb: F,
        ) -> O;
    }

    impl TestDadIpExt for Ipv4 {
        const DAD_ADDRESS: Ipv4DeviceAddr = unsafe {
            NonMulticastAddr::new_unchecked(NonMappedAddr::new_unchecked(
                SpecifiedAddr::new_unchecked(Ipv4Addr::new([192, 168, 0, 1])),
            ))
        };

        const DEFAULT_RETRANS_TIMER_DATA: () = ();

        const EXPECTED_NUM_ANNOUNCEMENTS: u16 = IPV4_DAD_ANNOUNCE_NUM.get();

        fn expected_timer_range(now: FakeInstant) -> impl RangeBounds<FakeInstant> + Debug {
            let FakeInstant { offset } = now;
            FakeInstant::from(offset + *IPV4_PROBE_RANGE.start())
                ..=FakeInstant::from(offset + *IPV4_PROBE_RANGE.end())
        }

        type DadHandlerCtx = FakeCoreCtxImpl<Ipv4>;

        fn with_dad_handler<O, F: FnOnce(&mut FakeCoreCtxImpl<Ipv4>) -> O>(
            core_ctx: &mut FakeCoreCtxImpl<Ipv4>,
            cb: F,
        ) -> O {
            cb(core_ctx)
        }
    }

    impl TestDadIpExt for Ipv6 {
        const DAD_ADDRESS: Ipv6DeviceAddr = unsafe {
            NonMappedAddr::new_unchecked(UnicastAddr::new_unchecked(Ipv6Addr::new([
                0xa, 0, 0, 0, 0, 0, 0, 1,
            ])))
        };

        const DEFAULT_RETRANS_TIMER_DATA: NonZeroDuration =
            NonZeroDuration::new(Duration::from_secs(1)).unwrap();

        // NB: Announcing isn't supported for IPv6.
        const EXPECTED_NUM_ANNOUNCEMENTS: u16 = 0;

        fn expected_timer_range(now: FakeInstant) -> impl RangeBounds<FakeInstant> + Debug {
            let FakeInstant { offset } = now;
            let expected_timer = FakeInstant::from(offset + Self::DEFAULT_RETRANS_TIMER_DATA.get());
            expected_timer..=expected_timer
        }

        type DadHandlerCtx = FakeCoreCtxImpl<Ipv6>;

        fn with_dad_handler<O, F: FnOnce(&mut FakeCoreCtxImpl<Ipv6>) -> O>(
            core_ctx: &mut FakeCoreCtxImpl<Ipv6>,
            cb: F,
        ) -> O {
            cb(core_ctx)
        }
    }

    impl<I: TestDadIpExt> Default for FakeDadAddressContext<I> {
        fn default() -> Self {
            Self {
                addr: I::DAD_ADDRESS,
                assigned: false,
                groups: Default::default(),
                should_perform_dad: true,
            }
        }
    }

    type FakeAddressCtxImpl<I> = FakeCoreCtx<FakeDadAddressContext<I>, (), FakeDeviceId>;

    impl<I: IpDeviceIpExt> DadAddressContext<I, FakeBindingsCtxImpl<I>> for FakeAddressCtxImpl<I> {
        fn with_address_assigned<O, F: FnOnce(&mut bool) -> O>(
            &mut self,
            &FakeDeviceId: &Self::DeviceId,
            request_addr: &Self::AddressId,
            cb: F,
        ) -> O {
            let FakeDadAddressContext { addr, assigned, .. } = &mut self.state;
            assert_eq!(request_addr.addr(), *addr);
            cb(assigned)
        }

        fn should_perform_dad(
            &mut self,
            &FakeDeviceId: &Self::DeviceId,
            request_addr: &Self::AddressId,
        ) -> bool {
            let FakeDadAddressContext { addr, should_perform_dad, .. } = &mut self.state;
            assert_eq!(request_addr.addr(), *addr);
            *should_perform_dad
        }
    }

    impl Ipv6DadAddressContext<FakeBindingsCtxImpl<Ipv6>> for FakeAddressCtxImpl<Ipv6> {
        fn join_multicast_group(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtxImpl<Ipv6>,
            &FakeDeviceId: &Self::DeviceId,
            multicast_addr: MulticastAddr<Ipv6Addr>,
        ) {
            *self.state.groups.entry(multicast_addr).or_default() += 1;
        }

        fn leave_multicast_group(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtxImpl<Ipv6>,
            &FakeDeviceId: &Self::DeviceId,
            multicast_addr: MulticastAddr<Ipv6Addr>,
        ) {
            match self.state.groups.entry(multicast_addr) {
                Entry::Vacant(_) => {}
                Entry::Occupied(mut e) => {
                    let v = e.get_mut();
                    const COUNT_BEFORE_REMOVE: usize = 1;
                    if *v == COUNT_BEFORE_REMOVE {
                        assert_eq!(e.remove(), COUNT_BEFORE_REMOVE);
                    } else {
                        *v -= 1
                    }
                }
            }
        }
    }

    struct FakeDadContext<I: IpDeviceIpExt> {
        state: DadState<I, FakeBindingsCtxImpl<I>>,
        max_dad_transmits: Option<NonZeroU16>,
        address_ctx: FakeAddressCtxImpl<I>,
    }

    type TestDadTimerId<I> = DadTimerId<
        I,
        FakeWeakDeviceId<FakeDeviceId>,
        FakeWeakAddressId<AddrSubnet<<I as Ip>::Addr, <I as AssignedAddrIpExt>::AssignedWitness>>,
    >;

    type FakeBindingsCtxImpl<I> =
        FakeBindingsCtx<TestDadTimerId<I>, IpDeviceEvent<FakeDeviceId, I, FakeInstant>, (), ()>;

    type FakeCoreCtxImpl<I> =
        FakeCoreCtx<FakeDadContext<I>, <I as DadIpExt>::SendData, FakeDeviceId>;

    fn get_address_id<I: IpDeviceIpExt>(
        addr: I::AssignedWitness,
    ) -> AddrSubnet<I::Addr, I::AssignedWitness> {
        AddrSubnet::from_witness(addr, I::Addr::BYTES * 8).unwrap()
    }

    impl<I: IpDeviceIpExt> CoreTimerContext<TestDadTimerId<I>, FakeBindingsCtxImpl<I>>
        for FakeCoreCtxImpl<I>
    {
        fn convert_timer(dispatch_id: TestDadTimerId<I>) -> TestDadTimerId<I> {
            dispatch_id
        }
    }

    impl<I: TestDadIpExt> DadContext<I, FakeBindingsCtxImpl<I>> for FakeCoreCtxImpl<I> {
        type DadAddressCtx<'a> = FakeAddressCtxImpl<I>;

        fn with_dad_state<
            O,
            F: FnOnce(DadStateRef<'_, I, Self::DadAddressCtx<'_>, FakeBindingsCtxImpl<I>>) -> O,
        >(
            &mut self,
            &FakeDeviceId: &FakeDeviceId,
            request_addr: &Self::AddressId,
            cb: F,
        ) -> O {
            let FakeDadContext { state, max_dad_transmits, address_ctx } = &mut self.state;
            let ctx_addr = address_ctx.state.addr;
            let requested_addr = request_addr.addr();
            assert!(
                ctx_addr == requested_addr,
                "invalid address {requested_addr} expected {ctx_addr}"
            );
            cb(DadStateRef {
                state: DadAddressStateRef { dad_state: state, core_ctx: address_ctx },
                retrans_timer_data: &I::DEFAULT_RETRANS_TIMER_DATA,
                max_dad_transmits,
            })
        }

        fn send_dad_probe(
            &mut self,
            bindings_ctx: &mut FakeBindingsCtxImpl<I>,
            &FakeDeviceId: &FakeDeviceId,
            data: I::SendData,
        ) {
            self.send_frame(bindings_ctx, data, EmptyBuf).unwrap()
        }
    }

    type FakeCtx<I> = CtxPair<FakeCoreCtxImpl<I>, FakeBindingsCtxImpl<I>>;

    // An example `into_bindings_event` callback to be provided to
    // `initialize_duplicate_address_detection()`.
    fn into_state_change_event<I: TestDadIpExt>(
        state: IpAddressState,
    ) -> IpDeviceEvent<FakeDeviceId, I, FakeInstant> {
        IpDeviceEvent::AddressStateChanged {
            state,
            addr: I::DAD_ADDRESS.into(),
            device: FakeDeviceId,
        }
    }

    #[ip_test(I)]
    #[should_panic(expected = "cannot do DAD for an already assigned address")]
    fn panic_non_tentative_address_handle_timer<I: TestDadIpExt>() {
        let FakeCtx::<I> { mut core_ctx, mut bindings_ctx } =
            FakeCtx::with_core_ctx(FakeCoreCtxImpl::with_state(FakeDadContext {
                state: DadState::Assigned { ran_dad: true },
                max_dad_transmits: None,
                address_ctx: FakeAddressCtxImpl::with_state(FakeDadAddressContext::default()),
            }));
        TimerHandler::handle_timer(
            &mut core_ctx,
            &mut bindings_ctx,
            dad_timer_id(),
            Default::default(),
        );
    }

    #[ip_test(I)]
    fn dad_disabled<I: TestDadIpExt>() {
        let FakeCtx::<I> { mut core_ctx, mut bindings_ctx } =
            FakeCtx::with_core_ctx(FakeCoreCtxImpl::with_state(FakeDadContext {
                state: DadState::Uninitialized,
                max_dad_transmits: None,
                address_ctx: FakeAddressCtxImpl::with_state(FakeDadAddressContext::default()),
            }));
        let address_id = get_address_id::<I>(I::DAD_ADDRESS);
        let start_dad = I::with_dad_handler(&mut core_ctx, |core_ctx| {
            core_ctx.initialize_duplicate_address_detection(
                &mut bindings_ctx,
                &FakeDeviceId,
                &address_id,
                into_state_change_event::<I>,
            )
        });
        assert_matches!(start_dad, NeedsDad::No);
        let FakeDadContext { state, address_ctx, .. } = &core_ctx.state;
        assert_matches!(*state, DadState::Assigned { ran_dad: false });
        let FakeDadAddressContext { assigned, groups, .. } = &address_ctx.state;
        assert!(*assigned);
        check_multicast_groups(I::VERSION, groups);
        assert_eq!(
            bindings_ctx.take_events(),
            &[IpDeviceEvent::AddressStateChanged {
                state: IpAddressState::Assigned,
                device: FakeDeviceId,
                addr: I::DAD_ADDRESS.into(),
            }][..]
        );
    }

    fn dad_timer_id<I: TestDadIpExt>() -> TestDadTimerId<I> {
        DadTimerId {
            addr: FakeWeakAddressId(get_address_id::<I>(I::DAD_ADDRESS)),
            device_id: FakeWeakDeviceId(FakeDeviceId),
            _marker: IpVersionMarker::new(),
        }
    }

    #[track_caller]
    fn check_multicast_groups(
        ip_version: IpVersion,
        groups: &HashMap<MulticastAddr<Ipv6Addr>, usize>,
    ) {
        match ip_version {
            // IPv4 should not join any multicast groups.
            IpVersion::V4 => assert_eq!(groups, &HashMap::new()),
            // IPv6 should join the solicited node multicast group.
            IpVersion::V6 => {
                assert_eq!(
                    groups,
                    &HashMap::from([(Ipv6::DAD_ADDRESS.to_solicited_node_address(), 1)])
                )
            }
        }
    }

    /// Verify the expected probe was sent while in [`DadState::Tentative`].
    fn check_probe_while_tentative<I: TestDadIpExt>(
        core_ctx: &FakeCoreCtxImpl<I>,
        bindings_ctx: &FakeBindingsCtxImpl<I>,
        frames_len: usize,
        dad_transmits_remaining: Option<NonZeroU16>,
    ) {
        let FakeDadContext { state, address_ctx, .. } = &core_ctx.state;
        let (got_transmits_remaining, ip_specific_state, probe_wait) = assert_matches!(
            state,
            DadState::Tentative {
                timer: _,
                dad_transmits_remaining,
                ip_specific_state,
                probe_wait,
            } => (dad_transmits_remaining, ip_specific_state, probe_wait)
        );
        assert_eq!(
            *got_transmits_remaining, dad_transmits_remaining,
            "got dad_transmits_remaining = {got_transmits_remaining:?},
                want dad_transmits_remaining = {dad_transmits_remaining:?}"
        );
        assert_eq!(probe_wait, &None);
        let FakeDadAddressContext { assigned, groups, .. } = &address_ctx.state;
        assert!(!*assigned);
        check_multicast_groups(I::VERSION, groups);

        let frames = core_ctx.frames();
        assert_eq!(frames.len(), frames_len, "frames = {:?}", frames);

        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Wrap<'a, I: TestDadIpExt> {
            meta: &'a I::SendData,
            ip_specific_state: &'a I::TentativeState,
        }

        let (meta, frame) = frames.last().expect("should have transmitted a frame");
        assert_eq!(&frame[..], EmptyBuf.as_ref());

        // Perform IP specific validation of the sent probe.
        I::map_ip::<_, ()>(
            Wrap { meta, ip_specific_state },
            |Wrap { meta: Ipv4DadSendData { target_ip, probe_type }, ip_specific_state: () }| {
                assert_eq!(*target_ip, Ipv4::DAD_ADDRESS.get());
                assert_eq!(*probe_type, Ipv4SentProbeType::Probe);
            },
            |Wrap {
                 meta: Ipv6DadSendData { dst_ip, message, nonce },
                 ip_specific_state:
                     Ipv6TentativeDadState {
                         nonces,
                         added_extra_transmits_after_detecting_looped_back_ns: _,
                     },
             }| {
                assert_eq!(*dst_ip, Ipv6::DAD_ADDRESS.to_solicited_node_address());
                assert_eq!(*message, NeighborSolicitation::new(Ipv6::DAD_ADDRESS.get()));
                assert!(nonces.contains(nonce), "should have stored nonce");
            },
        );

        bindings_ctx.timers.assert_timers_installed_range([(
            dad_timer_id(),
            I::expected_timer_range(bindings_ctx.now()),
        )]);
    }

    // Verify that the probe wait field is properly set after DAD initialization.
    fn check_probe_wait<I: TestDadIpExt>(core_ctx: &FakeDadContext<I>) {
        let FakeDadContext { state, .. } = core_ctx;
        let probe_wait = assert_matches!(
            state,
            DadState::Tentative { probe_wait, .. } => probe_wait
        );
        // IPv4 is expected to have a probe wait, while IPv6 should not.
        match I::VERSION {
            IpVersion::V4 => assert_matches!(probe_wait, Some(_)),
            IpVersion::V6 => assert_matches!(probe_wait, None),
        }
    }

    // Skip the probe wait period by triggering the next timer.
    fn skip_probe_wait<I: TestDadIpExt>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        bindings_ctx: &mut FakeBindingsCtxImpl<I>,
    ) {
        // IPv4 is expected to have scheduled a probe wait timer, while IPv6
        // should not.
        match I::VERSION {
            IpVersion::V4 => {
                assert_eq!(bindings_ctx.trigger_next_timer(core_ctx), Some(dad_timer_id()))
            }
            IpVersion::V6 => {}
        }
    }

    // Verify the expected announcement was sent while in [`DadState::Announcing`].
    fn check_announcement<I: TestDadIpExt>(
        core_ctx: &FakeCoreCtxImpl<I>,
        frames_len: usize,
        announcements_remaining: Option<NonZeroU16>,
    ) {
        let FakeDadContext { state, address_ctx, .. } = &core_ctx.state;
        match announcements_remaining {
            // If we have no more outstanding announcements, we should have
            // entered `DadState::Assigned`.
            None => assert_matches!(state, DadState::Assigned { ran_dad: true }),
            // Otherwise, we should still be in `DadState::Announcing`.
            Some(announcements_remaining) => {
                let state = assert_matches!(
                    state,
                    DadState::Announcing {
                        timer: _,
                        ip_specific_state,
                    } => ip_specific_state
                );
                #[derive(GenericOverIp)]
                #[generic_over_ip(I, Ip)]
                struct Wrap<'a, I: DadIpExt>(&'a I::AnnouncingState);
                let got_announcements_remaining = I::map_ip_in(
                    Wrap(state),
                    |Wrap(Ipv4AnnouncingDadState { announcements_remaining })| {
                        *announcements_remaining
                    },
                    |Wrap(never)| match *never {},
                );
                assert_eq!(
                    got_announcements_remaining, announcements_remaining,
                    "got announcements_remaining = {got_announcements_remaining:?},
                        want announcements_remaining = {announcements_remaining:?}"
                );
            }
        }

        let FakeDadAddressContext { assigned, .. } = &address_ctx.state;
        assert!(*assigned);

        let frames = core_ctx.frames();
        assert_eq!(frames.len(), frames_len, "frames = {:?}", frames);
        let (meta, frame) = frames.last().expect("should have transmitted a frame");
        assert_eq!(&frame[..], EmptyBuf.as_ref());

        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Wrap<'a, I: TestDadIpExt> {
            meta: &'a I::SendData,
        }

        // Perform IP specific validation of the sent probe.
        I::map_ip::<_, ()>(
            Wrap { meta },
            |Wrap { meta: Ipv4DadSendData { target_ip, probe_type } }| {
                assert_eq!(*target_ip, Ipv4::DAD_ADDRESS.get());
                assert_eq!(*probe_type, Ipv4SentProbeType::Announcement);
            },
            |_| unreachable!("DAD shouldn't send announcements for IPv6 addresses."),
        );
    }

    #[ip_test(I)]
    fn perform_dad<I: TestDadIpExt>() {
        const DAD_TRANSMITS_REQUIRED: u16 = 5;

        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtxImpl::with_state(FakeDadContext {
            state: DadState::Uninitialized,
            max_dad_transmits: NonZeroU16::new(DAD_TRANSMITS_REQUIRED),
            address_ctx: FakeAddressCtxImpl::with_state(FakeDadAddressContext::default()),
        }));
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        let address_id = get_address_id::<I>(I::DAD_ADDRESS);
        I::with_dad_handler(core_ctx, |core_ctx| {
            let start_dad = core_ctx.initialize_duplicate_address_detection(
                bindings_ctx,
                &FakeDeviceId,
                &address_id,
                into_state_change_event::<I>,
            );
            assert_eq!(
                bindings_ctx.take_events(),
                &[IpDeviceEvent::AddressStateChanged {
                    device: FakeDeviceId,
                    addr: I::DAD_ADDRESS.into(),
                    state: IpAddressState::Tentative,
                }][..]
            );
            let token = assert_matches!(start_dad, NeedsDad::Yes(token) => token);
            check_probe_wait::<I>(core_ctx.as_ref());
            core_ctx.start_duplicate_address_detection(bindings_ctx, token);
        });

        skip_probe_wait::<I>(core_ctx, bindings_ctx);

        for count in 0..=(DAD_TRANSMITS_REQUIRED - 1) {
            check_probe_while_tentative(
                core_ctx,
                bindings_ctx,
                usize::from(count + 1),
                NonZeroU16::new(DAD_TRANSMITS_REQUIRED - count - 1),
            );
            assert_eq!(bindings_ctx.trigger_next_timer(core_ctx), Some(dad_timer_id()));
        }

        // The address should be considered assigned once tentative phase
        // finishes.
        let FakeDadContext { address_ctx, .. } = &core_ctx.state;
        let FakeDadAddressContext { assigned, groups, .. } = &address_ctx.state;
        assert!(*assigned);
        check_multicast_groups(I::VERSION, groups);
        assert_eq!(
            bindings_ctx.take_events(),
            &[IpDeviceEvent::AddressStateChanged {
                device: FakeDeviceId,
                addr: I::DAD_ADDRESS.into(),
                state: IpAddressState::Assigned
            }][..]
        );

        for count in 1..=I::EXPECTED_NUM_ANNOUNCEMENTS {
            check_announcement(
                core_ctx,
                usize::from(DAD_TRANSMITS_REQUIRED + count),
                NonZeroU16::new(I::EXPECTED_NUM_ANNOUNCEMENTS - count),
            );
            // There should be a timer installed after all but the final
            // announcement.
            if count == I::EXPECTED_NUM_ANNOUNCEMENTS {
                bindings_ctx.timers.assert_no_timers_installed();
            } else {
                assert_eq!(bindings_ctx.trigger_next_timer(core_ctx), Some(dad_timer_id()));
            }
        }

        let FakeDadContext { state, address_ctx, .. } = &core_ctx.state;
        let FakeDadAddressContext { assigned, .. } = &address_ctx.state;
        assert_matches!(*state, DadState::Assigned { ran_dad: true });
        assert!(*assigned);
    }

    #[ip_test(I)]
    fn stop_dad_while_tentative<I: TestDadIpExt>() {
        const DAD_TRANSMITS_REQUIRED: u16 = 2;

        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            FakeCtx::with_core_ctx(FakeCoreCtxImpl::with_state(FakeDadContext {
                state: DadState::Uninitialized,
                max_dad_transmits: NonZeroU16::new(DAD_TRANSMITS_REQUIRED),
                address_ctx: FakeAddressCtxImpl::with_state(FakeDadAddressContext::default()),
            }));
        let address_id = get_address_id::<I>(I::DAD_ADDRESS);
        I::with_dad_handler(&mut core_ctx, |core_ctx| {
            let start_dad = core_ctx.initialize_duplicate_address_detection(
                &mut bindings_ctx,
                &FakeDeviceId,
                &address_id,
                into_state_change_event::<I>,
            );
            let token = assert_matches!(start_dad, NeedsDad::Yes(token) => token);
            core_ctx.start_duplicate_address_detection(&mut bindings_ctx, token);
        });

        skip_probe_wait::<I>(&mut core_ctx, &mut bindings_ctx);

        check_probe_while_tentative(
            &core_ctx,
            &bindings_ctx,
            1,
            NonZeroU16::new(DAD_TRANSMITS_REQUIRED - 1),
        );

        I::with_dad_handler(&mut core_ctx, |core_ctx| {
            core_ctx.stop_duplicate_address_detection(
                &mut bindings_ctx,
                &FakeDeviceId,
                &address_id,
            );
        });

        bindings_ctx.timers.assert_no_timers_installed();
        let FakeDadContext { state, address_ctx, .. } = &core_ctx.state;
        assert_matches!(*state, DadState::Uninitialized);
        let FakeDadAddressContext { assigned, groups, .. } = &address_ctx.state;
        assert!(!*assigned);
        assert_eq!(groups, &HashMap::new());
    }

    #[test]
    fn stop_dad_while_announcing() {
        // Verify that DAD can be properly canceled from the Announcing state.
        // Announcing is only supported for IPv4.
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            FakeCtx::with_core_ctx(FakeCoreCtxImpl::with_state(FakeDadContext {
                state: DadState::Uninitialized,
                max_dad_transmits: NonZeroU16::new(1),
                address_ctx: FakeAddressCtxImpl::with_state(FakeDadAddressContext::default()),
            }));
        let address_id = get_address_id::<Ipv4>(Ipv4::DAD_ADDRESS);

        // Initialize DAD and fully run through the Tentative state.
        let start_dad = core_ctx.initialize_duplicate_address_detection(
            &mut bindings_ctx,
            &FakeDeviceId,
            &address_id,
            into_state_change_event::<Ipv4>,
        );
        let token = assert_matches!(start_dad, NeedsDad::Yes(token) => token);
        core_ctx.start_duplicate_address_detection(&mut bindings_ctx, token);
        skip_probe_wait::<Ipv4>(&mut core_ctx, &mut bindings_ctx);
        check_probe_while_tentative(
            &core_ctx,
            &bindings_ctx,
            1,
            None, /* dad_transmits_remaining */
        );
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(dad_timer_id()));

        // Verify we're in the Announcing state.
        let FakeDadContext { state, address_ctx, .. } = &core_ctx.state;
        let FakeDadAddressContext { assigned, .. } = &address_ctx.state;
        assert_matches!(*state, DadState::Announcing { .. });
        bindings_ctx.timers.assert_timers_installed_range([(dad_timer_id(), ..)]);
        assert!(*assigned);

        // Stop DAD and verify everything was cleaned up properly.
        core_ctx.stop_duplicate_address_detection(&mut bindings_ctx, &FakeDeviceId, &address_id);
        bindings_ctx.timers.assert_no_timers_installed();
        let FakeDadContext { state, address_ctx, .. } = &core_ctx.state;
        let FakeDadAddressContext { assigned, .. } = &address_ctx.state;
        assert_matches!(*state, DadState::Uninitialized);
        assert!(!*assigned);
    }

    enum IncomingArpPacketCase {
        Unavailable,
        Assigned { ran_dad: bool, receive_data: Ipv4DadAddressInfo },
        Tentative,
    }

    #[test_case(IncomingArpPacketCase::Unavailable; "uninitialized")]
    #[test_case(IncomingArpPacketCase::Tentative; "tentative")]
    #[test_case(IncomingArpPacketCase::Assigned{
            ran_dad: true, receive_data: Ipv4DadAddressInfo::SourceAddr
        }; "assigned_ran_dad_source_addr")]
    #[test_case(IncomingArpPacketCase::Assigned {
            ran_dad: true, receive_data: Ipv4DadAddressInfo::TargetAddr
    }; "assigned_ran_dad_target_addr")]
    #[test_case(IncomingArpPacketCase::Assigned {
            ran_dad: false, receive_data: Ipv4DadAddressInfo::SourceAddr
    }; "assigned_skipped_dad_source_addr")]
    #[test_case(IncomingArpPacketCase::Assigned {
            ran_dad: false, receive_data: Ipv4DadAddressInfo::TargetAddr
    }; "assigned_skipped_dad_target_addr")]
    fn handle_incoming_arp_packet(case: IncomingArpPacketCase) {
        let mut ctx = FakeCtx::with_default_bindings_ctx(|bindings_ctx| {
            let dad_state = match &case {
                IncomingArpPacketCase::Unavailable => DadState::Uninitialized,
                IncomingArpPacketCase::Assigned { ran_dad, receive_data: _ } => {
                    DadState::Assigned { ran_dad: *ran_dad }
                }
                IncomingArpPacketCase::Tentative => DadState::Tentative {
                    dad_transmits_remaining: NonZeroU16::new(1),
                    timer: bindings_ctx.new_timer(dad_timer_id()),
                    ip_specific_state: Default::default(),
                    probe_wait: None,
                },
            };

            FakeCoreCtxImpl::with_state(FakeDadContext {
                state: dad_state,
                max_dad_transmits: NonZeroU16::new(1),
                address_ctx: FakeAddressCtxImpl::with_state(FakeDadAddressContext::default()),
            })
        });

        let (want_lookup_result, optional_receive_data) = match case {
            IncomingArpPacketCase::Unavailable => (DadIncomingPacketResult::Uninitialized, None),
            IncomingArpPacketCase::Tentative => {
                (DadIncomingPacketResult::Tentative { meta: () }, None)
            }
            IncomingArpPacketCase::Assigned { ran_dad, receive_data } => {
                let should_remove = match (ran_dad, &receive_data) {
                    (true, Ipv4DadAddressInfo::SourceAddr) => true,
                    _ => false,
                };
                (DadIncomingPacketResult::Assigned { should_remove }, Some(receive_data))
            }
        };

        let addr = get_address_id::<Ipv4>(Ipv4::DAD_ADDRESS);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        const ARBITRARY_RECEIVE_DATA: Ipv4DadAddressInfo = Ipv4DadAddressInfo::SourceAddr;
        assert_eq!(
            DadHandler::<Ipv4, _>::handle_incoming_packet(
                core_ctx,
                bindings_ctx,
                &FakeDeviceId,
                &addr,
                optional_receive_data.unwrap_or(ARBITRARY_RECEIVE_DATA),
            ),
            want_lookup_result
        );
    }

    #[test_case(true, None ; "assigned with no incoming nonce")]
    #[test_case(true, Some([1u8; MIN_NONCE_LENGTH]) ; "assigned with incoming nonce")]
    #[test_case(false, None ; "uninitialized with no incoming nonce")]
    #[test_case(false, Some([1u8; MIN_NONCE_LENGTH]) ; "uninitialized with incoming nonce")]
    fn handle_incoming_dad_neighbor_solicitation_while_not_tentative(
        assigned: bool,
        nonce: Option<OwnedNdpNonce>,
    ) {
        const MAX_DAD_TRANSMITS: u16 = 1;

        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtxImpl::with_state(FakeDadContext {
            state: if assigned {
                DadState::Assigned { ran_dad: true }
            } else {
                DadState::Uninitialized
            },
            max_dad_transmits: NonZeroU16::new(MAX_DAD_TRANSMITS),
            address_ctx: FakeAddressCtxImpl::with_state(FakeDadAddressContext::default()),
        }));
        let addr = get_address_id::<Ipv6>(Ipv6::DAD_ADDRESS);

        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;

        let want_lookup_result = if assigned {
            DadIncomingPacketResult::Assigned { should_remove: false }
        } else {
            DadIncomingPacketResult::Uninitialized
        };

        assert_eq!(
            DadHandler::<Ipv6, _>::handle_incoming_packet(
                core_ctx,
                bindings_ctx,
                &FakeDeviceId,
                &addr,
                nonce.as_ref().map(NdpNonce::from),
            ),
            want_lookup_result
        );
    }

    #[test_case(true ; "discards looped back NS")]
    #[test_case(false ; "acts on non-looped-back NS")]
    fn handle_incoming_dad_neighbor_solicitation_during_tentative(looped_back: bool) {
        const DAD_TRANSMITS_REQUIRED: u16 = 1;

        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtxImpl::with_state(FakeDadContext {
            state: DadState::Uninitialized,
            max_dad_transmits: NonZeroU16::new(DAD_TRANSMITS_REQUIRED),
            address_ctx: FakeAddressCtxImpl::with_state(FakeDadAddressContext::default()),
        }));
        let addr = get_address_id::<Ipv6>(Ipv6::DAD_ADDRESS);

        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        let address_id = get_address_id::<Ipv6>(Ipv6::DAD_ADDRESS);
        let start_dad = DadHandler::<Ipv6, _>::initialize_duplicate_address_detection(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            &address_id,
            into_state_change_event::<Ipv6>,
        );
        assert_eq!(
            bindings_ctx.take_events(),
            &[IpDeviceEvent::AddressStateChanged {
                device: FakeDeviceId,
                addr: Ipv6::DAD_ADDRESS.into(),
                state: IpAddressState::Tentative,
            }][..]
        );
        let token = assert_matches!(start_dad, NeedsDad::Yes(token) => token);
        DadHandler::<Ipv6, _>::start_duplicate_address_detection(core_ctx, bindings_ctx, token);

        check_probe_while_tentative(core_ctx, bindings_ctx, 1, None);

        let sent_nonce: OwnedNdpNonce = {
            let (Ipv6DadSendData { dst_ip: _, message: _, nonce }, _frame) =
                core_ctx.frames().last().expect("should have transmitted a frame");
            *nonce
        };

        let alternative_nonce = {
            let mut nonce = sent_nonce.clone();
            nonce[0] = nonce[0].wrapping_add(1);
            nonce
        };

        let incoming_nonce =
            NdpNonce::from(if looped_back { &sent_nonce } else { &alternative_nonce });

        let matched_nonce = assert_matches!(
            DadHandler::<Ipv6, _>::handle_incoming_packet(
                core_ctx,
                bindings_ctx,
                &FakeDeviceId,
                &addr,
                Some(incoming_nonce),
            ),
            DadIncomingPacketResult::Tentative {
                meta: Ipv6PacketResultMetadata {matched_nonce}
            } => matched_nonce
        );

        assert_eq!(matched_nonce, looped_back);

        let frames_len_before_extra_transmits = core_ctx.frames().len();
        assert_eq!(frames_len_before_extra_transmits, 1);

        let extra_dad_transmits_required =
            NonZero::new(if looped_back { DEFAULT_MAX_MULTICAST_SOLICIT.get() } else { 0 });

        let (dad_transmits_remaining, added_extra_transmits_after_detecting_looped_back_ns) = assert_matches!(
            &core_ctx.state.state,
            DadState::Tentative {
                dad_transmits_remaining,
                timer: _,
                ip_specific_state: Ipv6TentativeDadState {
                    nonces: _,
                    added_extra_transmits_after_detecting_looped_back_ns
                },
                probe_wait: None,
            } => (dad_transmits_remaining, added_extra_transmits_after_detecting_looped_back_ns),
            "DAD state should be Tentative"
        );

        assert_eq!(dad_transmits_remaining, &extra_dad_transmits_required);
        assert_eq!(added_extra_transmits_after_detecting_looped_back_ns, &matched_nonce);

        let extra_dad_transmits_required =
            extra_dad_transmits_required.map(|n| n.get()).unwrap_or(0);

        // The retransmit timer should have been kicked when we observed the matching nonce.
        assert_eq!(bindings_ctx.trigger_next_timer(core_ctx), Some(dad_timer_id()));

        // Even though we originally required only 1 DAD transmit, MAX_MULTICAST_SOLICIT more
        // should be required as a result of the looped back solicitation.
        for count in 0..extra_dad_transmits_required {
            check_probe_while_tentative(
                core_ctx,
                bindings_ctx,
                usize::from(count) + frames_len_before_extra_transmits + 1,
                NonZeroU16::new(extra_dad_transmits_required - count - 1),
            );
            assert_eq!(bindings_ctx.trigger_next_timer(core_ctx), Some(dad_timer_id()));
        }
        let FakeDadContext { state, address_ctx, .. } = &core_ctx.state;
        assert_matches!(*state, DadState::Assigned { ran_dad: true });
        let FakeDadAddressContext { assigned, groups, .. } = &address_ctx.state;
        assert!(*assigned);
        assert_eq!(groups, &HashMap::from([(Ipv6::DAD_ADDRESS.to_solicited_node_address(), 1)]));
        assert_eq!(
            bindings_ctx.take_events(),
            &[IpDeviceEvent::AddressStateChanged {
                device: FakeDeviceId,
                addr: Ipv6::DAD_ADDRESS.into(),
                state: IpAddressState::Assigned
            }][..]
        );
    }

    #[test]
    fn ipv4_dad_probe_interval_is_valid() {
        // Verify that the IPv4 Dad Probe delay is always valid with a handful
        // of different RNG seeds.
        FakeCryptoRng::with_fake_rngs(100, |mut bindings_ctx| {
            let duration = ipv4_dad_probe_interval(&mut bindings_ctx).get();
            assert!(IPV4_PROBE_RANGE.contains(&duration), "actual={duration:?}");
        })
    }

    #[test]
    fn ipv4_dad_probe_wait_is_valid() {
        // Verify that the IPv4 Dad Probe wait is always valid with a handful
        // of different RNG seeds.
        FakeCryptoRng::with_fake_rngs(100, |mut bindings_ctx| {
            let duration = ipv4_dad_probe_wait(&mut bindings_ctx);
            assert!(IPV4_PROBE_WAIT_RANGE.contains(&duration), "actual={duration:?}");
        })
    }

    #[ip_test(I)]
    fn dad_ignores_stale_timers<I: TestDadIpExt>() {
        const DAD_TRANSMITS_REQUIRED: u16 = 1;

        let mut ctx = FakeCtx::with_core_ctx(FakeCoreCtxImpl::with_state(FakeDadContext {
            state: DadState::Uninitialized,
            max_dad_transmits: NonZeroU16::new(DAD_TRANSMITS_REQUIRED),
            address_ctx: FakeAddressCtxImpl::with_state(FakeDadAddressContext::default()),
        }));
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        let address_id = get_address_id::<I>(I::DAD_ADDRESS);
        I::with_dad_handler(core_ctx, |core_ctx| {
            let start_dad = core_ctx.initialize_duplicate_address_detection(
                bindings_ctx,
                &FakeDeviceId,
                &address_id,
                into_state_change_event::<I>,
            );
            let token = assert_matches!(start_dad, NeedsDad::Yes(token) => token);
            core_ctx.start_duplicate_address_detection(bindings_ctx, token);
        });

        let old_timer = assert_matches!(
            &core_ctx.state.state,
            DadState::Tentative { timer, .. } => timer.clone()
        );

        // Restart DAD.
        I::with_dad_handler(core_ctx, |core_ctx| {
            core_ctx.stop_duplicate_address_detection(bindings_ctx, &FakeDeviceId, &address_id);
            let start_dad = core_ctx.initialize_duplicate_address_detection(
                bindings_ctx,
                &FakeDeviceId,
                &address_id,
                into_state_change_event::<I>,
            );
            let token = assert_matches!(start_dad, NeedsDad::Yes(token) => token);
            core_ctx.start_duplicate_address_detection(bindings_ctx, token);
        });

        skip_probe_wait::<I>(core_ctx, bindings_ctx);

        assert_matches!(core_ctx.state.state, DadState::Tentative { .. });
        let unique_timer_id = bindings_ctx.unique_timer_id(&old_timer);
        old_timer.dispatch_id.handle(core_ctx, bindings_ctx, unique_timer_id);
        assert_matches!(core_ctx.state.state, DadState::Tentative { .. });

        // For maximum paranoia, let's make sure firing the proper timer works
        // as we'd expect.
        let mut new_timer = assert_matches!(
            &core_ctx.state.state,
            DadState::Tentative { timer, .. } => timer.clone()
        );
        // Pretend we're firing this thing.
        assert_matches!(bindings_ctx.cancel_timer(&mut new_timer), Some(_));
        let unique_timer_id = bindings_ctx.unique_timer_id(&new_timer);
        new_timer.dispatch_id.handle(core_ctx, bindings_ctx, unique_timer_id);
        match I::VERSION {
            IpVersion::V4 => assert_matches!(core_ctx.state.state, DadState::Announcing { .. }),
            IpVersion::V6 => assert_matches!(core_ctx.state.state, DadState::Assigned { .. }),
        };
    }
}
