// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Internet Group Management Protocol, Version 2 (IGMPv2).
//!
//! IGMPv2 is a communications protocol used by hosts and adjacent routers on
//! IPv4 networks to establish multicast group memberships.

use core::fmt::Debug;
use core::time::Duration;

use log::{debug, error};
use net_declare::net_ip_v4;
use net_types::ip::{AddrSubnet, Ip as _, Ipv4, Ipv4Addr, Ipv4SourceAddr};
use net_types::{MulticastAddr, MulticastAddress as _, SpecifiedAddr, Witness};
use netstack3_base::{
    AnyDevice, Counter, DeviceIdContext, ErrorAndSerializer, HandleableTimer, Inspectable,
    InspectableValue, Inspector, InspectorExt, Instant, InstantContext, Ipv4DeviceAddr,
    ResourceCounterContext, WeakDeviceIdentifier,
};
use netstack3_filter::DynTransportSerializer;
use packet::{BufferMut, EmptyBuf, InnerPacketBuilder, NestablePacketBuilder as _};
use packet_formats::error::ParseError;
use packet_formats::igmp::messages::{
    IgmpLeaveGroup, IgmpMembershipQueryV2, IgmpMembershipQueryV3, IgmpMembershipReportV1,
    IgmpMembershipReportV2, IgmpMembershipReportV3Builder, IgmpPacket,
};
use packet_formats::igmp::{IgmpMessage, IgmpPacketBuilder, MessageType};
use packet_formats::ip::{DscpAndEcn, Ipv4Proto};
use packet_formats::ipv4::options::Ipv4Option;
use packet_formats::ipv4::{
    Ipv4OptionsTooLongError, Ipv4PacketBuilder, Ipv4PacketBuilderWithOptions,
};
use packet_formats::utils::NonZeroDuration;
use thiserror::Error;
use zerocopy::SplitByteSlice;

use crate::internal::base::{IpDeviceMtuContext, IpLayerHandler, IpPacketDestination};
use crate::internal::gmp::{
    self, GmpBindingsContext, GmpBindingsTypes, GmpContext, GmpContextInner, GmpEnabledGroup,
    GmpGroupState, GmpMode, GmpState, GmpStateContext, GmpStateRef, GmpTimerId, GmpTypeLayout,
    IpExt, MulticastGroupSet, NotAMemberErr, v2,
};
use crate::internal::local_delivery::{IpHeaderInfo, LocalDeliveryPacketInfo};

/// The destination address for all IGMPv3 reports.
///
/// Defined in [RFC 3376 section 4.2.14].
///
/// [RFC 3376 section 4.2.14]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4.2.14
const ALL_IGMPV3_CAPABLE_ROUTERS: MulticastAddr<Ipv4Addr> =
    unsafe { MulticastAddr::new_unchecked(net_ip_v4!("224.0.0.22")) };

/// The bindings types for IGMP.
pub trait IgmpBindingsTypes: GmpBindingsTypes {}
impl<BT> IgmpBindingsTypes for BT where BT: GmpBindingsTypes {}

/// The bindings execution context for IGMP.
pub trait IgmpBindingsContext: GmpBindingsContext + 'static {}
impl<BC> IgmpBindingsContext for BC where BC: GmpBindingsContext + 'static {}

/// The IGMP mode controllable by the user.
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
#[allow(missing_docs)]
pub enum IgmpConfigMode {
    V1,
    V2,
    V3,
}

/// A marker context for IGMP traits to allow for GMP test fakes.
pub trait IgmpContextMarker {}

/// Provides immutable access to IGMP state.
pub trait IgmpStateContext<BT: IgmpBindingsTypes>:
    DeviceIdContext<AnyDevice> + IgmpContextMarker
{
    /// Calls the function with an immutable reference to the device's IGMP
    /// state.
    fn with_igmp_state<
        O,
        F: FnOnce(
            &MulticastGroupSet<Ipv4Addr, GmpGroupState<Ipv4, BT>>,
            &GmpState<Ipv4, IgmpTypeLayout, BT>,
        ) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// The inner execution context for IGMP capable of sending packets.
pub trait IgmpSendContext<BT: IgmpBindingsTypes>:
    DeviceIdContext<AnyDevice>
    + IpLayerHandler<Ipv4, BT>
    + IpDeviceMtuContext<Ipv4>
    + ResourceCounterContext<Self::DeviceId, IgmpCounters>
{
    /// Gets an IP address and subnet associated with this device.
    fn get_ip_addr_subnet(
        &mut self,
        device: &Self::DeviceId,
    ) -> Option<AddrSubnet<Ipv4Addr, Ipv4DeviceAddr>>;
}

/// The execution context for the Internet Group Management Protocol (IGMP).
pub trait IgmpContext<BT: IgmpBindingsTypes>:
    DeviceIdContext<AnyDevice>
    + IgmpContextMarker
    + ResourceCounterContext<Self::DeviceId, IgmpCounters>
{
    /// The inner IGMP context capable of sending packets.
    type SendContext<'a>: IgmpSendContext<BT, DeviceId = Self::DeviceId> + 'a;

    /// Calls the function with a mutable reference to the device's IGMP state
    /// and whether or not IGMP is enabled for the `device`.
    fn with_igmp_state_mut<
        O,
        F: for<'a> FnOnce(Self::SendContext<'a>, GmpStateRef<'a, Ipv4, IgmpTypeLayout, BT>) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// A handler for incoming IGMP packets.
///
/// A blanket implementation is provided for all `C: IgmpContext`.
pub trait IgmpPacketHandler<BC, DeviceId> {
    /// Receive an IGMP message in an IP packet.
    fn receive_igmp_packet<B: BufferMut, H: IpHeaderInfo<Ipv4>>(
        &mut self,
        bindings_ctx: &mut BC,
        device: &DeviceId,
        src_ip: Ipv4SourceAddr,
        dst_ip: SpecifiedAddr<Ipv4Addr>,
        buffer: B,
        info: &LocalDeliveryPacketInfo<Ipv4, H>,
    );
}

impl<BC: IgmpBindingsContext, CC: IgmpContext<BC>> IgmpPacketHandler<BC, CC::DeviceId> for CC {
    fn receive_igmp_packet<B: BufferMut, H: IpHeaderInfo<Ipv4>>(
        &mut self,
        bindings_ctx: &mut BC,
        device: &CC::DeviceId,
        _src_ip: Ipv4SourceAddr,
        dst_ip: SpecifiedAddr<Ipv4Addr>,
        buffer: B,
        info: &LocalDeliveryPacketInfo<Ipv4, H>,
    ) {
        receive_igmp_packet(self, bindings_ctx, device, dst_ip, buffer, info).unwrap_or_else(|e| {
            debug!("Error occurred when handling IGMPv2 message: {}", e);
        });
    }
}

fn receive_igmp_packet<
    B: BufferMut,
    H: IpHeaderInfo<Ipv4>,
    CC: IgmpContext<BC>,
    BC: IgmpBindingsContext,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    dst_ip: SpecifiedAddr<Ipv4Addr>,
    mut buffer: B,
    info: &LocalDeliveryPacketInfo<Ipv4, H>,
) -> Result<(), IgmpError> {
    let LocalDeliveryPacketInfo { meta: _, header_info, marks: _ } = info;
    let dst_ip = dst_ip.into_addr();
    let ttl = header_info.hop_limit();

    // All RFCs define messages as being sent with a TTL of 1.
    //
    // Rejecting messages with bad TTL is almost a violation of the Robustness
    // Principle, but a packet with a different TTL is more likely to be
    // malicious than a poor implementation. Note that the same rationale is
    // applied to MLD.
    //
    // See RFC 1112 APPENDIX I, RFC 2236 section 2, and RFC 3376 section 4.
    if ttl != IGMP_IP_TTL {
        core_ctx.increment_both(device, |counters: &IgmpCounters| &counters.rx_err_bad_ttl);
        return Err(IgmpError::BadTtl(ttl));
    }

    let packet = match buffer.parse_with::<_, IgmpPacket<&[u8]>>(()) {
        Ok(packet) => packet,
        Err(e) => {
            core_ctx.increment_both(device, |counters: &IgmpCounters| &counters.rx_err_parse);
            return Err(IgmpError::Parse(e));
        }
    };

    match packet {
        IgmpPacket::MembershipQueryV2(msg) => {
            if msg.is_igmpv1_query() {
                core_ctx
                    .increment_both(device, |counters: &IgmpCounters| &counters.rx_igmpv1_query);
            } else {
                core_ctx
                    .increment_both(device, |counters: &IgmpCounters| &counters.rx_igmpv2_query);
            }
            // From RFC 3376 section 9.1:
            //
            // Hosts SHOULD ignore v1, v2 or v3 General Queries sent to a
            // multicast address other than 224.0.0.1, the all-systems address.
            if msg.group_addr() == Ipv4::UNSPECIFIED_ADDRESS
                && dst_ip.is_multicast()
                && dst_ip != *Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.as_ref()
            {
                core_ctx.increment_both(device, |counters: &IgmpCounters| {
                    &counters.rx_err_rejected_general_query
                });
                return Err(IgmpError::RejectedGeneralQuery { dst_ip });
            }
            // From RFC 3376 section 9.1:
            //
            // Hosts SHOULD ignore v2 or v3 Queries without the Router-Alert
            // option.
            if !msg.is_igmpv1_query() && !header_info.router_alert() {
                core_ctx.increment_both(device, |counters: &IgmpCounters| {
                    &counters.rx_err_missing_router_alert_in_query
                });
                return Err(IgmpError::MissingRouterAlertInQuery);
            }
            gmp::v1::handle_query_message(core_ctx, bindings_ctx, device, &msg).map_err(Into::into)
        }
        IgmpPacket::MembershipQueryV3(msg) => {
            core_ctx.increment_both(device, |counters: &IgmpCounters| &counters.rx_igmpv3_query);
            // From RFC 3376 section 9.1:
            //
            // Hosts SHOULD ignore v1, v2 or v3 General Queries sent to a
            // multicast address other than 224.0.0.1, the all-systems address.
            if msg.header().group_address() == Ipv4::UNSPECIFIED_ADDRESS
                && dst_ip.is_multicast()
                && dst_ip != *Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.as_ref()
            {
                core_ctx.increment_both(device, |counters: &IgmpCounters| {
                    &counters.rx_err_rejected_general_query
                });
                return Err(IgmpError::RejectedGeneralQuery { dst_ip });
            }
            // From RFC 3376 section 9.1:
            //
            // Hosts SHOULD ignore v2 or v3 Queries without the Router-Alert
            // option.
            if !header_info.router_alert() {
                core_ctx.increment_both(device, |counters: &IgmpCounters| {
                    &counters.rx_err_missing_router_alert_in_query
                });
                return Err(IgmpError::MissingRouterAlertInQuery);
            }

            gmp::v2::handle_query_message(core_ctx, bindings_ctx, device, &msg).map_err(Into::into)
        }
        IgmpPacket::MembershipReportV1(msg) => {
            core_ctx.increment_both(device, |counters: &IgmpCounters| &counters.rx_igmpv1_report);
            let addr = msg.group_addr();
            MulticastAddr::new(addr).map_or(Err(IgmpError::NotAMember { addr }), |group_addr| {
                gmp::v1::handle_report_message(core_ctx, bindings_ctx, device, group_addr)
                    .map_err(Into::into)
            })
        }
        IgmpPacket::MembershipReportV2(msg) => {
            core_ctx.increment_both(device, |counters: &IgmpCounters| &counters.rx_igmpv2_report);
            let addr = msg.group_addr();
            MulticastAddr::new(addr).map_or(Err(IgmpError::NotAMember { addr }), |group_addr| {
                gmp::v1::handle_report_message(core_ctx, bindings_ctx, device, group_addr)
                    .map_err(Into::into)
            })
        }
        IgmpPacket::LeaveGroup(_) => {
            core_ctx.increment_both(device, |counters: &IgmpCounters| &counters.rx_leave_group);
            debug!("Hosts are not interested in Leave Group messages");
            return Ok(());
        }
        IgmpPacket::MembershipReportV3(_) => {
            core_ctx.increment_both(device, |counters: &IgmpCounters| &counters.rx_igmpv3_report);
            debug!("Hosts are not interested in IGMPv3 report messages");
            return Ok(());
        }
    }
}

impl<B: SplitByteSlice> gmp::v1::QueryMessage<Ipv4> for IgmpMessage<B, IgmpMembershipQueryV2> {
    fn group_addr(&self) -> Ipv4Addr {
        self.group_addr()
    }

    fn max_response_time(&self) -> Duration {
        self.max_response_time().into()
    }
}

impl<B: SplitByteSlice> gmp::v2::QueryMessage<Ipv4> for IgmpMessage<B, IgmpMembershipQueryV3> {
    fn as_v1(&self) -> impl gmp::v1::QueryMessage<Ipv4> + '_ {
        self.as_v2_query()
    }

    fn robustness_variable(&self) -> u8 {
        self.header().querier_robustness_variable()
    }

    fn query_interval(&self) -> Duration {
        self.header().querier_query_interval()
    }

    fn group_address(&self) -> Ipv4Addr {
        self.header().group_address()
    }

    fn max_response_time(&self) -> Duration {
        self.max_response_time().into()
    }

    fn sources(&self) -> impl Iterator<Item = Ipv4Addr> + '_ {
        self.body().iter().copied()
    }
}

/// The IGMPv1 compatibility mode.
#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum IgmpV1Mode<I: Instant> {
    /// Forced IGMPv1 mode.
    Forced,
    /// IGMPv2 configuration in IGMPv1 compatibility.
    ///
    /// Sends v1 frames if `now` is before `until`.
    V2Compat { until: I },
    /// IGMPv3 configuration in IGMPv1 compatibility.
    ///
    /// Sends v1 frames if `now` is before `until`.
    V3Compat { until: I },
}

/// The IGMP compatibility mode.
#[derive(Eq, PartialEq, Copy, Clone, Debug, Default)]
pub enum IgmpMode<I: Instant> {
    /// Operating in IGMPv1 mode.
    V1(IgmpV1Mode<I>),
    /// Operating in IGMPv2 mode.
    ///
    /// If `compat` is `true` this is in compatibility mode and it'll eventually
    /// exit back to `V3` (controlled by GMP).
    V2 { compat: bool },
    /// Operating in IGMPv3 mode.
    #[default]
    V3,
}

impl<I: Instant> From<IgmpMode<I>> for GmpMode {
    fn from(value: IgmpMode<I>) -> Self {
        match value {
            IgmpMode::V1(v1) => {
                let compat = match v1 {
                    // GMP compat true means GMP expects to come back to GMPv2
                    // after a timer, which is false if our configured mode is
                    // IGMPv1 or IGMPv2.
                    IgmpV1Mode::Forced | IgmpV1Mode::V2Compat { .. } => false,
                    IgmpV1Mode::V3Compat { .. } => true,
                };
                Self::V1 { compat }
            }
            IgmpMode::V2 { compat } => Self::V1 { compat },
            IgmpMode::V3 => Self::V2,
        }
    }
}

impl<I: Instant> IgmpMode<I> {
    /// Returns `true` if we should send IGMPv1 messages at the current time
    /// given by `bindings_ctx`.
    fn should_send_v1<BC: InstantContext<Instant = I>>(&self, bindings_ctx: &mut BC) -> bool {
        match self {
            Self::V1(IgmpV1Mode::Forced) => true,
            Self::V1(IgmpV1Mode::V2Compat { until } | IgmpV1Mode::V3Compat { until }) => {
                bindings_ctx.now() < *until
            }
            Self::V2 { .. } | Self::V3 => false,
        }
    }
}

impl<I: Instant> InspectableValue for IgmpMode<I> {
    fn record<X: Inspector>(&self, name: &str, inspector: &mut X) {
        let v = match self {
            IgmpMode::V1(IgmpV1Mode::Forced) => "IGMPv1",
            IgmpMode::V1(IgmpV1Mode::V2Compat { .. }) => "IGMPv1(v2-compat)",
            IgmpMode::V1(IgmpV1Mode::V3Compat { .. }) => "IGMPv1(v3-compat)",
            IgmpMode::V2 { compat: true } => "IGMPv2(compat)",
            IgmpMode::V2 { compat: false } => "IGMPv2",
            IgmpMode::V3 => "IGMPv3",
        };
        inspector.record_str(name, v);
    }
}

impl IpExt for Ipv4 {
    type GmpProtoConfigMode = IgmpConfigMode;

    fn should_perform_gmp(addr: MulticastAddr<Ipv4Addr>) -> bool {
        // Per [RFC 2236 Section 6]:
        //
        //   The all-systems group (address 224.0.0.1) is handled as a special
        //   case.  The host starts in Idle Member state for that group on every
        //   interface, never transitions to another state, and never sends a
        //   report for that group.
        //
        // We abide by this requirement by not executing [`Actions`] on these
        // addresses. Executing [`Actions`] only produces externally-visible side
        // effects, and is not required to maintain the correctness of the MLD state
        // machines.
        //
        // [RFC 2236 Section 6]: https://datatracker.ietf.org/doc/html/rfc2236
        addr != Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS
    }
}

/// Uninstantiable type marking a [`GmpState`] as having IGMP types.
pub enum IgmpTypeLayout {}

impl<BT: IgmpBindingsTypes> GmpTypeLayout<Ipv4, BT> for IgmpTypeLayout {
    type Config = IgmpConfig;
    type ProtoMode = IgmpMode<BT::Instant>;
}

impl<BT: IgmpBindingsTypes, CC: IgmpStateContext<BT>> GmpStateContext<Ipv4, BT> for CC {
    type TypeLayout = IgmpTypeLayout;
    fn with_gmp_state<
        O,
        F: FnOnce(
            &MulticastGroupSet<Ipv4Addr, GmpGroupState<Ipv4, BT>>,
            &GmpState<Ipv4, IgmpTypeLayout, BT>,
        ) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.with_igmp_state(device, cb)
    }
}

impl<BC: IgmpBindingsContext, CC: IgmpContext<BC>> GmpContext<Ipv4, BC> for CC {
    type Inner<'a> = CC::SendContext<'a>;
    type TypeLayout = IgmpTypeLayout;

    fn with_gmp_state_mut_and_ctx<
        O,
        F: FnOnce(Self::Inner<'_>, GmpStateRef<'_, Ipv4, IgmpTypeLayout, BC>) -> O,
    >(
        &mut self,
        device: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.with_igmp_state_mut(device, cb)
    }
}

impl<CC, BC> GmpContextInner<Ipv4, BC> for CC
where
    CC: IgmpSendContext<BC>,
    BC: IgmpBindingsContext,
{
    type TypeLayout = IgmpTypeLayout;

    fn send_message_v1(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        cur_mode: &IgmpMode<BC::Instant>,
        group_addr: GmpEnabledGroup<Ipv4Addr>,
        msg_type: gmp::v1::GmpMessageType,
    ) {
        let group_addr = group_addr.into_multicast_addr();
        let result = match msg_type {
            gmp::v1::GmpMessageType::Report => {
                if cur_mode.should_send_v1(bindings_ctx) {
                    self.increment_both(device, |counters: &IgmpCounters| {
                        &counters.tx_igmpv1_report
                    });
                    send_igmp_v2_message::<_, _, IgmpMembershipReportV1>(
                        self,
                        bindings_ctx,
                        device,
                        group_addr,
                        group_addr,
                        (),
                    )
                } else {
                    self.increment_both(device, |counters: &IgmpCounters| {
                        &counters.tx_igmpv2_report
                    });
                    send_igmp_v2_message::<_, _, IgmpMembershipReportV2>(
                        self,
                        bindings_ctx,
                        device,
                        group_addr,
                        group_addr,
                        (),
                    )
                }
            }
            gmp::v1::GmpMessageType::Leave => {
                self.increment_both(device, |counters: &IgmpCounters| &counters.tx_leave_group);
                send_igmp_v2_message::<_, _, IgmpLeaveGroup>(
                    self,
                    bindings_ctx,
                    device,
                    group_addr,
                    Ipv4::ALL_ROUTERS_MULTICAST_ADDRESS,
                    (),
                )
            }
        };

        match result {
            Ok(()) => {}
            Err(err) => {
                self.increment_both(device, |counters: &IgmpCounters| &counters.tx_err);
                debug!(
                    "error sending IGMP message ({msg_type:?}) on device {device:?} for group \
                {group_addr}: {err}",
                )
            }
        }
    }

    fn send_report_v2(
        &mut self,
        bindings_ctx: &mut BC,
        device: &Self::DeviceId,
        groups: impl Iterator<Item: gmp::v2::VerifiedReportGroupRecord<Ipv4Addr> + Clone> + Clone,
    ) {
        let dst_ip = ALL_IGMPV3_CAPABLE_ROUTERS;
        let mut header = new_ip_header_builder(self, device, dst_ip);
        header.prefix_builder_mut().dscp_and_ecn(IGMPV3_DSCP_AND_ECN);
        let avail_len =
            usize::from(self.get_mtu(device)).saturating_sub(header.constraints().header_len());
        let reports = match IgmpMembershipReportV3Builder::new(groups).with_len_limits(avail_len) {
            Ok(msg) => msg,
            Err(e) => {
                // Warn here, we don't quite have a good global guarantee of
                // minimal acceptable MTUs across both IPv4 and IPv6. This
                // should effectively not happen though.
                //
                // TODO(https://fxbug.dev/383355972): Consider an assertion here
                // instead.
                error!("MTU too small to send IGMP reports: {e:?}");
                self.increment_both(device, |counters: &IgmpCounters| &counters.tx_err);
                return;
            }
        };
        for report in reports {
            self.increment_both(device, |counters: &IgmpCounters| &counters.tx_igmpv3_report);
            let destination = IpPacketDestination::Multicast(dst_ip);
            let mut report = report.into_serializer();
            let ip_frame = header.clone().wrap_body(DynTransportSerializer::new(&mut report));
            IpLayerHandler::send_ip_frame(self, bindings_ctx, device, destination, ip_frame)
                .unwrap_or_else(|ErrorAndSerializer { error, .. }| {
                    self.increment_both(device, |counters: &IgmpCounters| &counters.tx_err);
                    debug!("failed to send IGMPv3 report over {device:?}: {error:?}")
                });
        }
    }

    fn mode_update_from_v1_query<Q: gmp::v1::QueryMessage<Ipv4>>(
        &mut self,
        bindings_ctx: &mut BC,
        query: &Q,
        gmp_state: &GmpState<Ipv4, IgmpTypeLayout, BC>,
        config: &IgmpConfig,
    ) -> IgmpMode<BC::Instant> {
        // IGMPv2 hosts should be compatible with routers that only speak
        // IGMPv1. When an IGMPv2 host receives an IGMPv1 query (whose
        // `MaxRespCode` is 0), it should set up a timer and only respond with
        // IGMPv1 responses before the timer expires. Please refer to
        // https://tools.ietf.org/html/rfc2236#section-4 for details.

        // If this is an IGMPv2 query. Maintain any existing IGMPv1 mode
        // or forced IGMPv2, otherwise enter IGMPv2 compat.
        if query.max_response_time() != Duration::ZERO {
            return match gmp_state.mode {
                mode @ IgmpMode::V1(_) | mode @ IgmpMode::V2 { compat: false } => mode,
                IgmpMode::V2 { compat: true } | IgmpMode::V3 => IgmpMode::V2 { compat: true },
            };
        }

        // Otherwise this is an IGMPv1 query.
        match gmp_state.mode {
            mode @ IgmpMode::V1(IgmpV1Mode::Forced) => mode,
            IgmpMode::V2 { compat: false } | IgmpMode::V1(IgmpV1Mode::V2Compat { .. }) => {
                // From RFC 2236 section 4:
                //
                // This variable MUST be based upon whether or not an IGMPv1
                // query was heard in the last Version 1 Router Present Timeout
                // seconds, and MUST NOT be based upon the type of the last
                // Query heard.
                let duration = config.v1_router_present_timeout;
                IgmpMode::V1(IgmpV1Mode::V2Compat {
                    until: bindings_ctx.now().saturating_add(duration),
                })
            }
            IgmpMode::V3
            | IgmpMode::V2 { compat: true }
            | IgmpMode::V1(IgmpV1Mode::V3Compat { .. }) => {
                // From RFC 3376 section 4:
                //
                // The Group Compatibility Mode variable is based on whether an
                // older version report was heard in the last Older Version Host
                // Present Timeout seconds.
                let duration =
                    gmp_state.v2_proto.older_version_querier_present_timeout(config).get();
                IgmpMode::V1(IgmpV1Mode::V3Compat {
                    until: bindings_ctx.now().saturating_add(duration),
                })
            }
        }
    }

    fn mode_to_config(mode: &IgmpMode<BC::Instant>) -> IgmpConfigMode {
        match mode {
            IgmpMode::V1(IgmpV1Mode::Forced) => IgmpConfigMode::V1,
            IgmpMode::V1(IgmpV1Mode::V2Compat { .. }) | IgmpMode::V2 { compat: false } => {
                IgmpConfigMode::V2
            }
            IgmpMode::V1(IgmpV1Mode::V3Compat { .. })
            | IgmpMode::V2 { compat: true }
            | IgmpMode::V3 => IgmpConfigMode::V3,
        }
    }

    fn config_to_mode(
        cur_mode: &IgmpMode<BC::Instant>,
        config: IgmpConfigMode,
    ) -> IgmpMode<BC::Instant> {
        match config {
            IgmpConfigMode::V1 => IgmpMode::V1(IgmpV1Mode::Forced),
            IgmpConfigMode::V2 => match cur_mode {
                IgmpMode::V1(IgmpV1Mode::V2Compat { .. }) => {
                    // Switching from IGMPv2 to IGMPv2, copy current mode.
                    *cur_mode
                }
                IgmpMode::V1(IgmpV1Mode::V3Compat { until }) => {
                    // Switching from IGMPv3 to IGMPV2, copy the compatibility timeline.
                    IgmpMode::V1(IgmpV1Mode::V2Compat { until: *until })
                }
                IgmpMode::V1(IgmpV1Mode::Forced) | IgmpMode::V2 { .. } | IgmpMode::V3 => {
                    IgmpMode::V2 { compat: false }
                }
            },
            IgmpConfigMode::V3 => {
                match cur_mode {
                    IgmpMode::V1(IgmpV1Mode::V2Compat { until }) => {
                        // Switching from IGMPv2 to IGMPv3 just copy current mode.
                        IgmpMode::V1(IgmpV1Mode::V3Compat { until: *until })
                    }
                    IgmpMode::V1(IgmpV1Mode::V3Compat { .. }) | IgmpMode::V2 { compat: true } => {
                        // Switching from IGMPv3 to IGMPv3, copy current mode.
                        *cur_mode
                    }
                    IgmpMode::V1(IgmpV1Mode::Forced)
                    | IgmpMode::V2 { compat: false }
                    | IgmpMode::V3 => IgmpMode::V3,
                }
            }
        }
    }

    fn mode_on_disable(cur_mode: &IgmpMode<BC::Instant>) -> IgmpMode<BC::Instant> {
        match cur_mode {
            m @ IgmpMode::V1(IgmpV1Mode::Forced)
            | m @ IgmpMode::V2 { compat: false }
            | m @ IgmpMode::V3 => *m,
            IgmpMode::V1(IgmpV1Mode::V2Compat { .. }) => IgmpMode::V2 { compat: false },
            IgmpMode::V1(IgmpV1Mode::V3Compat { .. }) | IgmpMode::V2 { compat: true } => {
                IgmpMode::V3
            }
        }
    }

    fn mode_on_exit_compat() -> IgmpMode<BC::Instant> {
        IgmpMode::V3
    }
}

#[derive(Debug, Error)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) enum IgmpError {
    /// The host is trying to operate on an group address of which the host is
    /// not a member.
    #[error("the host has not already been a member of the address: {}", addr)]
    NotAMember { addr: Ipv4Addr },
    /// Failed to send an IGMP packet.
    #[error("failed to send out an IGMP packet to address: {}", addr)]
    SendFailure { addr: Ipv4Addr },
    /// IGMP is disabled
    #[error("IGMP is disabled on interface")]
    Disabled,
    #[error("failed to parse: {0}")]
    Parse(ParseError),
    #[error("message with incorrect ttl: {0}")]
    BadTtl(u8),
    #[error("rejected general query to {dst_ip}")]
    RejectedGeneralQuery { dst_ip: Ipv4Addr },
    #[error("router alert not present in query")]
    MissingRouterAlertInQuery,
}

impl From<NotAMemberErr<Ipv4>> for IgmpError {
    fn from(NotAMemberErr(addr): NotAMemberErr<Ipv4>) -> Self {
        Self::NotAMember { addr }
    }
}

impl From<v2::QueryError<Ipv4>> for IgmpError {
    fn from(err: v2::QueryError<Ipv4>) -> Self {
        match err {
            v2::QueryError::NotAMember(addr) => Self::NotAMember { addr },
            v2::QueryError::Disabled => Self::Disabled,
        }
    }
}

pub(crate) type IgmpResult<T> = Result<T, IgmpError>;

/// An IGMP timer ID.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct IgmpTimerId<D: WeakDeviceIdentifier>(GmpTimerId<Ipv4, D>);

impl<D: WeakDeviceIdentifier> IgmpTimerId<D> {
    pub(crate) fn device_id(&self) -> &D {
        let Self(inner) = self;
        inner.device_id()
    }

    /// Creates a new [`IgmpTimerId`] for `device`.
    #[cfg(any(test, feature = "testutils"))]
    pub const fn new(device: D) -> Self {
        Self(GmpTimerId::new(device))
    }
}

impl<D: WeakDeviceIdentifier> From<GmpTimerId<Ipv4, D>> for IgmpTimerId<D> {
    fn from(id: GmpTimerId<Ipv4, D>) -> IgmpTimerId<D> {
        Self(id)
    }
}

impl<BC: IgmpBindingsContext, CC: IgmpContext<BC>> HandleableTimer<CC, BC>
    for IgmpTimerId<CC::WeakDeviceId>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, _: BC::UniqueTimerId) {
        let Self(gmp) = self;
        gmp::handle_timer(core_ctx, bindings_ctx, gmp);
    }
}

/// An iterator that generates the IP options for IGMP packets.
///
/// This allows us to write `new_ip_header_builder` easily without a big mess of
/// static lifetimes.
///
/// IGMP messages require the Router Alert options. See [RFC 2236 section 2] ,
/// [RFC 3376 section 4].
///
/// [RFC 2236 section 2]:
///     https://datatracker.ietf.org/doc/html/rfc2236#section-2
/// [RFC 3376 section 4]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4
#[derive(Debug, Clone, Default)]
struct IgmpIpOptions(bool);

impl Iterator for IgmpIpOptions {
    type Item = Ipv4Option<'static>;

    fn next(&mut self) -> Option<Self::Item> {
        let Self(yielded) = self;
        if core::mem::replace(yielded, true) {
            None
        } else {
            Some(Ipv4Option::RouterAlert { data: 0 })
        }
    }
}

/// The required IP TTL for IGMP messages.
///
/// See [RFC 2236 section 2] , [RFC 3376 section 4].
///
/// [RFC 2236 section 2]:
///     https://datatracker.ietf.org/doc/html/rfc2236#section-2
/// [RFC 3376 section 4]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4
const IGMP_IP_TTL: u8 = 1;

/// The required IP DSCP and ECN for IGMPv3 messages.
///
/// [RFC 3376 section 4] defines the IP TOS (now DSCP and ECN) as having the
/// value 0xc0. So we construct the [`DscpAndEcn`] value from the value
/// specified in the RFC.
///
/// [RFC 3376 section 4]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4
const IGMPV3_DSCP_AND_ECN: DscpAndEcn = DscpAndEcn::new_with_raw(0xc0);

fn new_ip_header_builder<BC: IgmpBindingsContext, CC: IgmpSendContext<BC>>(
    core_ctx: &mut CC,
    device: &CC::DeviceId,
    dst_ip: MulticastAddr<Ipv4Addr>,
) -> Ipv4PacketBuilderWithOptions<'static, IgmpIpOptions> {
    // As per RFC 3376 section 4.2.13,
    //
    //   An IGMP report is sent with a valid IP source address for the
    //   destination subnet. The 0.0.0.0 source address may be used by a system
    //   that has not yet acquired an IP address.
    //
    // Note that RFC 3376 targets IGMPv3 but we could be running IGMPv2.
    // However, we still allow sending IGMP packets with the unspecified source
    // when no address is available so that IGMP snooping switches know to
    // forward multicast packets to us before an address is available. See RFC
    // 4541 for some details regarding considerations for IGMP/MLD snooping
    // switches.
    let src_ip =
        core_ctx.get_ip_addr_subnet(device).map_or(Ipv4::UNSPECIFIED_ADDRESS, |a| a.addr().get());
    Ipv4PacketBuilderWithOptions::new(
        Ipv4PacketBuilder::new(src_ip, dst_ip, IGMP_IP_TTL, Ipv4Proto::Igmp),
        IgmpIpOptions::default(),
    )
    .unwrap_or_else(|Ipv4OptionsTooLongError| unreachable!("router alert always fits"))
}

fn send_igmp_v2_message<BC: IgmpBindingsContext, CC: IgmpSendContext<BC>, M>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    group_addr: MulticastAddr<Ipv4Addr>,
    dst_ip: MulticastAddr<Ipv4Addr>,
    max_resp_time: M::MaxRespTime,
) -> IgmpResult<()>
where
    M: MessageType<EmptyBuf, FixedHeader = Ipv4Addr, VariableBody = ()>,
{
    let header = new_ip_header_builder(core_ctx, device, dst_ip);
    let mut body =
        IgmpPacketBuilder::<EmptyBuf, M>::new_with_resp_time(group_addr.get(), max_resp_time)
            .into_serializer();
    let ip_frame = header.wrap_body(DynTransportSerializer::new(&mut body));
    let destination = IpPacketDestination::Multicast(dst_ip);
    IpLayerHandler::send_ip_frame(core_ctx, bindings_ctx, &device, destination, ip_frame)
        .map_err(|_| IgmpError::SendFailure { addr: *group_addr })
}

#[derive(Debug)]
pub struct IgmpConfig {
    // When a host wants to send a report not because of a query, this value is
    // used as the delay timer.
    unsolicited_report_interval: Duration,
    // When this option is true, the host can send a leave message even when it
    // is not the last one in the multicast group.
    send_leave_anyway: bool,
    // Default timer value for Version 1 Router Present Timeout.
    v1_router_present_timeout: Duration,
}

/// The default value for `unsolicited_report_interval` as per [RFC 2236 Section
/// 8.10].
///
/// [RFC 2236 Section 8.10]: https://tools.ietf.org/html/rfc2236#section-8.10
pub const IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL: Duration = Duration::from_secs(10);
/// The default value for `v1_router_present_timeout` as per [RFC 2236 Section
/// 8.11].
///
/// [RFC 2236 Section 8.11]: https://tools.ietf.org/html/rfc2236#section-8.11
const DEFAULT_V1_ROUTER_PRESENT_TIMEOUT: Duration = Duration::from_secs(400);
/// The default value for the `MaxRespTime` if the query is a V1 query, whose
/// `MaxRespTime` field is 0 in the packet. Please refer to [RFC 2236 Section
/// 4].
///
/// [RFC 2236 Section 4]: https://tools.ietf.org/html/rfc2236#section-4
const DEFAULT_V1_QUERY_MAX_RESP_TIME: NonZeroDuration =
    NonZeroDuration::new(Duration::from_secs(10)).unwrap();

impl Default for IgmpConfig {
    fn default() -> Self {
        IgmpConfig {
            unsolicited_report_interval: IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL,
            send_leave_anyway: false,
            v1_router_present_timeout: DEFAULT_V1_ROUTER_PRESENT_TIMEOUT,
        }
    }
}

impl gmp::v1::ProtocolConfig for IgmpConfig {
    fn unsolicited_report_interval(&self) -> Duration {
        self.unsolicited_report_interval
    }

    fn send_leave_anyway(&self) -> bool {
        self.send_leave_anyway
    }

    fn get_max_resp_time(&self, resp_time: Duration) -> Option<NonZeroDuration> {
        // As per RFC 2236 section 4,
        //
        //   An IGMPv2 host may be placed on a subnet where the Querier router
        //   has not yet been upgraded to IGMPv2. The following requirements
        //   apply:
        //
        //        The IGMPv1 router will send General Queries with the Max
        //        Response Time set to 0.  This MUST be interpreted as a value
        //        of 100 (10 seconds).
        Some(NonZeroDuration::new(resp_time).unwrap_or(DEFAULT_V1_QUERY_MAX_RESP_TIME))
    }
}

impl gmp::v2::ProtocolConfig for IgmpConfig {
    fn query_response_interval(&self) -> NonZeroDuration {
        gmp::v2::DEFAULT_QUERY_RESPONSE_INTERVAL
    }

    fn unsolicited_report_interval(&self) -> NonZeroDuration {
        gmp::v2::DEFAULT_UNSOLICITED_REPORT_INTERVAL
    }
}

/// Statistics about IGMP.
///
/// The counter type `C` is generic to facilitate testing.
#[derive(Default, Debug)]
#[cfg_attr(
    any(test, feature = "testutils"),
    derive(PartialEq, netstack3_macros::CounterCollection)
)]
pub struct IgmpCounters<C = Counter> {
    // Count of IGMPv1 queries received.
    rx_igmpv1_query: C,
    // Count of IGMPv2 queries received.
    rx_igmpv2_query: C,
    // Count of IGMPv3 queries received.
    rx_igmpv3_query: C,
    // Count of IGMPv1 reports received.
    rx_igmpv1_report: C,
    // Count of IGMPv2 reports received.
    rx_igmpv2_report: C,
    // Count of IGMPv3 reports received.
    rx_igmpv3_report: C,
    // Count of Leave Group messages received.
    rx_leave_group: C,
    // The count of received IGMP messages that could not be parsed.
    rx_err_parse: C,
    // Count of queries that were rejected because the Router Alert option was
    // not present.
    rx_err_missing_router_alert_in_query: C,
    // Count of queries that were rejected because they weren't sent to the
    // all-systems address.
    rx_err_rejected_general_query: C,
    // Count of received IGMP messages that were rejected because they had
    // an invalid TTL.
    rx_err_bad_ttl: C,
    // Count of IGMPv1 reports sent.
    tx_igmpv1_report: C,
    // Count of IGMPv2 reports sent.
    tx_igmpv2_report: C,
    // Count of IGMPv3 reports sent.
    tx_igmpv3_report: C,
    // Count of Leave Group messages sent.
    tx_leave_group: C,
    // Count of IGMP messages that could not be sent.
    tx_err: C,
}

impl Inspectable for IgmpCounters {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Self {
            rx_igmpv1_query,
            rx_igmpv2_query,
            rx_igmpv3_query,
            rx_igmpv1_report,
            rx_igmpv2_report,
            rx_igmpv3_report,
            rx_leave_group,
            rx_err_parse,
            rx_err_missing_router_alert_in_query,
            rx_err_rejected_general_query,
            rx_err_bad_ttl,
            tx_igmpv1_report,
            tx_igmpv2_report,
            tx_igmpv3_report,
            tx_leave_group,
            tx_err,
        } = self;
        inspector.record_child("Rx", |inspector| {
            inspector.record_counter("IGMPv1Query", rx_igmpv1_query);
            inspector.record_counter("IGMPv2Query", rx_igmpv2_query);
            inspector.record_counter("IGMPv3Query", rx_igmpv3_query);
            inspector.record_counter("IGMPv1Report", rx_igmpv1_report);
            inspector.record_counter("IGMPv2Report", rx_igmpv2_report);
            inspector.record_counter("IGMPv3Report", rx_igmpv3_report);
            inspector.record_counter("LeaveGroup", rx_leave_group);
            inspector.record_child("Errors", |inspector| {
                inspector.record_counter("ParseFailed", rx_err_parse);
                inspector.record_counter(
                    "MissingRouterAlertInQuery",
                    rx_err_missing_router_alert_in_query,
                );
                inspector.record_counter("RejectedGeneralQuery", rx_err_rejected_general_query);
                inspector.record_counter("BadTTL", rx_err_bad_ttl);
            });
        });
        inspector.record_child("Tx", |inspector| {
            inspector.record_counter("IGMPv1Report", tx_igmpv1_report);
            inspector.record_counter("IGMPv2Report", tx_igmpv2_report);
            inspector.record_counter("IGMPv3Report", tx_igmpv3_report);
            inspector.record_counter("LeaveGroup", tx_leave_group);
            inspector.record_child("Errors", |inspector| {
                inspector.record_counter("SendFailed", tx_err);
            });
        })
    }
}

#[cfg(test)]
mod tests {
    use core::cell::RefCell;

    use alloc::rc::Rc;
    use alloc::vec;
    use alloc::vec::Vec;
    use assert_matches::assert_matches;

    use net_types::ip::{Ip, Mtu};
    use netstack3_base::testutil::{
        FakeDeviceId, FakeTimerCtxExt, FakeWeakDeviceId, TestIpExt as _, assert_empty,
        run_with_many_seeds,
    };
    use netstack3_base::{
        CounterCollection, CounterContext, CtxPair, IntoCoreTimerCtx, NetworkSerializationContext,
        NetworkSerializer, SendFrameContext as _,
    };
    use packet::serialize::Buf;
    use packet::{ParsablePacket as _, ParseBuffer, Serializer};
    use packet_formats::gmp::GroupRecordType;
    use packet_formats::igmp::IgmpResponseTimeV3;
    use packet_formats::igmp::messages::{
        IgmpMembershipQueryV2, IgmpMembershipQueryV3Builder, Igmpv3QQIC, Igmpv3QRV,
    };
    use packet_formats::ipv4::{Ipv4Header, Ipv4Packet};
    use packet_formats::testutil::parse_ip_packet;
    use test_case::test_case;

    use super::*;
    use crate::internal::base::{IpPacketDestination, IpSendFrameError, SendIpPacketMeta};
    use crate::internal::fragmentation::FragmentableIpSerializer;
    use crate::internal::gmp::{
        GmpEnabledGroup, GmpHandler as _, GmpState, GroupJoinResult, GroupLeaveResult,
    };
    use crate::testutil::FakeIpHeaderInfo;

    /// Metadata for sending an IGMP packet.
    #[derive(Debug, PartialEq)]
    pub(crate) struct IgmpPacketMetadata<D> {
        pub(crate) device: D,
        pub(crate) dst_ip: MulticastAddr<Ipv4Addr>,
    }

    impl<D> IgmpPacketMetadata<D> {
        fn new(device: D, dst_ip: MulticastAddr<Ipv4Addr>) -> IgmpPacketMetadata<D> {
            IgmpPacketMetadata { device, dst_ip }
        }
    }

    /// A fake [`IgmpContext`] that stores the [`MulticastGroupSet`] and an
    /// optional IPv4 address and subnet that may be returned in calls to
    /// [`IgmpContext::get_ip_addr_subnet`].
    struct FakeIgmpCtx {
        igmp_enabled: bool,
        shared: Rc<RefCell<Shared>>,
        addr_subnet: Option<AddrSubnet<Ipv4Addr, Ipv4DeviceAddr>>,
        stack_wide_counters: IgmpCounters,
        device_specific_counters: IgmpCounters,
    }

    /// The parts of `FakeIgmpCtx` that are behind a RefCell, mocking a lock.
    struct Shared {
        groups: MulticastGroupSet<Ipv4Addr, GmpGroupState<Ipv4, FakeBindingsCtx>>,
        gmp_state: GmpState<Ipv4, IgmpTypeLayout, FakeBindingsCtx>,
        config: IgmpConfig,
    }

    impl FakeIgmpCtx {
        fn gmp_state(&mut self) -> &mut GmpState<Ipv4, IgmpTypeLayout, FakeBindingsCtx> {
            &mut Rc::get_mut(&mut self.shared).unwrap().get_mut().gmp_state
        }

        fn gmp_state_and_config(
            &mut self,
        ) -> (&mut GmpState<Ipv4, IgmpTypeLayout, FakeBindingsCtx>, &mut IgmpConfig) {
            let shared = Rc::get_mut(&mut self.shared).unwrap().get_mut();
            (&mut shared.gmp_state, &mut shared.config)
        }

        fn groups(
            &mut self,
        ) -> &mut MulticastGroupSet<Ipv4Addr, GmpGroupState<Ipv4, FakeBindingsCtx>> {
            &mut Rc::get_mut(&mut self.shared).unwrap().get_mut().groups
        }
    }

    impl CounterContext<IgmpCounters> for FakeIgmpCtx {
        fn counters(&self) -> &IgmpCounters {
            &self.stack_wide_counters
        }
    }

    impl ResourceCounterContext<FakeDeviceId, IgmpCounters> for FakeIgmpCtx {
        fn per_resource_counters<'a>(&'a self, _device_id: &'a FakeDeviceId) -> &'a IgmpCounters {
            &self.device_specific_counters
        }
    }

    type FakeCtx = CtxPair<FakeCoreCtx, FakeBindingsCtx>;

    type FakeCoreCtx = netstack3_base::testutil::FakeCoreCtx<
        FakeIgmpCtx,
        IgmpPacketMetadata<FakeDeviceId>,
        FakeDeviceId,
    >;

    type FakeBindingsCtx = netstack3_base::testutil::FakeBindingsCtx<
        IgmpTimerId<FakeWeakDeviceId<FakeDeviceId>>,
        (),
        (),
        (),
    >;

    impl IgmpContextMarker for FakeCoreCtx {}

    impl IgmpStateContext<FakeBindingsCtx> for FakeCoreCtx {
        fn with_igmp_state<
            O,
            F: FnOnce(
                &MulticastGroupSet<Ipv4Addr, GmpGroupState<Ipv4, FakeBindingsCtx>>,
                &GmpState<Ipv4, IgmpTypeLayout, FakeBindingsCtx>,
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

    impl IgmpContext<FakeBindingsCtx> for FakeCoreCtx {
        type SendContext<'a> = &'a mut Self;
        fn with_igmp_state_mut<
            O,
            F: for<'a> FnOnce(
                Self::SendContext<'a>,
                GmpStateRef<'a, Ipv4, IgmpTypeLayout, FakeBindingsCtx>,
            ) -> O,
        >(
            &mut self,
            &FakeDeviceId: &FakeDeviceId,
            cb: F,
        ) -> O {
            let FakeIgmpCtx { igmp_enabled, shared, .. } = &mut self.state;
            let enabled = *igmp_enabled;
            let shared = Rc::clone(shared);
            let mut shared = shared.borrow_mut();
            let Shared { gmp_state, groups, config } = &mut *shared;
            cb(self, GmpStateRef { enabled, groups, gmp: gmp_state, config })
        }
    }

    impl IgmpSendContext<FakeBindingsCtx> for &mut FakeCoreCtx {
        fn get_ip_addr_subnet(
            &mut self,
            _device: &FakeDeviceId,
        ) -> Option<AddrSubnet<Ipv4Addr, Ipv4DeviceAddr>> {
            self.state.addr_subnet
        }
    }

    impl IpDeviceMtuContext<Ipv4> for &mut FakeCoreCtx {
        fn get_mtu(&mut self, _device: &FakeDeviceId) -> Mtu {
            Mtu::new(1500)
        }
    }

    impl IpLayerHandler<Ipv4, FakeBindingsCtx> for &mut FakeCoreCtx {
        fn send_ip_packet_from_device<S>(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtx,
            _meta: SendIpPacketMeta<
                Ipv4,
                &Self::DeviceId,
                Option<SpecifiedAddr<<Ipv4 as Ip>::Addr>>,
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
            bindings_ctx: &mut FakeBindingsCtx,
            device: &Self::DeviceId,
            destination: IpPacketDestination<Ipv4, &Self::DeviceId>,
            body: S,
        ) -> Result<(), IpSendFrameError<S>>
        where
            S: FragmentableIpSerializer<Ipv4, Buffer: BufferMut> + netstack3_filter::IpPacket<Ipv4>,
        {
            let addr = match destination {
                IpPacketDestination::Multicast(addr) => addr,
                _ => panic!("destination is not multicast: {:?}", destination),
            };

            (*self)
                .send_frame(bindings_ctx, IgmpPacketMetadata::new(device.clone(), addr), body)
                .map_err(|err| err.err_into())
        }
    }

    impl CounterContext<IgmpCounters> for &mut FakeCoreCtx {
        fn counters(&self) -> &IgmpCounters {
            <FakeCoreCtx as CounterContext<IgmpCounters>>::counters(self)
        }
    }

    impl ResourceCounterContext<FakeDeviceId, IgmpCounters> for &mut FakeCoreCtx {
        fn per_resource_counters<'a>(&'a self, device_id: &'a FakeDeviceId) -> &'a IgmpCounters {
            <
                FakeCoreCtx as ResourceCounterContext<FakeDeviceId, IgmpCounters>
            >::per_resource_counters(self, device_id)
        }
    }

    type CounterExpectations = IgmpCounters<u64>;

    impl CounterExpectations {
        #[track_caller]
        fn assert_counters<CC: ResourceCounterContext<FakeDeviceId, IgmpCounters>>(
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

    const MY_ADDR: SpecifiedAddr<Ipv4Addr> =
        unsafe { SpecifiedAddr::new_unchecked(Ipv4Addr::new([192, 168, 0, 2])) };
    const ROUTER_ADDR: Ipv4Addr = Ipv4Addr::new([192, 168, 0, 1]);
    const OTHER_HOST_ADDR: Ipv4Addr = Ipv4Addr::new([192, 168, 0, 3]);
    const GROUP_ADDR: MulticastAddr<Ipv4Addr> = <Ipv4 as gmp::testutil::TestIpExt>::GROUP_ADDR1;
    const GROUP_ADDR_2: MulticastAddr<Ipv4Addr> = <Ipv4 as gmp::testutil::TestIpExt>::GROUP_ADDR2;
    const GMP_TIMER_ID: IgmpTimerId<FakeWeakDeviceId<FakeDeviceId>> =
        IgmpTimerId::new(FakeWeakDeviceId(FakeDeviceId));

    fn new_recv_pkt_info() -> LocalDeliveryPacketInfo<Ipv4, FakeIpHeaderInfo> {
        LocalDeliveryPacketInfo {
            header_info: FakeIpHeaderInfo {
                router_alert: true,
                hop_limit: 1,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn receive_igmp_v1_query(core_ctx: &mut FakeCoreCtx, bindings_ctx: &mut FakeBindingsCtx) {
        let ser = IgmpPacketBuilder::<Buf<Vec<u8>>, IgmpMembershipQueryV2>::new_with_resp_time(
            GROUP_ADDR.get(),
            Duration::ZERO.try_into().unwrap(),
        );
        let buff = ser
            .into_serializer()
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        core_ctx.receive_igmp_packet(
            bindings_ctx,
            &FakeDeviceId,
            Ipv4SourceAddr::new(ROUTER_ADDR).unwrap(),
            MY_ADDR,
            buff,
            &new_recv_pkt_info(),
        );
    }

    fn receive_igmp_v2_query(
        core_ctx: &mut FakeCoreCtx,
        bindings_ctx: &mut FakeBindingsCtx,
        resp_time: NonZeroDuration,
    ) {
        let ser = IgmpPacketBuilder::<Buf<Vec<u8>>, IgmpMembershipQueryV2>::new_with_resp_time(
            GROUP_ADDR.get(),
            resp_time.get().try_into().unwrap(),
        );
        let buff = ser
            .into_serializer()
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        core_ctx.receive_igmp_packet(
            bindings_ctx,
            &FakeDeviceId,
            Ipv4SourceAddr::new(ROUTER_ADDR).unwrap(),
            MY_ADDR,
            buff,
            &new_recv_pkt_info(),
        );
    }

    fn receive_igmp_v2_general_query(
        core_ctx: &mut FakeCoreCtx,
        bindings_ctx: &mut FakeBindingsCtx,
        resp_time: NonZeroDuration,
    ) {
        let ser = IgmpPacketBuilder::<Buf<Vec<u8>>, IgmpMembershipQueryV2>::new_with_resp_time(
            Ipv4Addr::new([0, 0, 0, 0]),
            resp_time.get().try_into().unwrap(),
        );
        let buff = ser
            .into_serializer()
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        core_ctx.receive_igmp_packet(
            bindings_ctx,
            &FakeDeviceId,
            Ipv4SourceAddr::new(ROUTER_ADDR).unwrap(),
            MY_ADDR,
            buff,
            &new_recv_pkt_info(),
        );
    }

    fn receive_igmp_report(core_ctx: &mut FakeCoreCtx, bindings_ctx: &mut FakeBindingsCtx) {
        let ser = IgmpPacketBuilder::<Buf<Vec<u8>>, IgmpMembershipReportV2>::new(GROUP_ADDR.get());
        let buff = ser
            .into_serializer()
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        core_ctx.receive_igmp_packet(
            bindings_ctx,
            &FakeDeviceId,
            Ipv4SourceAddr::new(OTHER_HOST_ADDR).unwrap(),
            MY_ADDR,
            buff,
            &new_recv_pkt_info(),
        );
    }

    fn setup_igmpv2_test_environment_with_addr_subnet(
        seed: u128,
        a: Option<AddrSubnet<Ipv4Addr, Ipv4DeviceAddr>>,
    ) -> FakeCtx {
        let mut ctx = FakeCtx::with_default_bindings_ctx(|bindings_ctx| {
            // We start with enabled true to make tests easier to write.
            let igmp_enabled = true;
            FakeCoreCtx::with_state(FakeIgmpCtx {
                shared: Rc::new(RefCell::new(Shared {
                    groups: MulticastGroupSet::default(),
                    gmp_state: GmpState::new_with_enabled_and_mode::<_, IntoCoreTimerCtx>(
                        bindings_ctx,
                        FakeWeakDeviceId(FakeDeviceId),
                        igmp_enabled,
                        IgmpMode::V2 { compat: false },
                    ),
                    config: Default::default(),
                })),
                igmp_enabled,
                addr_subnet: None,
                stack_wide_counters: Default::default(),
                device_specific_counters: Default::default(),
            })
        });
        ctx.bindings_ctx.seed_rng(seed);
        ctx.core_ctx.state.addr_subnet = a;
        ctx
    }

    fn setup_igmpv2_test_environment(seed: u128) -> FakeCtx {
        setup_igmpv2_test_environment_with_addr_subnet(
            seed,
            Some(AddrSubnet::new(MY_ADDR.get(), 24).unwrap()),
        )
    }

    fn ensure_ttl_ihl_rtr(core_ctx: &FakeCoreCtx) {
        for (_, frame) in core_ctx.frames() {
            assert_eq!(frame[8], IGMP_IP_TTL); // TTL,
            assert_eq!(&frame[20..24], &[148, 4, 0, 0]); // RTR
            assert_eq!(frame[0], 0x46); // IHL
        }
    }

    #[test_case(Some(MY_ADDR); "specified_src")]
    #[test_case(None; "unspecified_src")]
    fn test_igmp_simple_integration(src_ip: Option<SpecifiedAddr<Ipv4Addr>>) {
        let check_report = |core_ctx: &mut FakeCoreCtx| {
            let expected_src_ip = src_ip.map_or(Ipv4::UNSPECIFIED_ADDRESS, |a| a.get());

            let frames = core_ctx.take_frames();
            let (IgmpPacketMetadata { device: FakeDeviceId, dst_ip }, frame) = assert_matches!(
                &frames[..], [x] => x);
            assert_eq!(dst_ip, &GROUP_ADDR);
            let (body, src_ip, dst_ip, proto, ttl) = parse_ip_packet::<Ipv4>(frame).unwrap();
            assert_eq!(src_ip, expected_src_ip);
            assert_eq!(dst_ip, GROUP_ADDR.get());
            assert_eq!(proto, Ipv4Proto::Igmp);
            assert_eq!(ttl, IGMP_IP_TTL);
            let mut bv = &body[..];
            assert_matches!(
                IgmpPacket::parse(&mut bv, ()).unwrap(),
                IgmpPacket::MembershipReportV2(msg) => {
                    assert_eq!(msg.group_addr(), GROUP_ADDR.get());
                }
            );
        };

        let addr_subnet = src_ip.map(|a| AddrSubnet::new(a.get(), 16).unwrap());
        run_with_many_seeds(|seed| {
            let FakeCtx { mut core_ctx, mut bindings_ctx } =
                setup_igmpv2_test_environment_with_addr_subnet(seed, addr_subnet);

            // Joining a group should send a report.
            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            check_report(&mut core_ctx);

            // Should send a report after a query.
            receive_igmp_v2_query(
                &mut core_ctx,
                &mut bindings_ctx,
                NonZeroDuration::from_secs(10).unwrap(),
            );
            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            check_report(&mut core_ctx);

            CounterExpectations { rx_igmpv2_query: 1, tx_igmpv2_report: 2, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_igmp_integration_fallback_from_idle() {
        run_with_many_seeds(|seed| {
            let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(seed);
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
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            assert_eq!(core_ctx.frames().len(), 2);

            receive_igmp_v2_query(
                &mut core_ctx,
                &mut bindings_ctx,
                NonZeroDuration::from_secs(10).unwrap(),
            );

            // We have received a query, hence we are falling back to Delay
            // Member state.
            let group_state = core_ctx.state.groups().get(&GROUP_ADDR).unwrap().v1();
            match group_state.get_inner() {
                gmp::v1::MemberState::Delaying(_) => {}
                _ => panic!("Wrong State!"),
            }

            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            assert_eq!(core_ctx.frames().len(), 3);
            ensure_ttl_ihl_rtr(&core_ctx);

            CounterExpectations { rx_igmpv2_query: 1, tx_igmpv2_report: 3, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_igmpv2_integration_igmpv1_router_present() {
        run_with_many_seeds(|seed| {
            let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(seed);

            assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V2 { compat: false });
            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            let now = bindings_ctx.now();
            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                now..=(now + IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL),
            )]);

            receive_igmp_v1_query(&mut core_ctx, &mut bindings_ctx);
            assert_eq!(core_ctx.frames().len(), 1);

            // Since we have heard from the v1 router, we should have set our
            // flag.
            let now = bindings_ctx.now();
            let until = now.panicking_add(DEFAULT_V1_ROUTER_PRESENT_TIMEOUT);
            assert_eq!(
                core_ctx.state.gmp_state().mode,
                IgmpMode::V1(IgmpV1Mode::V2Compat { until })
            );
            assert_eq!(core_ctx.state.gmp_state().mode.should_send_v1(&mut bindings_ctx), true);
            assert_eq!(core_ctx.frames().len(), 1);
            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                now..=(now + IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL),
            )]);
            bindings_ctx.timers.assert_timers_installed_range([(
                GMP_TIMER_ID,
                now..=(now + IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL),
            )]);

            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            // After the first timer, we send out our V1 report.
            assert_eq!(core_ctx.frames().len(), 2);
            // The last frame being sent should be a V1 report.
            let (_, frame) = core_ctx.frames().last().unwrap();
            // 34 and 0x12 are hacky but they can quickly tell it is a V1
            // report.
            assert_eq!(frame[24], 0x12);

            // Sleep until v1 router present is no more.
            bindings_ctx.timers.instant.time = until;

            // After the elapsed time, we should reset our flag for v1 routers.
            assert_eq!(core_ctx.state.gmp_state().mode.should_send_v1(&mut bindings_ctx), false);

            receive_igmp_v2_query(
                &mut core_ctx,
                &mut bindings_ctx,
                NonZeroDuration::from_secs(10).unwrap(),
            );
            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            assert_eq!(core_ctx.frames().len(), 3);
            // Now we should get V2 report
            assert_eq!(core_ctx.frames().last().unwrap().1[24], 0x16);
            ensure_ttl_ihl_rtr(&core_ctx);

            CounterExpectations {
                rx_igmpv1_query: 1,
                rx_igmpv2_query: 1,
                tx_igmpv2_report: 2,
                tx_igmpv1_report: 1,
                ..Default::default()
            }
            .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_igmp_integration_delay_reset_timer() {
        // This seed value was chosen to later produce a timer duration > 100ms.
        let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(123456);
        assert_eq!(
            core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
            GroupJoinResult::Joined(())
        );
        let now = bindings_ctx.now();
        core_ctx.state.gmp_state().timers.assert_range([(
            &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
            now..=(now + IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL),
        )]);
        let instant1 = bindings_ctx.timers.timers()[0].0.clone();
        let start = bindings_ctx.now();
        let duration = Duration::from_micros(((instant1 - start).as_micros() / 2) as u64);
        assert!(duration.as_millis() > 100);
        receive_igmp_v2_query(
            &mut core_ctx,
            &mut bindings_ctx,
            NonZeroDuration::new(duration).unwrap(),
        );
        assert_eq!(core_ctx.frames().len(), 1);
        let now = bindings_ctx.now();
        core_ctx.state.gmp_state().timers.assert_range([(
            &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
            now..=(now + duration),
        )]);
        let instant2 = bindings_ctx.timers.timers()[0].0.clone();
        // Because of the message, our timer should be reset to a nearer future.
        assert!(instant2 <= instant1);
        core_ctx
            .state
            .gmp_state()
            .timers
            .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
        assert!(bindings_ctx.now() - start <= duration);
        assert_eq!(core_ctx.frames().len(), 2);
        // Make sure it is a V2 report.
        assert_eq!(core_ctx.frames().last().unwrap().1[24], 0x16);
        ensure_ttl_ihl_rtr(&core_ctx);

        CounterExpectations { rx_igmpv2_query: 1, tx_igmpv2_report: 2, ..Default::default() }
            .assert_counters(&mut core_ctx);
    }

    #[test]
    fn test_igmp_integration_last_send_leave() {
        run_with_many_seeds(|seed| {
            let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(seed);
            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            let now = bindings_ctx.now();
            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                now..=(now + IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL),
            )]);
            // The initial unsolicited report.
            assert_eq!(core_ctx.frames().len(), 1);
            core_ctx
                .state
                .gmp_state()
                .timers
                .assert_top(&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), &());
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            // The report after the delay.
            assert_eq!(core_ctx.frames().len(), 2);
            assert_eq!(
                core_ctx.gmp_leave_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupLeaveResult::Left(())
            );
            // Our leave message.
            assert_eq!(core_ctx.frames().len(), 3);

            let leave_frame = &core_ctx.frames().last().unwrap().1;

            // Make sure it is a leave message.
            assert_eq!(leave_frame[24], 0x17);
            // Make sure the destination is ALL-ROUTERS (224.0.0.2).
            assert_eq!(leave_frame[16], 224);
            assert_eq!(leave_frame[17], 0);
            assert_eq!(leave_frame[18], 0);
            assert_eq!(leave_frame[19], 2);
            ensure_ttl_ihl_rtr(&core_ctx);

            CounterExpectations { tx_igmpv2_report: 2, tx_leave_group: 1, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_igmp_integration_always_idle_member() {
        run_with_many_seeds(|seed| {
            let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(seed);
            assert_eq!(
                core_ctx.gmp_join_group(
                    &mut bindings_ctx,
                    &FakeDeviceId,
                    Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS
                ),
                GroupJoinResult::Joined(())
            );
            assert_eq!(core_ctx.frames().len(), 0);
            bindings_ctx.timers.assert_no_timers_installed();

            CounterExpectations::default().assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_igmp_integration_not_last_does_not_send_leave() {
        run_with_many_seeds(|seed| {
            let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(seed);
            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            let now = bindings_ctx.now();
            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                now..=(now + IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL),
            )]);
            assert_eq!(core_ctx.frames().len(), 1);
            receive_igmp_report(&mut core_ctx, &mut bindings_ctx);
            bindings_ctx.timers.assert_no_timers_installed();
            // The report should be discarded because we have received from
            // someone else.
            assert_eq!(core_ctx.frames().len(), 1);
            assert_eq!(
                core_ctx.gmp_leave_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupLeaveResult::Left(())
            );
            // A leave message is not sent.
            assert_eq!(core_ctx.frames().len(), 1);
            ensure_ttl_ihl_rtr(&core_ctx);

            CounterExpectations { tx_igmpv2_report: 1, rx_igmpv2_report: 1, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_receive_general_query() {
        run_with_many_seeds(|seed| {
            let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(seed);
            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR_2),
                GroupJoinResult::Joined(())
            );
            let now = bindings_ctx.now();
            let range = now..=(now + IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL);
            core_ctx.state.gmp_state().timers.assert_range([
                (&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), range.clone()),
                (&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR_2).into(), range),
            ]);
            // The initial unsolicited report.
            assert_eq!(core_ctx.frames().len(), 2);
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            assert_eq!(core_ctx.frames().len(), 4);
            const RESP_TIME: NonZeroDuration = NonZeroDuration::from_secs(10).unwrap();
            receive_igmp_v2_general_query(&mut core_ctx, &mut bindings_ctx, RESP_TIME);
            // Two new timers should be there.
            let now = bindings_ctx.now();
            let range = now..=(now + RESP_TIME.get());
            core_ctx.state.gmp_state().timers.assert_range([
                (&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(), range.clone()),
                (&gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR_2).into(), range),
            ]);
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
            // Two new reports should be sent.
            assert_eq!(core_ctx.frames().len(), 6);
            ensure_ttl_ihl_rtr(&core_ctx);

            CounterExpectations { rx_igmpv2_query: 1, tx_igmpv2_report: 6, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_skip_igmp() {
        run_with_many_seeds(|seed| {
            // Test that we do not perform IGMP when IGMP is disabled.

            let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(seed);
            bindings_ctx.seed_rng(seed);
            // Test environment is created in enabled state.
            core_ctx.state.igmp_enabled = false;
            core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);

            // Assert that no observable effects have taken place.
            let assert_no_effect = |core_ctx: &FakeCoreCtx, bindings_ctx: &FakeBindingsCtx| {
                bindings_ctx.timers.assert_no_timers_installed();
                assert_empty(core_ctx.frames());
            };

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            // We should join the group but left in the GMP's non-member
            // state.
            assert_gmp_state!(core_ctx, &GROUP_ADDR, NonMember);
            assert_no_effect(&core_ctx, &bindings_ctx);

            receive_igmp_report(&mut core_ctx, &mut bindings_ctx);
            // We should have done no state transitions/work.
            assert_gmp_state!(core_ctx, &GROUP_ADDR, NonMember);
            assert_no_effect(&core_ctx, &bindings_ctx);

            receive_igmp_v2_query(
                &mut core_ctx,
                &mut bindings_ctx,
                NonZeroDuration::from_secs(10).unwrap(),
            );
            // We should have done no state transitions/work.
            assert_gmp_state!(core_ctx, &GROUP_ADDR, NonMember);
            assert_no_effect(&core_ctx, &bindings_ctx);

            assert_eq!(
                core_ctx.gmp_leave_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupLeaveResult::Left(())
            );
            // We should have left the group but not executed any `Actions`.
            assert!(core_ctx.state.groups().get(&GROUP_ADDR).is_none());
            assert_no_effect(&core_ctx, &bindings_ctx);

            CounterExpectations { rx_igmpv2_report: 1, rx_igmpv2_query: 1, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_igmp_integration_with_local_join_leave() {
        run_with_many_seeds(|seed| {
            // Simple IGMP integration test to check that when we call top-level
            // multicast join and leave functions, IGMP is performed.

            let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(seed);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            assert_eq!(core_ctx.frames().len(), 1);
            let now = bindings_ctx.now();
            let range = now..=(now + IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL);
            core_ctx.state.gmp_state().timers.assert_range([(
                &gmp::v1::DelayedReportTimerId::new_multicast(GROUP_ADDR).into(),
                range.clone(),
            )]);
            ensure_ttl_ihl_rtr(&core_ctx);

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
            ensure_ttl_ihl_rtr(&core_ctx);

            CounterExpectations { tx_igmpv2_report: 1, tx_leave_group: 1, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    #[test]
    fn test_igmp_enable_disable() {
        run_with_many_seeds(|seed| {
            let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(seed);
            assert_eq!(core_ctx.take_frames(), []);

            assert_eq!(
                core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, GROUP_ADDR),
                GroupJoinResult::Joined(())
            );
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            {
                let frames = core_ctx.take_frames();
                let (IgmpPacketMetadata { device: FakeDeviceId, dst_ip }, frame) =
                    assert_matches!(&frames[..], [x] => x);
                assert_eq!(dst_ip, &GROUP_ADDR);
                let (body, src_ip, dst_ip, proto, ttl) = parse_ip_packet::<Ipv4>(frame).unwrap();
                assert_eq!(src_ip, MY_ADDR.get());
                assert_eq!(dst_ip, GROUP_ADDR.get());
                assert_eq!(proto, Ipv4Proto::Igmp);
                assert_eq!(ttl, IGMP_IP_TTL);
                let mut bv = &body[..];
                assert_matches!(
                    IgmpPacket::parse(&mut bv, ()).unwrap(),
                    IgmpPacket::MembershipReportV2(msg) => {
                        assert_eq!(msg.group_addr(), GROUP_ADDR.get());
                    }
                );
            }

            // Should do nothing.
            core_ctx.gmp_handle_maybe_enabled(&mut bindings_ctx, &FakeDeviceId);
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            assert_eq!(core_ctx.take_frames(), []);

            // Should send done message.
            core_ctx.state.igmp_enabled = false;
            core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);
            assert_gmp_state!(core_ctx, &GROUP_ADDR, NonMember);
            {
                let frames = core_ctx.take_frames();
                let (IgmpPacketMetadata { device: FakeDeviceId, dst_ip }, frame) =
                    assert_matches!(&frames[..], [x] => x);
                assert_eq!(dst_ip, &Ipv4::ALL_ROUTERS_MULTICAST_ADDRESS);
                let (body, src_ip, dst_ip, proto, ttl) = parse_ip_packet::<Ipv4>(frame).unwrap();
                assert_eq!(src_ip, MY_ADDR.get());
                assert_eq!(dst_ip, Ipv4::ALL_ROUTERS_MULTICAST_ADDRESS.get());
                assert_eq!(proto, Ipv4Proto::Igmp);
                assert_eq!(ttl, IGMP_IP_TTL);
                let mut bv = &body[..];
                assert_matches!(
                    IgmpPacket::parse(&mut bv, ()).unwrap(),
                    IgmpPacket::LeaveGroup(msg) => {
                        assert_eq!(msg.group_addr(), GROUP_ADDR.get());
                    }
                );
            }

            // Should do nothing.
            core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);
            assert_gmp_state!(core_ctx, &GROUP_ADDR, NonMember);
            assert_eq!(core_ctx.take_frames(), []);

            // Should send report message.
            core_ctx.state.igmp_enabled = true;
            core_ctx.gmp_handle_maybe_enabled(&mut bindings_ctx, &FakeDeviceId);
            assert_gmp_state!(core_ctx, &GROUP_ADDR, Delaying);
            {
                let frames = core_ctx.take_frames();
                let (IgmpPacketMetadata { device: FakeDeviceId, dst_ip }, frame) =
                    assert_matches!(&frames[..], [x] => x);
                assert_eq!(dst_ip, &GROUP_ADDR);
                let (body, src_ip, dst_ip, proto, ttl) = parse_ip_packet::<Ipv4>(frame).unwrap();
                assert_eq!(src_ip, MY_ADDR.get());
                assert_eq!(dst_ip, GROUP_ADDR.get());
                assert_eq!(proto, Ipv4Proto::Igmp);
                assert_eq!(ttl, IGMP_IP_TTL);
                let mut bv = &body[..];
                assert_matches!(
                    IgmpPacket::parse(&mut bv, ()).unwrap(),
                    IgmpPacket::MembershipReportV2(msg) => {
                        assert_eq!(msg.group_addr(), GROUP_ADDR.get());
                    }
                );
            }

            CounterExpectations { tx_igmpv2_report: 2, tx_leave_group: 1, ..Default::default() }
                .assert_counters(&mut core_ctx);
        });
    }

    /// Test the basics of IGMPv3 report sending.
    #[test]
    fn send_igmpv3_report() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(0);
        let sent_report_addr = Ipv4::get_multicast_addr(130);
        let sent_report_mode = GroupRecordType::ModeIsExclude;
        let sent_report_sources = Vec::<Ipv4Addr>::new();
        core_ctx.with_gmp_state_mut_and_ctx(&FakeDeviceId, |mut core_ctx, _| {
            core_ctx.send_report_v2(
                &mut bindings_ctx,
                &FakeDeviceId,
                [gmp::v2::GroupRecord::new_with_sources(
                    GmpEnabledGroup::new(sent_report_addr).unwrap(),
                    sent_report_mode,
                    sent_report_sources.iter(),
                )]
                .into_iter(),
            );
        });

        let frames = core_ctx.take_frames();
        let (IgmpPacketMetadata { device: FakeDeviceId, dst_ip }, frame) =
            assert_matches!(&frames[..], [x] => x);
        assert_eq!(dst_ip, &ALL_IGMPV3_CAPABLE_ROUTERS);
        let mut buff = &frame[..];
        let ipv4 = buff.parse::<Ipv4Packet<_>>().expect("parse IPv4");
        assert_eq!(ipv4.ttl(), IGMP_IP_TTL);
        assert_eq!(ipv4.src_ip(), MY_ADDR.get());
        assert_eq!(ipv4.dst_ip(), ALL_IGMPV3_CAPABLE_ROUTERS.get());
        assert_eq!(ipv4.proto(), Ipv4Proto::Igmp);
        assert_eq!(ipv4.dscp_and_ecn(), IGMPV3_DSCP_AND_ECN);
        assert_eq!(
            ipv4.iter_options()
                .map(|o| {
                    assert_matches!(o, Ipv4Option::RouterAlert { data: 0 });
                })
                .count(),
            1
        );
        let igmp = buff.parse::<IgmpPacket<_>>().expect("parse IGMP");
        let report = assert_matches!(
            igmp,
            IgmpPacket::MembershipReportV3(report) => report
        );
        let report = report
            .body()
            .iter()
            .map(|r| {
                (
                    r.header().multicast_addr().clone(),
                    r.header().record_type().unwrap(),
                    r.sources().to_vec(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(report, vec![(sent_report_addr.get(), sent_report_mode, sent_report_sources)]);

        CounterExpectations { tx_igmpv3_report: 1, ..Default::default() }
            .assert_counters(&mut core_ctx);
    }

    /// Tests IGMPv3 entering compatibility modes.
    #[test]
    fn igmpv3_version_compat() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(0);
        core_ctx.with_gmp_state_mut(&FakeDeviceId, |state| {
            gmp::enter_mode(&mut bindings_ctx, state, IgmpMode::V3);
        });

        for _ in 0..2 {
            assert_eq!(
                gmp::v1::handle_query_message(
                    &mut core_ctx,
                    &mut bindings_ctx,
                    &FakeDeviceId,
                    &gmp::testutil::FakeV1Query {
                        group_addr: Ipv4::UNSPECIFIED_ADDRESS,
                        max_response_time: Duration::ZERO
                    }
                ),
                Ok(())
            );
            // We should be in IGMPv1 compat mode and the compat instant is set
            // by the IGMPv3 RFC definition.
            let (gmp_state, config) = core_ctx.state.gmp_state_and_config();
            let until = bindings_ctx.now().panicking_add(
                gmp_state.v2_proto.older_version_querier_present_timeout(config).get(),
            );
            assert_eq!(gmp_state.mode, IgmpMode::V1(IgmpV1Mode::V3Compat { until }));
            assert_eq!(gmp_state.mode.should_send_v1(&mut bindings_ctx), true);
            bindings_ctx.timers.instant.sleep(Duration::from_secs(2));
        }

        let (v2_deadline, ()) =
            core_ctx.state.gmp_state().timers.get(&gmp::TimerIdInner::V1Compat).unwrap();
        let prev_mode = core_ctx.state.gmp_state().mode;
        // Now receive an IGMPv2 query. The IGMPv1 compat deadline should not
        // move.
        assert_eq!(
            gmp::v1::handle_query_message(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeDeviceId,
                &gmp::testutil::FakeV1Query {
                    group_addr: Ipv4::UNSPECIFIED_ADDRESS,
                    max_response_time: Duration::from_secs(10)
                }
            ),
            Ok(())
        );
        assert_eq!(core_ctx.state.gmp_state().mode, prev_mode);
        assert_ne!(
            core_ctx.state.gmp_state().timers.get(&gmp::TimerIdInner::V1Compat).unwrap(),
            (v2_deadline, &())
        );
        // We haven't moved timers so we should still be sending IGMPv1 responses.
        assert_eq!(core_ctx.state.gmp_state().mode.should_send_v1(&mut bindings_ctx), true);

        // Triggering the next timer should clear IGMPv2 compat back into IGMPv3
        // and we update the compat mode.
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(GMP_TIMER_ID));
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V3);
        assert_eq!(core_ctx.state.gmp_state().mode.should_send_v1(&mut bindings_ctx), false);
    }

    #[test]
    fn version_compat_clears_on_disable() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } = setup_igmpv2_test_environment(0);
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V2 { compat: false });
        assert_eq!(
            gmp::v1::handle_query_message(
                &mut core_ctx,
                &mut bindings_ctx,
                &FakeDeviceId,
                &gmp::testutil::FakeV1Query {
                    group_addr: Ipv4::UNSPECIFIED_ADDRESS,
                    max_response_time: Duration::ZERO
                }
            ),
            Ok(())
        );
        assert_matches!(core_ctx.state.gmp_state().mode, IgmpMode::V1(IgmpV1Mode::V2Compat { .. }));
        core_ctx.state.igmp_enabled = false;
        core_ctx.gmp_handle_disabled(&mut bindings_ctx, &FakeDeviceId);
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V2 { compat: false });
    }

    #[test]
    fn user_mode_change() {
        let mut ctx = setup_igmpv2_test_environment(0);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        assert_eq!(core_ctx.gmp_get_mode(&FakeDeviceId), IgmpConfigMode::V2);
        assert_eq!(
            core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, GROUP_ADDR),
            GroupJoinResult::Joined(())
        );
        // Ignore first reports.
        let _ = core_ctx.take_frames();
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, IgmpConfigMode::V3),
            IgmpConfigMode::V2
        );
        assert_eq!(core_ctx.gmp_get_mode(&FakeDeviceId), IgmpConfigMode::V3);
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V3);
        // No side-effects.
        assert_eq!(core_ctx.take_frames(), Vec::new());

        // If we receive an IGMPv1 query, we'll go into compat mode but still
        // report IGMPv3 to the user.
        receive_igmp_v1_query(core_ctx, bindings_ctx);
        assert_matches!(core_ctx.state.gmp_state().mode, IgmpMode::V1(IgmpV1Mode::V3Compat { .. }));
        assert_eq!(core_ctx.gmp_get_mode(&FakeDeviceId), IgmpConfigMode::V3);

        // Acknowledge query response.
        assert_eq!(bindings_ctx.trigger_next_timer(core_ctx), Some(GMP_TIMER_ID));
        assert_eq!(core_ctx.take_frames().len(), 1);

        // Even if user attempts to set IGMPv3 again we'll keep it in compat.
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, IgmpConfigMode::V3),
            IgmpConfigMode::V3
        );
        assert_eq!(core_ctx.take_frames(), Vec::new());
        let until = assert_matches!(
            core_ctx.state.gmp_state().mode,
            IgmpMode::V1(IgmpV1Mode::V3Compat { until }) => until
        );
        // If the user switches to IGMPv2, we keep the IGMPv1 compat information.
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, IgmpConfigMode::V2),
            IgmpConfigMode::V3
        );
        assert_eq!(core_ctx.take_frames(), Vec::new());
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V1(IgmpV1Mode::V2Compat { until }));

        // Forcing IGMPv1 mode, however, exits compat.
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, IgmpConfigMode::V1),
            IgmpConfigMode::V2
        );
        assert_eq!(core_ctx.take_frames(), Vec::new());
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V1(IgmpV1Mode::Forced));

        // Back to IGMPv2...
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, IgmpConfigMode::V2),
            IgmpConfigMode::V1
        );
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V2 { compat: false });
        // ...and then IGMPv3.
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, IgmpConfigMode::V3),
            IgmpConfigMode::V2
        );
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V3);
        assert_eq!(core_ctx.take_frames(), Vec::new());

        // Receiving an IGMPv2 query while in IGMPv3 enters compat.
        receive_igmp_v2_query(core_ctx, bindings_ctx, NonZeroDuration::from_secs(1).unwrap());
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V2 { compat: true });
        assert_eq!(core_ctx.gmp_get_mode(&FakeDeviceId), IgmpConfigMode::V3);
        // Acknowledge query response.
        assert_eq!(bindings_ctx.trigger_next_timer(core_ctx), Some(GMP_TIMER_ID));
        assert_eq!(core_ctx.take_frames().len(), 1);

        // Even if user attempts to set IGMPv3 again we'll keep it in compat.
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, IgmpConfigMode::V3),
            IgmpConfigMode::V3
        );
        assert_eq!(core_ctx.take_frames(), Vec::new());
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V2 { compat: true });

        // Force IGMPv2 exits IGMPv2 compat.
        assert_eq!(
            core_ctx.gmp_set_mode(bindings_ctx, &FakeDeviceId, IgmpConfigMode::V2),
            IgmpConfigMode::V3
        );
        assert_eq!(core_ctx.state.gmp_state().mode, IgmpMode::V2 { compat: false });
        assert_eq!(core_ctx.take_frames(), Vec::new());
        // No timers.
        core_ctx.state.gmp_state().timers.assert_timers([]);
    }

    #[test]
    fn reject_bad_messages() {
        let mut ctx = setup_igmpv2_test_environment(0);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;

        let v1_query = {
            let ser = IgmpPacketBuilder::<Buf<Vec<u8>>, IgmpMembershipQueryV2>::new_with_resp_time(
                Ipv4::UNSPECIFIED_ADDRESS,
                Duration::ZERO.try_into().unwrap(),
            );
            ser.into_serializer()
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .unwrap()
        };
        let v2_query = {
            let ser = IgmpPacketBuilder::<Buf<Vec<u8>>, IgmpMembershipQueryV2>::new_with_resp_time(
                Ipv4::UNSPECIFIED_ADDRESS,
                Duration::from_secs(10).try_into().unwrap(),
            );
            ser.into_serializer()
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .unwrap()
        };
        let v3_query = {
            let ser = IgmpMembershipQueryV3Builder::new(
                IgmpResponseTimeV3::new_exact(Duration::from_secs(1)).unwrap(),
                None,
                false,
                Igmpv3QRV::new(2),
                Igmpv3QQIC::new_exact(Duration::from_secs(125)).unwrap(),
                core::iter::empty(),
            );
            ser.into_serializer()
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .unwrap()
        };

        let base_header_info = new_recv_pkt_info().header_info;

        // TTL must be 1.
        const BAD_TTL: u8 = 2;
        for q in [&v1_query, &v2_query, &v3_query] {
            assert_eq!(
                receive_igmp_packet(
                    core_ctx,
                    bindings_ctx,
                    &FakeDeviceId,
                    Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into(),
                    q.clone(),
                    &LocalDeliveryPacketInfo {
                        header_info: FakeIpHeaderInfo {
                            hop_limit: BAD_TTL,
                            ..base_header_info.clone()
                        },
                        ..Default::default()
                    }
                ),
                Err(IgmpError::BadTtl(BAD_TTL))
            );
        }

        // Multicast destination IPs must be all nodes.
        const BAD_DST_IP: MulticastAddr<Ipv4Addr> = GROUP_ADDR;
        for q in [&v1_query, &v2_query, &v3_query] {
            assert_eq!(
                receive_igmp_packet(
                    core_ctx,
                    bindings_ctx,
                    &FakeDeviceId,
                    BAD_DST_IP.into(),
                    q.clone(),
                    &new_recv_pkt_info(),
                ),
                Err(IgmpError::RejectedGeneralQuery { dst_ip: BAD_DST_IP.get() })
            );
        }

        for q in [&v2_query, &v3_query] {
            assert_eq!(
                receive_igmp_packet(
                    core_ctx,
                    bindings_ctx,
                    &FakeDeviceId,
                    MY_ADDR,
                    q.clone(),
                    &LocalDeliveryPacketInfo {
                        header_info: FakeIpHeaderInfo {
                            // Router alert must be set.
                            router_alert: false,
                            ..base_header_info.clone()
                        },
                        ..Default::default()
                    },
                ),
                Err(IgmpError::MissingRouterAlertInQuery)
            );
        }

        CounterExpectations {
            // Note: Queries with a Bad TTL aren't accounted for here.
            rx_igmpv1_query: 1,
            rx_igmpv2_query: 2,
            rx_igmpv3_query: 2,
            rx_err_rejected_general_query: 3,
            rx_err_missing_router_alert_in_query: 2,
            rx_err_bad_ttl: 3,
            ..Default::default()
        }
        .assert_counters(core_ctx);
    }
}
