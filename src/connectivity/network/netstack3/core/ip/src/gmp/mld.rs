// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Multicast Listener Discovery (MLD).
//!
//! MLD is derived from version 2 of IPv4's Internet Group Management Protocol,
//! IGMPv2. One important difference to note is that MLD uses ICMPv6 (IP
//! Protocol 58) message types, rather than IGMP (IP Protocol 2) message types.

use core::time::Duration;

use log::{debug, error};
use net_declare::net_ip_v6;
use net_types::ip::{Ip, Ipv6, Ipv6Addr, Ipv6ReservedScope, Ipv6Scope, Ipv6SourceAddr};
use net_types::{
    LinkLocalAddress as _, LinkLocalUnicastAddr, MulticastAddr, ScopeableAddress, SpecifiedAddr,
    Witness,
};
use netstack3_base::{
    AnyDevice, Counter, DeviceIdContext, ErrorAndSerializer, HandleableTimer, Inspectable,
    InspectableValue, Inspector, InspectorExt, ResourceCounterContext, WeakDeviceIdentifier,
};
use netstack3_filter::{self as filter, DynTransportSerializer};
use packet::{InnerPacketBuilder, NestablePacketBuilder as _, NestableSerializer as _};
use packet_formats::icmp::mld::{
    MldPacket, Mldv1Body, Mldv1MessageBuilder, Mldv1MessageType, Mldv2QueryBody,
    Mldv2ReportMessageBuilder, MulticastListenerDone, MulticastListenerReport,
    MulticastListenerReportV2,
};
use packet_formats::icmp::{IcmpMessage, IcmpPacketBuilder, IcmpSenderZeroCode};
use packet_formats::ip::Ipv6Proto;
use packet_formats::ipv6::ext_hdrs::{
    ExtensionHeaderOptionAction, HopByHopOption, HopByHopOptionData,
};
use packet_formats::ipv6::{Ipv6PacketBuilder, Ipv6PacketBuilderWithHbhOptions};
use packet_formats::utils::NonZeroDuration;
use thiserror::Error;
use zerocopy::SplitByteSlice;

use crate::internal::base::{IpDeviceMtuContext, IpLayerHandler, IpPacketDestination};
use crate::internal::gmp::{
    self, GmpBindingsContext, GmpBindingsTypes, GmpContext, GmpContextInner, GmpEnabledGroup,
    GmpGroupState, GmpMode, GmpState, GmpStateContext, GmpStateRef, GmpTimerId, GmpTypeLayout,
    IpExt, MulticastGroupSet, NotAMemberErr,
};
use crate::internal::local_delivery::IpHeaderInfo;

/// The destination address for all MLDv2 reports.
///
/// Defined in [RFC 3376 section 5.2.14].
///
/// [RFC 3376 section 5.2.14]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.2.14
const ALL_MLDV2_CAPABLE_ROUTERS: MulticastAddr<Ipv6Addr> =
    unsafe { MulticastAddr::new_unchecked(net_ip_v6!("FF02::16")) };

/// The bindings types for MLD.
pub trait MldBindingsTypes: GmpBindingsTypes {}
impl<BT> MldBindingsTypes for BT where BT: GmpBindingsTypes {}

/// The bindings execution context for MLD.
pub(crate) trait MldBindingsContext: GmpBindingsContext {}
impl<BC> MldBindingsContext for BC where BC: GmpBindingsContext {}

/// Provides immutable access to MLD state.
pub trait MldStateContext<BT: MldBindingsTypes>:
    DeviceIdContext<AnyDevice> + MldContextMarker
{
    /// Calls the function with an immutable reference to the device's MLD
    /// state.
    fn with_mld_state<
        O,
        F: FnOnce(
            &MulticastGroupSet<Ipv6Addr, GmpGroupState<Ipv6, BT>>,
            &GmpState<Ipv6, MldTypeLayout, BT>,
        ) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// The execution context capable of sending frames for MLD.
pub trait MldSendContext<BT: MldBindingsTypes>:
    DeviceIdContext<AnyDevice>
    + IpLayerHandler<Ipv6, BT>
    + IpDeviceMtuContext<Ipv6>
    + MldContextMarker
    + ResourceCounterContext<Self::DeviceId, MldCounters>
{
    /// Gets the IPv6 link local address on `device`.
    fn get_ipv6_link_local_addr(
        &mut self,
        device: &Self::DeviceId,
    ) -> Option<LinkLocalUnicastAddr<Ipv6Addr>>;
}

/// A marker context for MLD traits to allow for GMP test fakes.
pub trait MldContextMarker {}

/// The execution context for the Multicast Listener Discovery (MLD) protocol.
pub trait MldContext<BT: MldBindingsTypes>:
    DeviceIdContext<AnyDevice> + MldContextMarker + ResourceCounterContext<Self::DeviceId, MldCounters>
{
    /// The inner context given to `with_mld_state_mut`.
    type SendContext<'a>: MldSendContext<BT, DeviceId = Self::DeviceId> + 'a;

    /// Calls the function with a mutable reference to the device's MLD state
    /// and whether or not MLD is enabled for the `device`.
    fn with_mld_state_mut<
        O,
        F: FnOnce(Self::SendContext<'_>, GmpStateRef<'_, Ipv6, MldTypeLayout, BT>) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// A handler for incoming MLD packets.
///
/// A blanket implementation is provided for all `C: MldContext`.
pub trait MldPacketHandler<BC, DeviceId> {
    /// Receive an MLD packet.
    fn receive_mld_packet<B: SplitByteSlice, H: IpHeaderInfo<Ipv6>>(
        &mut self,
        bindings_ctx: &mut BC,
        device: &DeviceId,
        src_ip: Ipv6SourceAddr,
        dst_ip: SpecifiedAddr<Ipv6Addr>,
        packet: MldPacket<B>,
        header_info: &H,
    );
}

fn receive_mld_packet<
    B: SplitByteSlice,
    H: IpHeaderInfo<Ipv6>,
    CC: MldContext<BC>,
    BC: MldBindingsContext,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    src_ip: Ipv6SourceAddr,
    packet: MldPacket<B>,
    header_info: &H,
) -> Result<(), MldError> {
    // MLDv2 Specifies that all received queries & reports with an invalid hop
    // limit (not equal to 1) should be dropped (See RFC 3810 Section 6.2 and
    // Section 7.4).
    //
    // MLDv1 does not specify the expected behavior when receiving a message
    // with an invalid hop limit, however it does specify that all senders of
    // MLDv1 messages must set the hop limit to 1 (See RFC 2710 Section 3). Our
    // interpretation of this is to drop MLDv1 messages without a hop limit of
    // 1, as any sender that generates them is violating the RFC.
    //
    // This could be considered a violation of the Robustness Principle, but a
    // it is our belief that a packet with a different hop limit is more likely
    // to be malicious than a poor implementation. Note that the same rationale
    // is applied to IGMP.
    if header_info.hop_limit() != MLD_IP_HOP_LIMIT {
        core_ctx.increment_both(device, |counters: &MldCounters| &counters.rx_err_bad_hop_limit);
        return Err(MldError::BadHopLimit { hop_limit: header_info.hop_limit() });
    }

    match packet {
        MldPacket::MulticastListenerQuery(msg) => {
            core_ctx.increment_both(device, |counters: &MldCounters| &counters.rx_mldv1_query);
            // From RFC 2710 section 5:
            //
            //  - To be valid, the Query message MUST come from a link-
            //  local IPv6 Source Address [...]
            if !src_ip.is_link_local() {
                core_ctx
                    .increment_both(device, |counters: &MldCounters| &counters.rx_err_bad_src_addr);
                return Err(MldError::BadSourceAddress { addr: src_ip.into_addr() });
            }
            gmp::v1::handle_query_message(core_ctx, bindings_ctx, device, msg.body())
                .map_err(Into::into)
        }
        MldPacket::MulticastListenerQueryV2(msg) => {
            core_ctx.increment_both(device, |counters: &MldCounters| &counters.rx_mldv2_query);
            // From RFC 3810 section 5.1.14:
            //
            // If a node (router or host) receives a Query message with
            // the IPv6 Source Address set to the unspecified address (::), or any
            // other address that is not a valid IPv6 link-local address, it MUST
            // silently discard the message.
            if !src_ip.is_link_local() {
                core_ctx
                    .increment_both(device, |counters: &MldCounters| &counters.rx_err_bad_src_addr);
                return Err(MldError::BadSourceAddress { addr: src_ip.into_addr() });
            }

            // From RFC 3810 section 6.2:
            //
            // Upon reception of an MLD message that contains a Query, the node
            // checks [...] and if the Router Alert option is present in the
            // Hop-By-Hop Options header of the IPv6 packet. If any of these
            // checks fails, the packet is dropped.
            if !header_info.router_alert() {
                core_ctx.increment_both(device, |counters: &MldCounters| {
                    &counters.rx_err_missing_router_alert
                });
                return Err(MldError::MissingRouterAlert);
            }

            gmp::v2::handle_query_message(core_ctx, bindings_ctx, device, msg.body())
                .map_err(Into::into)
        }
        MldPacket::MulticastListenerReport(msg) => {
            core_ctx.increment_both(device, |counters: &MldCounters| &counters.rx_mldv1_report);
            // From RFC 2710 section 5:
            //
            //  - To be valid, the Report message MUST come from a link-
            //   local IPv6 Source Address [...]
            //
            // However, RFC 3810 allows MLD reports to be sent from
            // unspecified addresses and we in fact send those, so we relax
            // to allow accepting from unspecified addresses as well.
            match src_ip {
                Ipv6SourceAddr::Unspecified => {}
                Ipv6SourceAddr::Unicast(src_ip) => {
                    if !src_ip.is_link_local() {
                        core_ctx.increment_both(device, |counters: &MldCounters| {
                            &counters.rx_err_bad_src_addr
                        });
                        return Err(MldError::BadSourceAddress { addr: src_ip.into_addr() });
                    }
                }
            }
            let addr = msg.body().group_addr;
            MulticastAddr::new(msg.body().group_addr).map_or(
                Err(MldError::NotAMember { addr }),
                |group_addr| {
                    gmp::v1::handle_report_message(core_ctx, bindings_ctx, device, group_addr)
                        .map_err(Into::into)
                },
            )
        }
        MldPacket::MulticastListenerReportV2(_) => {
            core_ctx.increment_both(device, |counters: &MldCounters| &counters.rx_mldv2_report);
            debug!("Hosts are not interested in MLDv2 report messages");
            Ok(())
        }
        MldPacket::MulticastListenerDone(_) => {
            core_ctx.increment_both(device, |counters: &MldCounters| &counters.rx_leave_group);
            debug!("Hosts are not interested in Done messages");
            Ok(())
        }
    }
}

impl<BC: MldBindingsContext, CC: MldContext<BC>> MldPacketHandler<BC, CC::DeviceId> for CC {
    fn receive_mld_packet<B: SplitByteSlice, H: IpHeaderInfo<Ipv6>>(
        &mut self,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        src_ip: Ipv6SourceAddr,
        _dst_ip: SpecifiedAddr<Ipv6Addr>,
        packet: MldPacket<B>,
        header_info: &H,
    ) {
        receive_mld_packet(self, bindings_ctx, device, src_ip, packet, header_info)
            .unwrap_or_else(|e| debug!("Error occurred when handling MLD message: {}", e));
    }
}

impl<B: SplitByteSlice> gmp::v1::QueryMessage<Ipv6> for Mldv1Body<B> {
    fn group_addr(&self) -> Ipv6Addr {
        self.group_addr
    }

    fn max_response_time(&self) -> Duration {
        self.max_response_delay()
    }
}

impl<B: SplitByteSlice> gmp::v2::QueryMessage<Ipv6> for Mldv2QueryBody<B> {
    fn as_v1(&self) -> impl gmp::v1::QueryMessage<Ipv6> + '_ {
        self.as_v1_query()
    }

    fn robustness_variable(&self) -> u8 {
        self.header().querier_robustness_variable()
    }

    fn query_interval(&self) -> Duration {
        self.header().querier_query_interval()
    }

    fn group_address(&self) -> Ipv6Addr {
        self.header().group_address()
    }

    fn max_response_time(&self) -> Duration {
        self.header().max_response_delay().into()
    }

    fn sources(&self) -> impl Iterator<Item = Ipv6Addr> + '_ {
        self.sources().iter().copied()
    }
}

/// The MLD mode controllable by the user.
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
#[allow(missing_docs)]
pub enum MldConfigMode {
    V1,
    V2,
}

impl IpExt for Ipv6 {
    type GmpProtoConfigMode = MldConfigMode;

    fn should_perform_gmp(group_addr: MulticastAddr<Ipv6Addr>) -> bool {
        // Per [RFC 3810 Section 6]:
        //
        // > No MLD messages are ever sent regarding neither the link-scope
        // > all-nodes multicast address, nor any multicast address of scope 0
        // > (reserved) or 1 (node-local).
        //
        // We abide by this requirement by not executing [`Actions`] on these
        // addresses. Executing [`Actions`] only produces externally-visible side
        // effects, and is not required to maintain the correctness of the MLD state
        // machines.
        //
        // [RFC 3810 Section 6]: https://tools.ietf.org/html/rfc3810#section-6
        group_addr != Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS
            && ![Ipv6Scope::Reserved(Ipv6ReservedScope::Scope0), Ipv6Scope::InterfaceLocal]
                .contains(&group_addr.scope())
    }
}

/// Newtype over [`GmpMode`] to tailor it to GMP.
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct MldMode(GmpMode);

impl From<MldMode> for GmpMode {
    fn from(MldMode(v): MldMode) -> Self {
        v
    }
}

// NB: This could be derived, but it feels better to have it called out
// explicitly in MLD.
impl Default for MldMode {
    fn default() -> Self {
        Self(GmpMode::V2)
    }
}

impl InspectableValue for MldMode {
    fn record<I: Inspector>(&self, name: &str, inspector: &mut I) {
        let Self(gmp_mode) = self;
        let v = match gmp_mode {
            GmpMode::V1 { compat: false } => "MLDv1(compat)",
            GmpMode::V1 { compat: true } => "MLDv1",
            GmpMode::V2 => "MLDv2",
        };
        inspector.record_str(name, v);
    }
}

/// Uninstantiable type marking a [`GmpState`] as having MLD types.
pub enum MldTypeLayout {}

impl<BT: MldBindingsTypes> GmpTypeLayout<Ipv6, BT> for MldTypeLayout {
    type Config = MldConfig;
    type ProtoMode = MldMode;
}

impl<BT: MldBindingsTypes, CC: MldStateContext<BT>> GmpStateContext<Ipv6, BT> for CC {
    type TypeLayout = MldTypeLayout;

    fn with_gmp_state<
        O,
        F: FnOnce(
            &MulticastGroupSet<Ipv6Addr, GmpGroupState<Ipv6, BT>>,
            &GmpState<Ipv6, MldTypeLayout, BT>,
        ) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.with_mld_state(device, cb)
    }
}

impl<BC: MldBindingsContext, CC: MldContext<BC>> GmpContext<Ipv6, BC> for CC {
    type TypeLayout = MldTypeLayout;
    type Inner<'a> = CC::SendContext<'a>;

    fn with_gmp_state_mut_and_ctx<
        O,
        F: FnOnce(Self::Inner<'_>, GmpStateRef<'_, Ipv6, Self::TypeLayout, BC>) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.with_mld_state_mut(device, cb)
    }
}

impl<BC: MldBindingsContext, CC: MldSendContext<BC>> GmpContextInner<Ipv6, BC> for CC {
    type TypeLayout = MldTypeLayout;
    fn send_message_v1(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        _cur_mode: &MldMode,
        group_addr: GmpEnabledGroup<Ipv6Addr>,
        msg_type: gmp::v1::GmpMessageType,
    ) {
        let group_addr = group_addr.into_multicast_addr();
        let result = match msg_type {
            gmp::v1::GmpMessageType::Report => {
                self.increment_both(device, |counters: &MldCounters| &counters.tx_mldv1_report);
                send_mld_v1_packet::<_, _>(
                    self,
                    bindings_ctx,
                    device,
                    group_addr,
                    MldMessage::ListenerReport { group_addr },
                )
            }
            gmp::v1::GmpMessageType::Leave => {
                self.increment_both(device, |counters: &MldCounters| &counters.tx_leave_group);
                send_mld_v1_packet::<_, _>(
                    self,
                    bindings_ctx,
                    device,
                    Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS,
                    MldMessage::ListenerDone { group_addr },
                )
            }
        };

        match result {
            Ok(()) => {}
            Err(err) => {
                self.increment_both(device, |counters: &MldCounters| &counters.tx_err);
                debug!(
                    "error sending MLD message ({msg_type:?}) on device {device:?} for group \
                {group_addr}: {err}",
                )
            }
        }
    }

    fn send_report_v2(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        groups: impl Iterator<Item: gmp::v2::VerifiedReportGroupRecord<Ipv6Addr> + Clone> + Clone,
    ) {
        let dst_ip = ALL_MLDV2_CAPABLE_ROUTERS;
        let (ipv6, icmp) =
            new_ip_and_icmp_builders(self, device, dst_ip, MulticastListenerReportV2);
        let header = ipv6.constraints().header_len() + icmp.constraints().header_len();
        let avail_len = usize::from(self.get_mtu(device)).saturating_sub(header);
        let reports = match Mldv2ReportMessageBuilder::new(groups).with_len_limits(avail_len) {
            Ok(msg) => msg,
            Err(e) => {
                self.increment_both(device, |counters: &MldCounters| &counters.tx_err);
                // Warn here, we don't quite have a good global guarantee of
                // minimal acceptable MTUs across both IPv4 and IPv6. This
                // should effectively not happen though.
                //
                // TODO(https://fxbug.dev/383355972): Consider an assertion here
                // instead.
                error!("MTU too small to send MLD reports: {e:?}");
                return;
            }
        };
        for report in reports {
            self.increment_both(device, |counters: &MldCounters| &counters.tx_mldv2_report);
            let mut report = report.into_serializer().wrap_in(icmp.clone());
            let report = DynTransportSerializer::new(&mut report);
            let destination = IpPacketDestination::Multicast(dst_ip);
            let ip_frame = report.wrap_in(ipv6.clone());
            IpLayerHandler::send_ip_frame(self, bindings_ctx, device, destination, ip_frame)
                .unwrap_or_else(|ErrorAndSerializer { error, .. }| {
                    self.increment_both(device, |counters: &MldCounters| &counters.tx_err);
                    debug!("failed to send MLDv2 report over {device:?}: {error:?}")
                });
        }
    }

    fn mode_update_from_v1_query<Q: gmp::v1::QueryMessage<Ipv6>>(
        &mut self,
        _bindings_ctx: &mut BC,
        _query: &Q,
        gmp_state: &GmpState<Ipv6, MldTypeLayout, BC>,
        _config: &MldConfig,
    ) -> MldMode {
        let MldMode(gmp) = &gmp_state.mode;
        MldMode(gmp.maybe_enter_v1_compat())
    }

    fn mode_to_config(MldMode(gmp_mode): &MldMode) -> MldConfigMode {
        match gmp_mode {
            GmpMode::V2 | GmpMode::V1 { compat: true } => MldConfigMode::V2,
            GmpMode::V1 { compat: false } => MldConfigMode::V1,
        }
    }

    fn config_to_mode(MldMode(cur_mode): &MldMode, config: MldConfigMode) -> MldMode {
        MldMode(match config {
            MldConfigMode::V1 => GmpMode::V1 { compat: false },
            MldConfigMode::V2 => match cur_mode {
                GmpMode::V1 { compat: true } => *cur_mode,
                GmpMode::V1 { compat: false } | GmpMode::V2 => GmpMode::V2,
            },
        })
    }

    fn mode_on_disable(MldMode(cur_mode): &MldMode) -> MldMode {
        MldMode(cur_mode.maybe_exit_v1_compat())
    }

    fn mode_on_exit_compat() -> MldMode {
        MldMode(GmpMode::V2)
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub(crate) enum MldError {
    /// The host is trying to operate on an group address of which the host is
    /// not a member.
    #[error("the host has not already been a member of the address: {}", addr)]
    NotAMember { addr: Ipv6Addr },
    /// Failed to send an IGMP packet.
    #[error("failed to send out an MLD packet to address: {}", addr)]
    SendFailure { addr: Ipv6Addr },
    /// Message ignored because of bad source address.
    #[error("bad source address: {}", addr)]
    BadSourceAddress { addr: Ipv6Addr },
    /// Message ignored because of the router alter option was not present
    #[error("router alert option not present")]
    MissingRouterAlert,
    /// Message ignored because of the hop limit was invalid.
    #[error("message with incorrect hop limit: {hop_limit}")]
    BadHopLimit { hop_limit: u8 },
    /// MLD is disabled
    #[error("MLD is disabled on interface")]
    Disabled,
}

impl From<NotAMemberErr<Ipv6>> for MldError {
    fn from(NotAMemberErr(addr): NotAMemberErr<Ipv6>) -> Self {
        Self::NotAMember { addr }
    }
}

impl From<gmp::v2::QueryError<Ipv6>> for MldError {
    fn from(err: gmp::v2::QueryError<Ipv6>) -> Self {
        match err {
            gmp::v2::QueryError::NotAMember(addr) => Self::NotAMember { addr },
            gmp::v2::QueryError::Disabled => Self::Disabled,
        }
    }
}

pub(crate) type MldResult<T> = Result<T, MldError>;

#[derive(Debug)]
pub struct MldConfig {
    unsolicited_report_interval: Duration,
    send_leave_anyway: bool,
}

/// The default value for `unsolicited_report_interval` [RFC 2710 Section 7.10]
///
/// [RFC 2710 Section 7.10]: https://tools.ietf.org/html/rfc2710#section-7.10
pub const MLD_DEFAULT_UNSOLICITED_REPORT_INTERVAL: Duration = Duration::from_secs(10);

impl Default for MldConfig {
    fn default() -> Self {
        MldConfig {
            unsolicited_report_interval: MLD_DEFAULT_UNSOLICITED_REPORT_INTERVAL,
            send_leave_anyway: false,
        }
    }
}

impl gmp::v1::ProtocolConfig for MldConfig {
    fn unsolicited_report_interval(&self) -> Duration {
        self.unsolicited_report_interval
    }

    fn send_leave_anyway(&self) -> bool {
        self.send_leave_anyway
    }

    fn get_max_resp_time(&self, resp_time: Duration) -> Option<NonZeroDuration> {
        NonZeroDuration::new(resp_time)
    }
}

impl gmp::v2::ProtocolConfig for MldConfig {
    fn query_response_interval(&self) -> NonZeroDuration {
        gmp::v2::DEFAULT_QUERY_RESPONSE_INTERVAL
    }

    fn unsolicited_report_interval(&self) -> NonZeroDuration {
        gmp::v2::DEFAULT_UNSOLICITED_REPORT_INTERVAL
    }
}

/// An MLD timer to delay the sending of a report.
#[derive(PartialEq, Eq, Clone, Copy, Debug, Hash)]
pub struct MldTimerId<D: WeakDeviceIdentifier>(GmpTimerId<Ipv6, D>);

impl<D: WeakDeviceIdentifier> MldTimerId<D> {
    pub(crate) fn device_id(&self) -> &D {
        let Self(this) = self;
        this.device_id()
    }

    /// Creates a new [`MldTimerId`] for a GMP delayed report on `device`.
    #[cfg(any(test, feature = "testutils"))]
    pub fn new(device: D) -> Self {
        Self(GmpTimerId { device, _marker: Default::default() })
    }
}

impl<D: WeakDeviceIdentifier> From<GmpTimerId<Ipv6, D>> for MldTimerId<D> {
    fn from(id: GmpTimerId<Ipv6, D>) -> MldTimerId<D> {
        MldTimerId(id)
    }
}

impl<BC: MldBindingsContext, CC: MldContext<BC>> HandleableTimer<CC, BC>
    for MldTimerId<CC::WeakDeviceId>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, _: BC::UniqueTimerId) {
        let Self(id) = self;
        gmp::handle_timer(core_ctx, bindings_ctx, id);
    }
}

/// An iterator that generates the IP options for MLD packets.
///
/// This allows us to write `new_ip_and_icmp_builders` easily without a big mess
/// of static lifetimes.
///
/// MLD messages require the Router Alert hop-by-hop extension. See [RFC 2710
/// section 3] , [RFC 3810 section 5].
///
/// [RFC 2710 section 2]:
///     https://datatracker.ietf.org/doc/html/rfc2710#section-3
/// [RFC 3810 section 5]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5
#[derive(Debug, Clone, Default)]
struct MldIpOptions(bool);

impl Iterator for MldIpOptions {
    type Item = HopByHopOption<'static>;

    fn next(&mut self) -> Option<Self::Item> {
        let Self(yielded) = self;
        if core::mem::replace(yielded, true) {
            None
        } else {
            Some(HopByHopOption {
                action: ExtensionHeaderOptionAction::SkipAndContinue,
                mutable: false,
                data: HopByHopOptionData::RouterAlert { data: 0 },
            })
        }
    }
}

/// The required IP TTL for MLD messages.
///
/// See [RFC 2710 section 3] , [RFC 3810 section 5].
///
/// [RFC 2710 section 2]:
///     https://datatracker.ietf.org/doc/html/rfc2710#section-3
/// [RFC 3810 section 5]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5
const MLD_IP_HOP_LIMIT: u8 = 1;

fn new_ip_and_icmp_builders<
    BC: MldBindingsContext,
    CC: MldSendContext<BC>,
    M: IcmpMessage<Ipv6, Code = IcmpSenderZeroCode> + filter::IcmpMessage<Ipv6>,
>(
    core_ctx: &mut CC,
    device: &CC::DeviceId,
    dst_ip: MulticastAddr<Ipv6Addr>,
    msg: M,
) -> (Ipv6PacketBuilderWithHbhOptions<'static, MldIpOptions>, IcmpPacketBuilder<Ipv6, M>) {
    // According to https://tools.ietf.org/html/rfc3590#section-4, if a valid
    // link-local address is not available for the device (e.g., one has not
    // been configured), the message is sent with the unspecified address (::)
    // as the IPv6 source address.
    //
    // TODO(https://fxbug.dev/42180878): Handle an IPv6 link-local address being
    // assigned when reports were sent with the unspecified source address.
    let src_ip =
        core_ctx.get_ipv6_link_local_addr(device).map_or(Ipv6::UNSPECIFIED_ADDRESS, |x| x.get());

    let ipv6 = Ipv6PacketBuilderWithHbhOptions::new(
        Ipv6PacketBuilder::new(src_ip, dst_ip.get(), MLD_IP_HOP_LIMIT, Ipv6Proto::Icmpv6),
        MldIpOptions::default(),
    )
    .unwrap();
    let icmp = IcmpPacketBuilder::new(src_ip, dst_ip.get(), IcmpSenderZeroCode, msg);
    (ipv6, icmp)
}

/// A type to allow implementing the required filtering traits on a concrete
/// subset of message types.
enum MldMessage {
    ListenerReport { group_addr: <MulticastListenerReport as Mldv1MessageType>::GroupAddr },
    ListenerDone { group_addr: <MulticastListenerDone as Mldv1MessageType>::GroupAddr },
}

/// Send an MLD packet.
///
/// The MLD packet being sent should have its `hop_limit` to be 1 and a
/// `RouterAlert` option in its Hop-by-Hop Options extensions header.
fn send_mld_v1_packet<BC: MldBindingsContext, CC: MldSendContext<BC>>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    dst_ip: MulticastAddr<Ipv6Addr>,
    msg: MldMessage,
) -> MldResult<()> {
    macro_rules! send {
        ($type:ty, $struct:expr, $group_addr:expr) => {{
            let (ipv6, icmp) = new_ip_and_icmp_builders(core_ctx, device, dst_ip, $struct);

            let mut message =
                Mldv1MessageBuilder::<$type>::new_with_max_resp_delay($group_addr, ())
                    .into_serializer()
                    .wrap_in(icmp);
            let message = DynTransportSerializer::new(&mut message);
            let ip_frame = message.wrap_in(ipv6);
            let destination = IpPacketDestination::Multicast(dst_ip);
            IpLayerHandler::send_ip_frame(core_ctx, bindings_ctx, &device, destination, ip_frame)
                .map_err(|_| MldError::SendFailure { addr: $group_addr.into() })
        }};
    }

    match msg {
        MldMessage::ListenerReport { group_addr } => {
            send!(MulticastListenerReport, MulticastListenerReport, group_addr)
        }
        MldMessage::ListenerDone { group_addr } => {
            send!(MulticastListenerDone, MulticastListenerDone, group_addr)
        }
    }
}

/// Statistics about MLD.
///
/// The counter type `C` is generic to facilitate testing.
#[derive(Default, Debug)]
#[cfg_attr(
    any(test, feature = "testutils"),
    derive(PartialEq, netstack3_macros::CounterCollection)
)]
pub struct MldCounters<C = Counter> {
    /// Count of MLDv1 queries received.
    rx_mldv1_query: C,
    /// Count of MLDv2 queries received.
    rx_mldv2_query: C,
    /// Count of MLDv1 reports received.
    rx_mldv1_report: C,
    /// Count of MLDv2 reports received.
    rx_mldv2_report: C,
    /// Count of Leave Group messages received.
    rx_leave_group: C,
    /// Count of MLD messages received with an invalid source address.
    rx_err_bad_src_addr: C,
    /// Count of MLD messages received with an invalid hop limit.
    rx_err_bad_hop_limit: C,
    /// Count of MLD messages received without the Router Alert option.
    rx_err_missing_router_alert: C,
    /// Count of MLDv1 reports sent.
    tx_mldv1_report: C,
    /// Count of MLDv2 reports sent.
    tx_mldv2_report: C,
    /// Count of Leave Group messages sent.
    tx_leave_group: C,
    /// Count of MLD messages that could not be sent.
    tx_err: C,
}

impl Inspectable for MldCounters {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Self {
            rx_mldv1_query,
            rx_mldv2_query,
            rx_mldv1_report,
            rx_mldv2_report,
            rx_leave_group,
            rx_err_bad_src_addr,
            rx_err_bad_hop_limit,
            rx_err_missing_router_alert,
            tx_mldv1_report,
            tx_mldv2_report,
            tx_leave_group,
            tx_err,
        } = self;
        inspector.record_child("Rx", |inspector| {
            inspector.record_counter("MLDv1Query", rx_mldv1_query);
            inspector.record_counter("MLDv2Query", rx_mldv2_query);
            inspector.record_counter("MLDv1Report", rx_mldv1_report);
            inspector.record_counter("MLDv2Report", rx_mldv2_report);
            inspector.record_counter("LeaveGroup", rx_leave_group);
            inspector.record_child("Errors", |inspector| {
                inspector.record_counter("BadSourceAddress", rx_err_bad_src_addr);
                inspector.record_counter("BadHopLimit", rx_err_bad_hop_limit);
                inspector.record_counter("MissingRouterAlert", rx_err_missing_router_alert);
            })
        });
        inspector.record_child("Tx", |inspector| {
            inspector.record_counter("MLDv1Report", tx_mldv1_report);
            inspector.record_counter("MLDv2Report", tx_mldv2_report);
            inspector.record_counter("LeaveGroup", tx_leave_group);
            inspector.record_child("Errors", |inspector| {
                inspector.record_counter("SendFailed", tx_err);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::RefCell;
    use packet::Serializer;

    use assert_matches::assert_matches;
    use net_types::ethernet::Mac;
    use net_types::ip::{IpVersionMarker, Mtu};
    use netstack3_base::testutil::{
        FakeDeviceId, FakeInstant, FakeTimerCtxExt, FakeWeakDeviceId, TestIpExt, assert_empty,
        new_rng, run_with_many_seeds,
    };
    use netstack3_base::{
        CounterCollection, CounterContext, CtxPair, InstantContext as _, IntoCoreTimerCtx,
        NetworkSerializationContext, NetworkSerializer, SendFrameContext,
    };
    use packet::{Buf, BufferMut, ParseBuffer};
    use packet_formats::gmp::GroupRecordType;
    use packet_formats::icmp::mld::{
        Mldv2QueryMessageBuilder, MulticastListenerQuery, MulticastListenerQueryV2,
    };
    use packet_formats::icmp::{IcmpParseArgs, Icmpv6MessageType, Icmpv6Packet};
    use packet_formats::ip::IpPacket;
    use packet_formats::ipv6::Ipv6Packet;
    use packet_formats::ipv6::ext_hdrs::Ipv6ExtensionHeader;

    use super::*;
    use crate::internal::base::{IpPacketDestination, IpSendFrameError, SendIpPacketMeta};
    use crate::internal::fragmentation::FragmentableIpSerializer;
    use crate::internal::gmp::{
        GmpEnabledGroup, GmpHandler as _, GmpState, GroupJoinResult, GroupLeaveResult,
    };

    /// Metadata for sending an MLD packet in an IP packet.
    #[derive(Debug, PartialEq)]
    pub(crate) struct MldFrameMetadata<D> {
        pub(crate) device: D,
        pub(crate) dst_ip: MulticastAddr<Ipv6Addr>,
    }

    impl<D> MldFrameMetadata<D> {
        fn new(device: D, dst_ip: MulticastAddr<Ipv6Addr>) -> MldFrameMetadata<D> {
            MldFrameMetadata { device, dst_ip }
        }
    }

    /// A fake [`MldContext`] that stores the [`MldInterface`] and an optional
    /// IPv6 link-local address that may be returned in calls to
    /// [`MldContext::get_ipv6_link_local_addr`].
    struct FakeMldCtx {
        shared: Rc<RefCell<Shared>>,
        mld_enabled: bool,
        ipv6_link_local: Option<LinkLocalUnicastAddr<Ipv6Addr>>,
        stack_wide_counters: MldCounters,
        device_specific_counters: MldCounters,
    }

    impl FakeMldCtx {
        fn gmp_state(&mut self) -> &mut GmpState<Ipv6, MldTypeLayout, FakeBindingsCtxImpl> {
            &mut Rc::get_mut(&mut self.shared).unwrap().get_mut().gmp_state
        }

        fn groups(
            &mut self,
        ) -> &mut MulticastGroupSet<Ipv6Addr, GmpGroupState<Ipv6, FakeBindingsCtxImpl>> {
            &mut Rc::get_mut(&mut self.shared).unwrap().get_mut().groups
        }
    }

    impl CounterContext<MldCounters> for FakeMldCtx {
        fn counters(&self) -> &MldCounters {
            &self.stack_wide_counters
        }
    }

    impl ResourceCounterContext<FakeDeviceId, MldCounters> for FakeMldCtx {
        fn per_resource_counters<'a>(&'a self, _device_id: &'a FakeDeviceId) -> &'a MldCounters {
            &self.device_specific_counters
        }
    }

    /// The parts of `FakeMldCtx` that are behind a RefCell, mocking a lock.
    struct Shared {
        groups: MulticastGroupSet<Ipv6Addr, GmpGroupState<Ipv6, FakeBindingsCtxImpl>>,
        gmp_state: GmpState<Ipv6, MldTypeLayout, FakeBindingsCtxImpl>,
        config: MldConfig,
    }

    /// Creates a new test context in MLDv1.
    ///
    /// A historical note: a number of tests were originally written when only
    /// MLDv1 was supported.
    fn new_mldv1_context() -> FakeCtxImpl {
        FakeCtxImpl::with_default_bindings_ctx(|bindings_ctx| {
            // We start with enabled true to make tests easier to write.
            let mld_enabled = true;
            FakeCoreCtxImpl::with_state(FakeMldCtx {
                shared: Rc::new(RefCell::new(Shared {
                    groups: MulticastGroupSet::default(),
                    gmp_state: GmpState::new_with_enabled_and_mode::<_, IntoCoreTimerCtx>(
                        bindings_ctx,
                        FakeWeakDeviceId(FakeDeviceId),
                        mld_enabled,
                        MldMode(GmpMode::V1 { compat: false }),
                    ),
                    config: Default::default(),
                })),
                mld_enabled,
                ipv6_link_local: None,
                stack_wide_counters: Default::default(),
                device_specific_counters: Default::default(),
            })
        })
    }

    type FakeCtxImpl = CtxPair<FakeCoreCtxImpl, FakeBindingsCtxImpl>;
    type FakeCoreCtxImpl = netstack3_base::testutil::FakeCoreCtx<
        FakeMldCtx,
        MldFrameMetadata<FakeDeviceId>,
        FakeDeviceId,
    >;
    type FakeBindingsCtxImpl = netstack3_base::testutil::FakeBindingsCtx<
        MldTimerId<FakeWeakDeviceId<FakeDeviceId>>,
        (),
        (),
        (),
    >;

    impl MldContextMarker for FakeCoreCtxImpl {}
    impl MldContextMarker for &'_ mut FakeCoreCtxImpl {}

    impl MldStateContext<FakeBindingsCtxImpl> for FakeCoreCtxImpl {
        fn with_mld_state<
            O,
            F: FnOnce(
                &MulticastGroupSet<Ipv6Addr, GmpGroupState<Ipv6, FakeBindingsCtxImpl>>,
                &GmpState<Ipv6, MldTypeLayout, FakeBindingsCtxImpl>,
            ) -> O,
        >(
            &mut self,
            &FakeDeviceId: &FakeDeviceId,
            cb: F,
        ) -> O {
            let state = self.state.shared.borrow();
            cb(&state.groups, &state.gmp_state)
        }
    }

    impl MldContext<FakeBindingsCtxImpl> for FakeCoreCtxImpl {
        type SendContext<'a> = &'a mut Self;
        fn with_mld_state_mut<
            O,
            F: FnOnce(
                Self::SendContext<'_>,
                GmpStateRef<'_, Ipv6, MldTypeLayout, FakeBindingsCtxImpl>,
            ) -> O,
        >(
            &mut self,
            &FakeDeviceId: &FakeDeviceId,
            cb: F,
        ) -> O {
            let FakeMldCtx { mld_enabled, shared, .. } = &mut self.state;
            let enabled = *mld_enabled;
            let shared = Rc::clone(shared);
            let mut shared = shared.borrow_mut();
            let Shared { gmp_state, groups, config } = &mut *shared;
            cb(self, GmpStateRef { enabled, groups, gmp: gmp_state, config })
        }
    }

    impl IpDeviceMtuContext<Ipv6> for &mut FakeCoreCtxImpl {
        fn get_mtu(&mut self, _device: &FakeDeviceId) -> Mtu {
            Ipv6::MINIMUM_LINK_MTU
        }
    }

    impl MldSendContext<FakeBindingsCtxImpl> for &mut FakeCoreCtxImpl {
        fn get_ipv6_link_local_addr(
            &mut self,
            _device: &FakeDeviceId,
        ) -> Option<LinkLocalUnicastAddr<Ipv6Addr>> {
            self.state.ipv6_link_local
        }
    }

    impl IpLayerHandler<Ipv6, FakeBindingsCtxImpl> for &mut FakeCoreCtxImpl {
        fn send_ip_packet_from_device<S>(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtxImpl,
            _meta: SendIpPacketMeta<
                Ipv6,
                &Self::DeviceId,
                Option<SpecifiedAddr<<Ipv6 as Ip>::Addr>>,
            >,
            _body: S,
        ) -> Result<(), IpSendFrameError<S>>
        where
            S: NetworkSerializer,
            S::Buffer: BufferMut,
        {
            unimplemented!();
        }

        fn send_ip_frame<S>(
            &mut self,
            bindings_ctx: &mut FakeBindingsCtxImpl,
            device: &Self::DeviceId,
            destination: IpPacketDestination<Ipv6, &Self::DeviceId>,
            body: S,
        ) -> Result<(), IpSendFrameError<S>>
        where
            S: FragmentableIpSerializer<Ipv6, Buffer: BufferMut> + netstack3_filter::IpPacket<Ipv6>,
        {
            let addr = match destination {
                IpPacketDestination::Multicast(addr) => addr,
                _ => panic!("destination is not multicast: {:?}", destination),
            };
            (*self)
                .send_frame(bindings_ctx, MldFrameMetadata::new(device.clone(), addr), body)
                .map_err(|e| e.err_into())
        }
    }

    impl CounterContext<MldCounters> for &mut FakeCoreCtxImpl {
        fn counters(&self) -> &MldCounters {
            <FakeCoreCtxImpl as CounterContext<MldCounters>>::counters(self)
        }
    }

    impl ResourceCounterContext<FakeDeviceId, MldCounters> for &mut FakeCoreCtxImpl {
        fn per_resource_counters<'a>(&'a self, device_id: &'a FakeDeviceId) -> &'a MldCounters {
            <
                FakeCoreCtxImpl as ResourceCounterContext<FakeDeviceId, MldCounters>
            >::per_resource_counters(self, device_id)
        }
    }

    type CounterExpectations = MldCounters<u64>;

    impl CounterExpectations {
        #[track_caller]
        fn assert_counters<CC: ResourceCounterContext<FakeDeviceId, MldCounters>>(
            &self,
            core_ctx: &mut CC,
        ) {
            assert_eq!(self, &core_ctx.counters().cast(), "stack-wide counter mismatch");
            assert_eq!(
                self,
                &core_ctx.per_resource_counters(&FakeDeviceId).cast(),
                "device-specific counter mismatch"
            );
        }
    }

    #[test]
    fn test_mld_immediate_report() {
        run_with_many_seeds(|seed| {
            // Most of the test surface is covered by the GMP implementation,
            // MLD specific part is mostly passthrough. This test case is here
            // because MLD allows a router to ask for report immediately, by
            // specifying the `MaxRespDelay` to be 0. If this is the case, the
            // host should send the report immediately instead of setting a
            // timer.
            let mut rng = new_rng(seed);
            let cfg = MldConfig::default();
            let (mut s, _actions) =
                gmp::v1::GmpStateMachine::join_group(&mut rng, FakeInstant::default(), false, &cfg);
            assert_eq!(
                s.query_received(&mut rng, Duration::from_secs(0), FakeInstant::default(), &cfg),
                gmp::v1::QueryReceivedActions::StopTimerAndSendReport,
            );
        });
    }

    const MY_IP: SpecifiedAddr<Ipv6Addr> = unsafe {
        SpecifiedAddr::new_unchecked(Ipv6Addr::from_bytes([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 192, 168, 0, 3,
        ]))
    };
    const MY_MAC: Mac = Mac::new([1, 2, 3, 4, 5, 6]);
    const ROUTER_MAC: Mac = Mac::new([6, 5, 4, 3, 2, 1]);
    const GROUP_ADDR: MulticastAddr<Ipv6Addr> = <Ipv6 as gmp::testutil::TestIpExt>::GROUP_ADDR1;
    const TIMER_ID: MldTimerId<FakeWeakDeviceId<FakeDeviceId>> = MldTimerId(GmpTimerId {
        device: FakeWeakDeviceId(FakeDeviceId),
        _marker: IpVersionMarker::new(),
    });

    struct FakeHeaderInfo {
        hop_limit: u8,
        router_alert: bool,
    }

    impl IpHeaderInfo<Ipv6> for FakeHeaderInfo {
        fn dscp_and_ecn(&self) -> packet_formats::ip::DscpAndEcn {
            unimplemented!()
        }
        fn hop_limit(&self) -> u8 {
            self.hop_limit
        }
        fn router_alert(&self) -> bool {
            self.router_alert
        }
        fn as_bytes(&self) -> [&[u8]; 2] {
            unimplemented!()
        }
    }

    const DEFAULT_HEADER_INFO: FakeHeaderInfo =
        FakeHeaderInfo { hop_limit: MLD_IP_HOP_LIMIT, router_alert: true };

    fn new_v1_query(resp_time: Duration, group_addr: MulticastAddr<Ipv6Addr>) -> Buf<Vec<u8>> {
        let router_addr: Ipv6Addr = ROUTER_MAC.to_ipv6_link_local().addr().get();
        IcmpPacketBuilder::<_, _>::new(
            router_addr,
            MY_IP,
            IcmpSenderZeroCode,
            MulticastListenerQuery,
        )
        .wrap_body(
            Mldv1MessageBuilder::<MulticastListenerQuery>::new_with_max_resp_delay(
                group_addr.get(),
                resp_time.try_into().unwrap(),
            )
            .into_serializer(),
        )
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b()
    }

    fn new_v1_report(group_addr: MulticastAddr<Ipv6Addr>) -> Buf<Vec<u8>> {
        let router_addr: Ipv6Addr = ROUTER_MAC.to_ipv6_link_local().addr().get();
        IcmpPacketBuilder::<_, _>::new(
            router_addr,
            MY_IP,
            IcmpSenderZeroCode,
            MulticastListenerReport,
        )
        .wrap_body(
            Mldv1MessageBuilder::<MulticastListenerReport>::new(group_addr).into_serializer(),
        )
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b()
    }

    fn new_v2_general_query() -> Buf<Vec<u8>> {
        let router_addr: Ipv6Addr = ROUTER_MAC.to_ipv6_link_local().addr().get();
        IcmpPacketBuilder::<_, _>::new(
            router_addr,
            MY_IP,
            IcmpSenderZeroCode,
            MulticastListenerQueryV2,
        )
        .wrap_body(
            Mldv2QueryMessageBuilder::new(
                Default::default(),
                None,
                false,
                Default::default(),
                Default::default(),
                core::iter::empty::<Ipv6Addr>(),
            )
            .into_serializer(),
        )
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b()
    }

    fn parse_mld_packet<B: ParseBuffer>(buffer: &mut B) -> MldPacket<&[u8]> {
        let router_addr: Ipv6Addr = ROUTER_MAC.to_ipv6_link_local().addr().get();
        match buffer
            .parse_with::<_, Icmpv6Packet<_>>(IcmpParseArgs::new(router_addr, MY_IP))
            .unwrap()
        {
            Icmpv6Packet::Mld(packet) => packet,
            _ => panic!("serialized icmpv6 message is not an mld message"),
        }
    }

    fn receive_mldv1_query(
        core_ctx: &mut FakeCoreCtxImpl,
        bindings_ctx: &mut FakeBindingsCtxImpl,
        resp_time: Duration,
        group_addr: MulticastAddr<Ipv6Addr>,
    ) {
        let router_addr: Ipv6Addr = ROUTER_MAC.to_ipv6_link_local().addr().get();
        let mut buffer = new_v1_query(resp_time, group_addr);
        let packet = parse_mld_packet(&mut buffer);
        core_ctx.receive_mld_packet(
            bindings_ctx,
            &FakeDeviceId,
            router_addr.try_into().unwrap(),
            MY_IP,
            packet,
            &DEFAULT_HEADER_INFO,
        )
    }

    fn receive_mldv1_report(
        core_ctx: &mut FakeCoreCtxImpl,
        bindings_ctx: &mut FakeBindingsCtxImpl,
        group_addr: MulticastAddr<Ipv6Addr>,
    ) {
        let router_addr: Ipv6Addr = ROUTER_MAC.to_ipv6_link_local().addr().get();
        let mut buffer = new_v1_report(group_addr);
        let packet = parse_mld_packet(&mut buffer);
        core_ctx.receive_mld_packet(
            bindings_ctx,
            &FakeDeviceId,
            router_addr.try_into().unwrap(),
            MY_IP,
            packet,
            &DEFAULT_HEADER_INFO,
        )
    }

    // Ensure the ttl is 1.
    fn ensure_ttl(frame: &[u8]) {
        assert_eq!(frame[7], MLD_IP_HOP_LIMIT);
    }

    fn ensure_slice_addr(frame: &[u8], start: usize, end: usize, ip: Ipv6Addr) {
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&frame[start..end]);
        assert_eq!(Ipv6Addr::from_bytes(bytes), ip);
    }

    // Ensure the destination address field in the ICMPv6 packet is correct.
    fn ensure_dst_addr(frame: &[u8], ip: Ipv6Addr) {
        ensure_slice_addr(frame, 24, 40, ip);
    }

    // Ensure the multicast address field in the MLD packet is correct.
    fn ensure_multicast_addr(frame: &[u8], ip: Ipv6Addr) {
        ensure_slice_addr(frame, 56, 72, ip);
    }

    // Ensure a sent frame meets the requirement.
    fn ensure_frame(
        frame: &[u8],
        op: u8,
        dst: MulticastAddr<Ipv6Addr>,
        multicast: MulticastAddr<Ipv6Addr>,
    ) {
        ensure_ttl(frame);
        assert_eq!(frame[48], op);
        // Ensure the length our payload is 32 = 8 (hbh_ext_hdr) + 24 (mld)
        assert_eq!(frame[5], 32);
        // Ensure the next header is our HopByHop Extension Header.
        assert_eq!(frame[6], 0);
        // Ensure there is a RouterAlert HopByHopOption in our sent frame
        assert_eq!(&frame[40..48], &[58, 0, 5, 2, 0, 0, 1, 0]);
        ensure_ttl(&frame[..]);
        ensure_dst_addr(&frame[..], dst.get());
        ensure_multicast_addr(&frame[..], multicast.get());
    }

    #[test]
    fn test_mld_simple_integration() {
        run_with_many_seeds(|seed| {
            let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
            bindings_ctx.seed_rng(seed);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );

            receive_mldv1_query(
                &mut core_ctx,
                &mut bindings_ctx,
                Duration::from_secs(10),
                GROUP_ADDR,
            );
            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(TIMER_ID));

            // We should get two MLD reports - one for the unsolicited one for
            // the host to turn into Delay Member state and the other one for
            // the timer being fired.
            assert_eq!(core_ctx.frames().len(), 2);
            // The frames are all reports.
            for (_, frame) in core_ctx.frames() {
                ensure_frame(&frame, 131, GROUP_ADDR, GROUP_ADDR);
                ensure_slice_addr(&frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
            }

            CounterExpectations { rx_mldv1_query: 1, tx_mldv1_report: 2, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_mld_immediate_query() {
        run_with_many_seeds(|seed| {
            let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
            bindings_ctx.seed_rng(seed);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            assert_eq!(core_ctx.frames().len(), 1);

            receive_mldv1_query(
                &mut core_ctx,
                &mut bindings_ctx,
                Duration::from_secs(0),
                GROUP_ADDR,
            );
            // The query says that it wants to hear from us immediately.
            assert_eq!(core_ctx.frames().len(), 2);
            // There should be no timers set.
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), None);
            // The frames are all reports.
            for (_, frame) in core_ctx.frames() {
                ensure_frame(&frame, 131, GROUP_ADDR, GROUP_ADDR);
                ensure_slice_addr(&frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
            }

            CounterExpectations { rx_mldv1_query: 1, tx_mldv1_report: 2, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_mld_integration_fallback_from_idle() {
        run_with_many_seeds(|seed| {
            let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
            bindings_ctx.seed_rng(seed);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            assert_eq!(core_ctx.frames().len(), 1);

            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(TIMER_ID));
            assert_eq!(core_ctx.frames().len(), 2);

            receive_mldv1_query(
                &mut core_ctx,
                &mut bindings_ctx,
                Duration::from_secs(10),
                GROUP_ADDR,
            );

            // We have received a query, hence we are falling back to Delay
            // Member state.
            let group_state = core_ctx.state.groups().get(&GROUP_ADDR).unwrap();
            match group_state.v1().get_inner() {
                gmp::v1::MemberState::Delaying(_) => {}
                _ => panic!("Wrong State!"),
            }

            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(TIMER_ID));
            assert_eq!(core_ctx.frames().len(), 3);
            // The frames are all reports.
            for (_, frame) in core_ctx.frames() {
                ensure_frame(&frame, 131, GROUP_ADDR, GROUP_ADDR);
                ensure_slice_addr(&frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
            }

            CounterExpectations { rx_mldv1_query: 1, tx_mldv1_report: 3, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_mld_integration_immediate_query_wont_fallback() {
        run_with_many_seeds(|seed| {
            let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
            bindings_ctx.seed_rng(seed);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            assert_eq!(core_ctx.frames().len(), 1);

            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(TIMER_ID));
            assert_eq!(core_ctx.frames().len(), 2);

            receive_mldv1_query(
                &mut core_ctx,
                &mut bindings_ctx,
                Duration::from_secs(0),
                GROUP_ADDR,
            );

            // Since it is an immediate query, we will send a report immediately
            // and turn into Idle state again.
            let group_state = core_ctx.state.groups().get(&GROUP_ADDR).unwrap();
            match group_state.v1().get_inner() {
                gmp::v1::MemberState::Idle(_) => {}
                _ => panic!("Wrong State!"),
            }

            // No timers!
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), None);
            assert_eq!(core_ctx.frames().len(), 3);
            // The frames are all reports.
            for (_, frame) in core_ctx.frames() {
                ensure_frame(&frame, 131, GROUP_ADDR, GROUP_ADDR);
                ensure_slice_addr(&frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
            }

            CounterExpectations { rx_mldv1_query: 1, tx_mldv1_report: 3, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_mld_integration_delay_reset_timer() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
        // This seed was carefully chosen to produce a substantial duration
        // value below.
        bindings_ctx.seed_rng(123456);
        assert_eq!(
            core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
            GroupJoinResult::Joined(())
        );

        core_ctx.state.gmp_state().timers.assert_timers([(
            gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
            (),
            FakeInstant::from(Duration::from_micros(590_354)),
        )]);
        let instant1 = bindings_ctx.timers.timers()[0].0.clone();
        let start = bindings_ctx.now();
        let duration = instant1 - start;

        receive_mldv1_query(&mut core_ctx, &mut bindings_ctx, duration, GROUP_ADDR);
        assert_eq!(core_ctx.frames().len(), 1);
        core_ctx.state.gmp_state().timers.assert_timers([(
            gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
            (),
            FakeInstant::from(Duration::from_micros(34_831)),
        )]);
        let instant2 = bindings_ctx.timers.timers()[0].0.clone();
        // This new timer should be sooner.
        assert!(instant2 <= instant1);
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(TIMER_ID));
        assert!(bindings_ctx.now() - start <= duration);
        assert_eq!(core_ctx.frames().len(), 2);
        // The frames are all reports.
        for (_, frame) in core_ctx.frames() {
            ensure_frame(&frame, 131, GROUP_ADDR, GROUP_ADDR);
            ensure_slice_addr(&frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
        }

        CounterExpectations { rx_mldv1_query: 1, tx_mldv1_report: 2, ..Default::default() }
            .assert_counters(&mut core_ctx);
    }

    #[test]
    fn test_mld_integration_last_send_leave() {
        run_with_many_seeds(|seed| {
            let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
            bindings_ctx.seed_rng(seed);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            let now = bindings_ctx.now();

            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                now..=(now + MLD_DEFAULT_UNSOLICITED_REPORT_INTERVAL),
            )]);
            // The initial unsolicited report.
            assert_eq!(core_ctx.frames().len(), 1);
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(TIMER_ID));
            // The report after the delay.
            assert_eq!(core_ctx.frames().len(), 2);
            assert_eq!(
                core_ctx.gmp_leave_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupLeaveResult::Left(())
            );
            // Our leave message.
            assert_eq!(core_ctx.frames().len(), 3);
            // The first two messages should be reports.
            ensure_frame(&core_ctx.frames()[0].1, 131, GROUP_ADDR, GROUP_ADDR);
            ensure_slice_addr(&core_ctx.frames()[0].1, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
            ensure_frame(&core_ctx.frames()[1].1, 131, GROUP_ADDR, GROUP_ADDR);
            ensure_slice_addr(&core_ctx.frames()[1].1, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
            // The last one should be the done message whose destination is all
            // routers.
            ensure_frame(
                &core_ctx.frames()[2].1,
                132,
                Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS,
                GROUP_ADDR,
            );
            ensure_slice_addr(&core_ctx.frames()[2].1, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);

            CounterExpectations { tx_mldv1_report: 2, tx_leave_group: 1, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_mld_integration_not_last_does_not_send_leave() {
        run_with_many_seeds(|seed| {
            let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
            bindings_ctx.seed_rng(seed);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            let now = bindings_ctx.now();
            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                now..=(now + MLD_DEFAULT_UNSOLICITED_REPORT_INTERVAL),
            )]);
            assert_eq!(core_ctx.frames().len(), 1);
            receive_mldv1_report(&mut core_ctx, &mut bindings_ctx, GROUP_ADDR);
            bindings_ctx.timers.assert_no_timers_installed();
            // The report should be discarded because we have received from someone
            // else.
            assert_eq!(core_ctx.frames().len(), 1);
            assert_eq!(
                core_ctx.gmp_leave_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupLeaveResult::Left(())
            );
            // A leave message is not sent.
            assert_eq!(core_ctx.frames().len(), 1);
            // The frames are all reports.
            for (_, frame) in core_ctx.frames() {
                ensure_frame(&frame, 131, GROUP_ADDR, GROUP_ADDR);
                ensure_slice_addr(&frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
            }

            CounterExpectations { rx_mldv1_report: 1, tx_mldv1_report: 1, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_mld_with_link_local() {
        run_with_many_seeds(|seed| {
            let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
            bindings_ctx.seed_rng(seed);

            core_ctx.state.ipv6_link_local = Some(MY_MAC.to_ipv6_link_local().addr());
            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(TIMER_ID));
            for (_, frame) in core_ctx.frames() {
                ensure_frame(&frame, 131, GROUP_ADDR, GROUP_ADDR);
                ensure_slice_addr(&frame, 8, 24, MY_MAC.to_ipv6_link_local().addr().get());
            }
        });
    }

    #[test]
    fn test_skip_mld() {
        run_with_many_seeds(|seed| {
            // Test that we do not perform MLD for addresses that we're supposed
            // to skip or when MLD is disabled.
            let test = |FakeCtxImpl { mut core_ctx, mut bindings_ctx }, group| {
                core_ctx.state.ipv6_link_local = Some(MY_MAC.to_ipv6_link_local().addr());

                // Assert that no observable effects have taken place.
                let assert_no_effect =
                    |core_ctx: &FakeCoreCtxImpl, bindings_ctx: &FakeBindingsCtxImpl| {
                        bindings_ctx.timers.assert_no_timers_installed();
                        assert_empty(core_ctx.frames());
                    };

                assert_eq!(
                    core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, group),
                    GroupJoinResult::Joined(())
                );
                // We should join the group but left in the GMP's non-member
                // state.
                assert_gmp_state!(core_ctx, &group, NonMember);
                assert_no_effect(&core_ctx, &bindings_ctx);

                receive_mldv1_report(&mut core_ctx, &mut bindings_ctx, group);
                // We should have done no state transitions/work.
                assert_gmp_state!(core_ctx, &group, NonMember);
                assert_no_effect(&core_ctx, &bindings_ctx);

                receive_mldv1_query(
                    &mut core_ctx,
                    &mut bindings_ctx,
                    Duration::from_secs(10),
                    group,
                );
                // We should have done no state transitions/work.
                assert_gmp_state!(core_ctx, &group, NonMember);
                assert_no_effect(&core_ctx, &bindings_ctx);

                assert_eq!(
                    core_ctx.gmp_leave_group(&mut bindings_ctx, &FakeDeviceId, group),
                    GroupLeaveResult::Left(())
                );
                // We should have left the group but not executed any `Actions`.
                assert!(core_ctx.state.groups().get(&group).is_none());
                assert_no_effect(&core_ctx, &bindings_ctx);

                CounterExpectations { rx_mldv1_report: 1, rx_mldv1_query: 1, ..Default::default() }
                    .assert_counters(&mut core_ctx);
            };

            let new_ctx = || {
                let mut ctx = new_mldv1_context();
                ctx.bindings_ctx.seed_rng(seed);
                ctx
            };

            // Test that we skip executing `Actions` for addresses we're
            // supposed to skip.
            test(new_ctx(), Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS);
            let mut bytes = Ipv6::MULTICAST_SUBNET.network().ipv6_bytes();
            // Manually set the "scope" field to 0.
            bytes[1] = bytes[1] & 0xF0;
            let reserved0 = MulticastAddr::new(Ipv6Addr::from_bytes(bytes)).unwrap();
            // Manually set the "scope" field to 1 (interface-local).
            bytes[1] = (bytes[1] & 0xF0) | 1;
            let iface_local = MulticastAddr::new(Ipv6Addr::from_bytes(bytes)).unwrap();
            test(new_ctx(), reserved0);
            test(new_ctx(), iface_local);

            // Test that we skip executing `Actions` when MLD is disabled on the
            // device.
            let mut ctx = new_ctx();
            ctx.core_ctx.state.mld_enabled = false;
            ctx.core_ctx.gmp_handle_disabled(&mut ctx.bindings_ctx, &FakeDeviceId);
            test(ctx, GROUP_ADDR);
        });
    }

    #[test]
    fn test_mld_integration_with_local_join_leave() {
        run_with_many_seeds(|seed| {
            // Simple MLD integration test to check that when we call top-level
            // multicast join and leave functions, MLD is performed.
            let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
            bindings_ctx.seed_rng(seed);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            assert_eq!(core_ctx.frames().len(), 1);
            let now = bindings_ctx.now();
            let range = now..=(now + MLD_DEFAULT_UNSOLICITED_REPORT_INTERVAL);

            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                range.clone(),
            )]);
            let frame = &core_ctx.frames().last().unwrap().1;
            ensure_frame(frame, 131, GROUP_ADDR, GROUP_ADDR);
            ensure_slice_addr(frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::AlreadyMember
            );
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            assert_eq!(core_ctx.frames().len(), 1);
            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                range.clone(),
            )]);

            assert_eq!(
                core_ctx.gmp_leave_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupLeaveResult::StillMember
            );
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            assert_eq!(core_ctx.frames().len(), 1);

            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                range,
            )]);

            assert_eq!(
                core_ctx.gmp_leave_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupLeaveResult::Left(())
            );
            assert_eq!(core_ctx.frames().len(), 2);
            bindings_ctx.timers.assert_no_timers_installed();
            let frame = &core_ctx.frames().last().unwrap().1;
            ensure_frame(frame, 132, Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS, GROUP_ADDR);
            ensure_slice_addr(frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);

            CounterExpectations { tx_mldv1_report: 1, tx_leave_group: 1, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_mld_enable_disable() {
        run_with_many_seeds(|seed| {
            let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
            bindings_ctx.seed_rng(seed);
            assert_eq!(core_ctx.take_frames(), []);

            // Should not perform MLD for the all-nodes address.
            //
            // As per RFC 3810 Section 6,
            //
            //   No MLD messages are ever sent regarding neither the link-scope,
            //   all-nodes multicast address, nor any multicast address of scope
            //   0 (reserved) or 1 (node-local).
            assert_eq!(
                core_ctx.gmp_join_group(
                    &mut bindings_ctx,
                    &FakeDeviceId,
                    Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS
                ),
                GroupJoinResult::Joined(())
            );
            assert_gmp_state!(core_ctx, &Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS, NonMember);
            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            {
                let frames = core_ctx.take_frames();
                let (MldFrameMetadata { device: FakeDeviceId, dst_ip }, frame) =
                    assert_matches!(&frames[..], [x] => x);
                assert_eq!(dst_ip, &GROUP_ADDR);
                ensure_frame(
                    frame,
                    Icmpv6MessageType::MulticastListenerReport.into(),
                    GROUP_ADDR,
                    GROUP_ADDR,
                );
                ensure_slice_addr(frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
            }

            // Should do nothing.
            core_ctx.gmp_handle_maybe_enabled(&mut bindings_ctx, &FakeDeviceId);
            assert_gmp_state!(core_ctx, &Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS, NonMember);
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            assert_eq!(core_ctx.take_frames(), []);

            // Should send done message.
            core_ctx.state.mld_enabled = false;
            core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);
            assert_gmp_state!(core_ctx, &Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS, NonMember);
            assert_gmp_state!(core_ctx, &GROUP_ADDR, NonMember);
            {
                let frames = core_ctx.take_frames();
                let (MldFrameMetadata { device: FakeDeviceId, dst_ip }, frame) =
                    assert_matches!(&frames[..], [x] => x);
                assert_eq!(dst_ip, &Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS);
                ensure_frame(
                    frame,
                    Icmpv6MessageType::MulticastListenerDone.into(),
                    Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS,
                    GROUP_ADDR,
                );
                ensure_slice_addr(frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);
            }

            // Should do nothing.
            core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);
            assert_gmp_state!(core_ctx, &Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS, NonMember);
            assert_gmp_state!(core_ctx, &GROUP_ADDR, NonMember);
            assert_eq!(core_ctx.take_frames(), []);

            // Should send report message.
            core_ctx.state.mld_enabled = true;
            core_ctx.gmp_handle_maybe_enabled(&mut bindings_ctx, &FakeDeviceId);
            assert_gmp_state!(core_ctx, &Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS, NonMember);
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            let frames = core_ctx.take_frames();
            let (MldFrameMetadata { device: FakeDeviceId, dst_ip }, frame) =
                assert_matches!(&frames[..], [x] => x);
            assert_eq!(dst_ip, &GROUP_ADDR);
            ensure_frame(
                frame,
                Icmpv6MessageType::MulticastListenerReport.into(),
                GROUP_ADDR,
                GROUP_ADDR,
            );
            ensure_slice_addr(frame, 8, 24, Ipv6::UNSPECIFIED_ADDRESS);

            CounterExpectations { tx_mldv1_report: 2, tx_leave_group: 1, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    /// Test the basics of MLDv2 report sending.
    #[test]
    fn send_gmpv2_report() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_mldv1_context();
        let sent_report_addr = Ipv6::get_multicast_addr(130);
        let sent_report_mode = GroupRecordType::ModeIsExclude;
        let sent_report_sources = Vec::<Ipv6Addr>::new();
        (&mut core_ctx).send_report_v2(
            &mut bindings_ctx,
            &FakeDeviceId,
            [gmp::v2::GroupRecord::new_with_sources(
                GmpEnabledGroup::new(sent_report_addr).unwrap(),
                sent_report_mode,
                sent_report_sources.iter(),
            )]
            .into_iter(),
        );
        let frames = core_ctx.take_frames();
        let (MldFrameMetadata { device: FakeDeviceId, dst_ip }, frame) =
            assert_matches!(&frames[..], [x] => x);
        assert_eq!(dst_ip, &ALL_MLDV2_CAPABLE_ROUTERS);
        let mut buff = &frame[..];
        let ipv6 = buff.parse::<Ipv6Packet<_>>().expect("parse IPv6");
        assert_eq!(ipv6.ttl(), MLD_IP_HOP_LIMIT);
        assert_eq!(ipv6.src_ip(), Ipv6::UNSPECIFIED_ADDRESS);
        assert_eq!(ipv6.dst_ip(), ALL_MLDV2_CAPABLE_ROUTERS.get());
        assert_eq!(ipv6.proto(), Ipv6Proto::Icmpv6);
        assert_eq!(
            ipv6.iter_extension_hdrs()
                .map(|h| {
                    let options = assert_matches!(
                        h,
                        Ipv6ExtensionHeader::HopByHopOptions { options } => options
                    );
                    assert_eq!(
                        options
                            .iter()
                            .map(|o| {
                                assert_matches!(
                                    o.data,
                                    HopByHopOptionData::RouterAlert { data: 0 }
                                );
                            })
                            .count(),
                        1
                    );
                })
                .count(),
            1
        );
        let args = IcmpParseArgs::new(ipv6.src_ip(), ipv6.dst_ip());
        let icmp = buff.parse_with::<_, Icmpv6Packet<_>>(args).expect("parse ICMPv6");
        let report = assert_matches!(
            icmp,
            Icmpv6Packet::Mld(MldPacket::MulticastListenerReportV2(report)) => report
        );
        let report = report
            .body()
            .iter_multicast_records()
            .map(|r| {
                (
                    r.header().multicast_addr().clone(),
                    r.header().record_type().unwrap(),
                    r.sources().to_vec(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(report, vec![(sent_report_addr.get(), sent_report_mode, sent_report_sources)]);

        CounterExpectations { tx_mldv2_report: 1, ..Default::default() }
            .assert_counters(&mut core_ctx);
    }

    #[test]
    fn v1_query_reject_bad_ipv6_source_addr() {
        let mut ctx = new_mldv1_context();
        let FakeCtxImpl { core_ctx, bindings_ctx } = &mut ctx;

        let buffer = new_v1_query(Duration::from_secs(1), GROUP_ADDR).into_inner();
        for addr in
            [Ipv6SourceAddr::Unspecified, Ipv6SourceAddr::new(net_ip_v6!("2001::1")).unwrap()]
        {
            let mut buffer = &buffer[..];
            let packet = parse_mld_packet(&mut buffer);
            assert_eq!(
                receive_mld_packet(
                    core_ctx,
                    bindings_ctx,
                    &FakeDeviceId,
                    addr,
                    packet,
                    &DEFAULT_HEADER_INFO,
                ),
                Err(MldError::BadSourceAddress { addr: addr.into_addr() })
            );
        }
        CounterExpectations { rx_mldv1_query: 2, rx_err_bad_src_addr: 2, ..Default::default() }
            .assert_counters(core_ctx);
    }

    #[test]
    fn v2_query_reject_bad_ipv6_source_addr() {
        let mut ctx = new_mldv1_context();
        let FakeCtxImpl { core_ctx, bindings_ctx } = &mut ctx;

        let buffer = new_v2_general_query().into_inner();
        for addr in
            [Ipv6SourceAddr::Unspecified, Ipv6SourceAddr::new(net_ip_v6!("2001::1")).unwrap()]
        {
            let mut buffer = &buffer[..];
            let packet = parse_mld_packet(&mut buffer);
            assert_eq!(
                receive_mld_packet(
                    core_ctx,
                    bindings_ctx,
                    &FakeDeviceId,
                    addr,
                    packet,
                    &DEFAULT_HEADER_INFO,
                ),
                Err(MldError::BadSourceAddress { addr: addr.into_addr() })
            );
        }

        CounterExpectations { rx_mldv2_query: 2, rx_err_bad_src_addr: 2, ..Default::default() }
            .assert_counters(core_ctx);
    }

    #[test]
    fn v1_report_reject_bad_ipv6_source_addr() {
        let mut ctx = new_mldv1_context();
        let FakeCtxImpl { core_ctx, bindings_ctx } = &mut ctx;

        assert_eq!(
            core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, GROUP_ADDR),
            GroupJoinResult::Joined(())
        );

        let buffer = new_v1_report(GROUP_ADDR).into_inner();
        let addr = Ipv6SourceAddr::new(net_ip_v6!("2001::1")).unwrap();
        let mut buffer = &buffer[..];
        let packet = parse_mld_packet(&mut buffer);
        assert_eq!(
            receive_mld_packet(
                core_ctx,
                bindings_ctx,
                &FakeDeviceId,
                addr,
                packet,
                &DEFAULT_HEADER_INFO,
            ),
            Err(MldError::BadSourceAddress { addr: addr.into_addr() })
        );

        // Unspecified is okay however.
        let buffer = new_v1_report(GROUP_ADDR).into_inner();
        let addr = Ipv6SourceAddr::Unspecified;
        let mut buffer = &buffer[..];
        let packet = parse_mld_packet(&mut buffer);
        assert_eq!(
            receive_mld_packet(
                core_ctx,
                bindings_ctx,
                &FakeDeviceId,
                addr,
                packet,
                &DEFAULT_HEADER_INFO,
            ),
            Ok(())
        );

        CounterExpectations {
            rx_mldv1_report: 2,
            rx_err_bad_src_addr: 1,
            tx_mldv1_report: 1,
            ..Default::default()
        }
        .assert_counters(core_ctx);
    }

    #[test]
    fn reject_bad_hop_limit() {
        let mut ctx = new_mldv1_context();
        let FakeCtxImpl { core_ctx, bindings_ctx } = &mut ctx;
        let src_addr: Ipv6Addr = ROUTER_MAC.to_ipv6_link_local().addr().get();
        let src_addr: Ipv6SourceAddr = src_addr.try_into().unwrap();

        let messages = [
            new_v1_query(Duration::from_secs(1), GROUP_ADDR).into_inner(),
            new_v2_general_query().into_inner(),
            new_v1_report(GROUP_ADDR).into_inner(),
        ];
        for buffer in messages {
            for hop_limit in [0, 2] {
                let header_info = FakeHeaderInfo { hop_limit, router_alert: true };
                let mut buffer = &buffer[..];
                let packet = parse_mld_packet(&mut buffer);
                assert_eq!(
                    receive_mld_packet(
                        core_ctx,
                        bindings_ctx,
                        &FakeDeviceId,
                        src_addr,
                        packet,
                        &header_info,
                    ),
                    Err(MldError::BadHopLimit { hop_limit })
                );
            }
        }
        CounterExpectations { rx_err_bad_hop_limit: 6, ..Default::default() }
            .assert_counters(core_ctx);
    }

    #[test]
    fn v2_query_reject_missing_router_alert() {
        let mut ctx = new_mldv1_context();
        let FakeCtxImpl { core_ctx, bindings_ctx } = &mut ctx;
        let src_addr: Ipv6Addr = ROUTER_MAC.to_ipv6_link_local().addr().get();
        let src_addr: Ipv6SourceAddr = src_addr.try_into().unwrap();

        let buffer = new_v2_general_query().into_inner();
        let header_info = FakeHeaderInfo { hop_limit: MLD_IP_HOP_LIMIT, router_alert: false };
        let mut buffer = &buffer[..];
        let packet = parse_mld_packet(&mut buffer);
        assert_eq!(
            receive_mld_packet(
                core_ctx,
                bindings_ctx,
                &FakeDeviceId,
                src_addr,
                packet,
                &header_info,
            ),
            Err(MldError::MissingRouterAlert),
        );
        CounterExpectations {
            rx_mldv2_query: 1,
            rx_err_missing_router_alert: 1,
            ..Default::default()
        }
        .assert_counters(core_ctx);
    }

    #[test]
    fn user_mode_change() {
        let mut ctx = new_mldv1_context();
        let FakeCtxImpl { core_ctx, bindings_ctx } = &mut ctx;
        assert_eq!(core_ctx.gmp_get_mode(&FakeDeviceId), MldConfigMode::V1);
        assert_eq!(
            core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, GROUP_ADDR),
            GroupJoinResult::Joined(())
        );
        // Ignore first reports.
        let _ = core_ctx.take_frames();
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, MldConfigMode::V2),
            MldConfigMode::V1
        );
        assert_eq!(core_ctx.gmp_get_mode(&FakeDeviceId), MldConfigMode::V2);
        assert_eq!(core_ctx.state.gmp_state().mode, MldMode(GmpMode::V2));
        // No side-effects.
        assert_eq!(core_ctx.take_frames(), Vec::new());

        // If we receive a v1 query, we'll go into compat mode but still report
        // v2 to the user.
        receive_mldv1_query(core_ctx, bindings_ctx, Duration::from_secs(0), GROUP_ADDR);
        assert_eq!(core_ctx.state.gmp_state().mode, MldMode(GmpMode::V1 { compat: true }));
        // Acknowledge query response.
        assert_eq!(core_ctx.take_frames().len(), 1);
        assert_eq!(core_ctx.gmp_get_mode(&FakeDeviceId), MldConfigMode::V2);

        // Even if user attempts to set V2 again we'll keep it in compat.
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, MldConfigMode::V2),
            MldConfigMode::V2
        );
        assert_eq!(core_ctx.take_frames(), Vec::new());
        assert_eq!(core_ctx.state.gmp_state().mode, MldMode(GmpMode::V1 { compat: true }));

        // Forcing V1 mode, however, exits compat.
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, MldConfigMode::V1),
            MldConfigMode::V2
        );
        assert_eq!(core_ctx.take_frames(), Vec::new());
        assert_eq!(core_ctx.state.gmp_state().mode, MldMode(GmpMode::V1 { compat: false }));
    }
}
