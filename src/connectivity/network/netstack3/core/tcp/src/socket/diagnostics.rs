// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::convert::Infallible as Never;
use core::num::NonZeroU16;

use net_types::Witness as _;
use net_types::ip::{Ip, IpAddress as _};
use netstack3_base::{
    InstantBindingsTypes, IpSocketProperties, Marks, Matcher, SocketDiagnosticsSeed,
    WeakDeviceIdentifier,
};

use crate::TcpCountersWithSocket;
use crate::internal::socket::{
    BoundState, DualStackIpExt, Listener, SocketCookie, TcpBindingsTypes, TcpSocketId,
    TcpSocketState, TcpSocketStateInner, Unbound,
};
use crate::internal::state::State;
use crate::internal::state::info::TcpSocketInfo;

/// Required state gathered into one struct for matching a socket, so it's
/// possible to implement traits against the collection.
pub(crate) struct TcpSocketStateForMatching<
    'a,
    I: DualStackIpExt,
    D: netstack3_base::WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
> {
    pub(crate) state: &'a TcpSocketState<I, D, BT>,
    pub(crate) id: &'a TcpSocketId<I, D, BT>,
}

impl<'a, I: DualStackIpExt, D: netstack3_base::WeakDeviceIdentifier, BT: TcpBindingsTypes>
    netstack3_base::MaybeSocketTransportProperties for TcpSocketStateForMatching<'a, I, D, BT>
{
    type TcpProps<'b>
        = Self
    where
        Self: 'b;

    type UdpProps<'b>
        = Never
    where
        Self: 'b;

    fn tcp_socket_properties(&self) -> Option<&Self::TcpProps<'_>> {
        Some(self)
    }

    fn udp_socket_properties(&self) -> Option<&Self::UdpProps<'_>> {
        None
    }
}

impl<'a, I: DualStackIpExt, D: netstack3_base::WeakDeviceIdentifier, BT: TcpBindingsTypes>
    netstack3_base::TcpSocketProperties for TcpSocketStateForMatching<'a, I, D, BT>
{
    fn src_port_matches(&self, matcher: &netstack3_base::BoundPortMatcher) -> bool {
        let src_port = match &self.state.socket_state {
            TcpSocketStateInner::Unbound(_) => None,
            TcpSocketStateInner::Bound(BoundState { addr, .. })
            | TcpSocketStateInner::Listener(Listener { addr, .. }) => {
                Some(I::get_bound_info(addr).port.get())
            }
            TcpSocketStateInner::Connected { conn, .. } => {
                Some(I::get_conn_info(conn).local_addr.port.get())
            }
        };

        matcher.matches(&src_port)
    }

    fn dst_port_matches(&self, matcher: &netstack3_base::BoundPortMatcher) -> bool {
        let dst_port = match &self.state.socket_state {
            TcpSocketStateInner::Unbound(_)
            | TcpSocketStateInner::Bound(_)
            | TcpSocketStateInner::Listener(_) => None,
            TcpSocketStateInner::Connected { conn, .. } => {
                Some(I::get_conn_info(conn).remote_addr.port.get())
            }
        };

        matcher.matches(&dst_port)
    }

    fn state_matches(&self, matcher: &netstack3_base::TcpStateMatcher) -> bool {
        matcher.matches(&self.state.base_state())
    }
}

impl<'a, I: DualStackIpExt, D: netstack3_base::WeakDeviceIdentifier, BT: TcpBindingsTypes>
    IpSocketProperties<BT::DeviceClass> for TcpSocketStateForMatching<'a, I, D, BT>
where
    D::Strong: netstack3_base::InterfaceProperties<BT::DeviceClass>,
{
    fn family_matches(&self, family: &net_types::ip::IpVersion) -> bool {
        I::VERSION == *family
    }

    fn src_addr_matches(&self, addr: &netstack3_base::BoundAddressMatcherEither) -> bool {
        let src_addr = match &self.state.socket_state {
            TcpSocketStateInner::Unbound(_) => None,
            TcpSocketStateInner::Bound(BoundState { addr, .. })
            | TcpSocketStateInner::Listener(Listener { addr, .. }) => {
                I::get_bound_info(addr).addr.map(|a| a.addr().get())
            }
            TcpSocketStateInner::Connected { conn, .. } => {
                Some(I::get_conn_info(conn).local_addr.ip.addr().get())
            }
        };

        addr.matches(&src_addr.map(|a| a.to_ip_addr()))
    }

    fn dst_addr_matches(&self, addr: &netstack3_base::BoundAddressMatcherEither) -> bool {
        let dst_addr = match &self.state.socket_state {
            TcpSocketStateInner::Unbound(_)
            | TcpSocketStateInner::Bound(_)
            | TcpSocketStateInner::Listener(_) => None,
            TcpSocketStateInner::Connected { conn, .. } => {
                Some(I::get_conn_info(conn).remote_addr.ip.addr().get())
            }
        };

        addr.matches(&dst_addr.map(|a| a.to_ip_addr()))
    }

    fn transport_protocol_matches(
        &self,
        proto: &netstack3_base::SocketTransportProtocolMatcher,
    ) -> bool {
        proto.matches(self)
    }

    fn bound_interface_matches(
        &self,
        iface: &netstack3_base::BoundInterfaceMatcher<BT::DeviceClass>,
    ) -> bool {
        let device = match &self.state.socket_state {
            TcpSocketStateInner::Unbound(_) => None,
            TcpSocketStateInner::Bound(BoundState { addr, .. })
            | TcpSocketStateInner::Listener(Listener { addr, .. }) => {
                I::get_bound_info(addr).device
            }
            TcpSocketStateInner::Connected { conn, .. } => I::get_conn_info(conn).device,
        };
        iface.matches(&device.and_then(|d| d.upgrade()).as_ref())
    }

    fn cookie_matches(&self, cookie: &netstack3_base::SocketCookieMatcher) -> bool {
        cookie.matches(&self.id.socket_cookie().export_value())
    }

    fn mark_matches(&self, matcher: &netstack3_base::MarkInDomainMatcher) -> bool {
        matcher.matcher.matches(self.state.socket_options.ip_options.marks.get(matcher.domain))
    }
}

/// Publicly-accessible diagnostic information about TCP sockets.
#[cfg_attr(any(test, feature = "testutils"), derive(Debug, PartialEq, Eq))]
pub struct TcpSocketDiagnostics<I: Ip, Instant> {
    /// The socket's TCP state machine.
    pub state_machine: netstack3_base::TcpSocketState,
    /// The socket's tuple.
    pub tuple: TcpSocketDiagnosticTuple<I>,
    /// The socket's cookie.
    pub cookie: SocketCookie,
    /// The socket's marks.
    pub marks: Marks,
    /// The socket's extended info.
    pub tcp_info: Option<TcpSocketInfo<Instant>>,
}

/// All of the information required to compute [`TcpSocketDiagnostics`]. Gives
/// the owner control over when (or if) the transformation happens.
pub struct TcpSocketDiagnosticsSeed<I, D, BT>
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
{
    pub(crate) state: TcpSocketState<I, D, BT>,
    pub(crate) counters: TcpCountersWithSocket<I>,
    pub(crate) cookie: SocketCookie,
}

impl<I, D, BT> SocketDiagnosticsSeed for TcpSocketDiagnosticsSeed<I, D, BT>
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
{
    type Output = TcpSocketDiagnostics<I, <BT as InstantBindingsTypes>::Instant>;

    fn resolve(self) -> Option<Self::Output> {
        let Self { state, counters, cookie } = self;

        state.get_diagnostics(&counters, true).map(|(tuple, state_machine, marks, tcp_info)| {
            TcpSocketDiagnostics { tuple, state_machine, cookie, marks, tcp_info }
        })
    }
}

/// The tuple of a TCP socket.
///
/// This is separate from the state machine state because it's possible for
/// some states to be entered while having just the 2-tuple or the full
/// 4-tuple (CLOSED and LISTENING).
#[derive(Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum TcpSocketDiagnosticTuple<I: Ip> {
    /// The socket is bound, but not connected. Only the 2-tuple is available,
    /// although the source address might be None if the socket is bound to the
    /// catch-all address.
    Bound { src_addr: Option<I::Addr>, src_port: NonZeroU16 },
    /// The socket is connected, so the full 4-tuple is available.
    Connected { src_addr: I::Addr, src_port: NonZeroU16, dst_addr: I::Addr, dst_port: NonZeroU16 },
}

impl<I: Ip> TcpSocketDiagnosticTuple<I> {
    /// Returns the socket's source address.
    pub fn src_addr(&self) -> Option<I::Addr> {
        match self {
            Self::Bound { src_addr, src_port: _ } => *src_addr,
            Self::Connected { src_addr, src_port: _, dst_addr: _, dst_port: _ } => Some(*src_addr),
        }
    }

    /// Returns the socket's source port.
    pub fn src_port(&self) -> Option<NonZeroU16> {
        match self {
            Self::Bound { src_addr: _, src_port }
            | Self::Connected { src_addr: _, src_port, dst_addr: _, dst_port: _ } => {
                Some(*src_port)
            }
        }
    }

    /// Returns the socket's destination address.
    pub fn dst_addr(&self) -> Option<I::Addr> {
        match self {
            Self::Bound { src_addr: _, src_port: _ } => None,
            Self::Connected { src_addr: _, src_port: _, dst_addr, dst_port: _ } => Some(*dst_addr),
        }
    }

    /// Returns the socket's destination port.
    pub fn dst_port(&self) -> Option<NonZeroU16> {
        match self {
            Self::Bound { src_addr: _, src_port: _ } => None,
            Self::Connected { src_addr: _, src_port: _, dst_addr: _, dst_port } => Some(*dst_port),
        }
    }
}

impl<I, D, BT> TcpSocketState<I, D, BT>
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
{
    pub(crate) fn tcp_info(
        &self,
        counters: &crate::internal::counters::TcpCountersWithSocket<I>,
    ) -> TcpSocketInfo<BT::Instant> {
        match &self.socket_state {
            TcpSocketStateInner::Unbound(unbound) => {
                TcpSocketInfo::from_partial_state(unbound.base_state(), counters.as_ref())
            }
            TcpSocketStateInner::Bound(bound) => {
                TcpSocketInfo::from_partial_state(bound.base_state(), counters.as_ref())
            }
            TcpSocketStateInner::Listener(listener) => {
                TcpSocketInfo::from_partial_state(listener.base_state(), counters.as_ref())
            }
            TcpSocketStateInner::Connected { conn, timer: _ } => {
                TcpSocketInfo::from_full_state(I::get_state(conn), counters.as_ref())
            }
        }
    }

    pub(crate) fn get_diagnostics(
        &self,
        counters: &crate::internal::counters::TcpCountersWithSocket<I>,
        extended_info: bool,
    ) -> Option<(
        TcpSocketDiagnosticTuple<I>,
        netstack3_base::TcpSocketState,
        Marks,
        Option<TcpSocketInfo<BT::Instant>>,
    )> {
        let tuple = match &self.socket_state {
            TcpSocketStateInner::Unbound(_) => None,
            TcpSocketStateInner::Bound(BoundState { addr, .. })
            | TcpSocketStateInner::Listener(Listener { addr, .. }) => {
                let addr_info = I::get_bound_info(addr);
                Some(TcpSocketDiagnosticTuple::Bound {
                    src_addr: addr_info.addr.map(|ip| ip.addr().get()),
                    src_port: addr_info.port,
                })
            }
            TcpSocketStateInner::Connected { conn, .. } => {
                let info = I::get_conn_info(conn);
                Some(TcpSocketDiagnosticTuple::Connected {
                    src_addr: info.local_addr.ip.addr().get(),
                    dst_addr: info.remote_addr.ip.addr().get(),
                    src_port: info.local_addr.port,
                    dst_port: info.remote_addr.port,
                })
            }
        }?;

        Some((
            tuple,
            self.base_state(),
            self.socket_options.ip_options.marks,
            extended_info.then(|| self.tcp_info(counters)),
        ))
    }

    pub(crate) fn base_state(&self) -> netstack3_base::TcpSocketState {
        match &self.socket_state {
            TcpSocketStateInner::Unbound(unbound) => unbound.base_state(),
            TcpSocketStateInner::Bound(bound) => bound.base_state(),
            TcpSocketStateInner::Listener(listener) => listener.base_state(),
            TcpSocketStateInner::Connected { conn, .. } => match I::get_state(conn) {
                State::Closed(_) => netstack3_base::TcpSocketState::Close,
                State::Listen(_) => netstack3_base::TcpSocketState::Listen,
                State::SynRcvd(_) => netstack3_base::TcpSocketState::SynRecv,
                State::SynSent(_) => netstack3_base::TcpSocketState::SynSent,
                State::Established(_) => netstack3_base::TcpSocketState::Established,
                State::CloseWait(_) => netstack3_base::TcpSocketState::CloseWait,
                State::LastAck(_) => netstack3_base::TcpSocketState::LastAck,
                State::FinWait1(_) => netstack3_base::TcpSocketState::FinWait1,
                State::FinWait2(_) => netstack3_base::TcpSocketState::FinWait2,
                State::Closing(_) => netstack3_base::TcpSocketState::Closing,
                State::TimeWait(_) => netstack3_base::TcpSocketState::TimeWait,
            },
        }
    }
}

impl<D, Extra> Unbound<D, Extra> {
    fn base_state(&self) -> netstack3_base::TcpSocketState {
        netstack3_base::TcpSocketState::Close
    }
}

impl<I, D, BT> BoundState<I, D, BT>
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
{
    fn base_state(&self) -> netstack3_base::TcpSocketState {
        netstack3_base::TcpSocketState::Close
    }
}

impl<I, D, BT> Listener<I, D, BT>
where
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
{
    fn base_state(&self) -> netstack3_base::TcpSocketState {
        netstack3_base::TcpSocketState::Listen
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;
    use assert_matches::assert_matches;
    use core::num::NonZeroUsize;
    use core::time::Duration;

    use ip_test_macro::ip_test;
    use net_types::ZonedAddr;
    use net_types::ip::{Ip, Subnet};
    use netstack3_base::testutil::{
        FakeDeviceId, FakeInstant, FakeNetworkSpec as _, set_logger_for_test,
    };
    use netstack3_base::{
        AddressMatcher, AddressMatcherEither, AddressMatcherType, BoundAddressMatcherEither,
        BoundInterfaceMatcher, BoundPortMatcher, InterfaceMatcher, IpSocketMatcher, Mark,
        MarkDomain, MarkMatcher, PortMatcher, SocketCookieMatcher, SocketTransportProtocolMatcher,
        StrongDeviceIdentifier, SubnetMatcher, TcpSocketMatcher, TcpStateMatcher, UdpSocketMatcher,
    };
    use test_case::test_case;
    use test_util::assert_gt;

    use super::*;
    use crate::AcceptError;
    use crate::internal::base::ConnectionError;
    use crate::internal::socket::TcpContext;
    use crate::internal::socket::tests::{
        FakeTcpNetworkSpec, TcpApiExt, TcpBindingsCtx, TcpCoreCtx, TcpCtx, TcpTestIpExt,
    };
    use crate::internal::state::info::CongestionControlState;

    const LOCAL_PORT_1: NonZeroU16 = NonZeroU16::new(1234).unwrap();
    const LOCAL_PORT_2: NonZeroU16 = NonZeroU16::new(5678).unwrap();
    const LOCAL_PORT_3: NonZeroU16 = NonZeroU16::new(4321).unwrap();

    const REMOTE_PORT_1: NonZeroU16 = NonZeroU16::new(100).unwrap();
    const REMOTE_PORT_2: NonZeroU16 = NonZeroU16::new(200).unwrap();

    const MARK: u32 = 0x10;
    const MARK_MASK: u32 = !0;

    #[ip_test(I)]
    fn diagnostics_match_ip_version<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        api.bind(&s, None, Some(LOCAL_PORT_1)).expect("bind should succeed");
        api.listen(&s, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        let mut results = Vec::new();
        api.bound_sockets_diagnostics(&IpSocketMatcher::Family(I::VERSION), &mut results, false);
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound { src_addr: None, src_port: LOCAL_PORT_1 },
                cookie: s.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Family(
                <<I as netstack3_base::socket::DualStackIpExt>::OtherVersion as Ip>::VERSION,
            ),
            &mut results,
            false,
        );
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_src_addr<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");
        api.listen(&s, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        let mut results = Vec::new();
        let matcher = I::map_ip_in(
            I::TEST_ADDRS.local_ip.get(),
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 32).unwrap(),
                    )),
                    invert: false,
                }))
            },
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 128).unwrap(),
                    )),
                    invert: false,
                }))
            },
        );
        api.bound_sockets_diagnostics(&IpSocketMatcher::SrcAddr(matcher), &mut results, false);
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound {
                    src_addr: Some(I::TEST_ADDRS.local_ip.get()),
                    src_port: LOCAL_PORT_1,
                },
                cookie: s.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        let matcher = I::map_ip_in(
            I::TEST_ADDRS.remote_ip.get(),
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 32).unwrap(),
                    )),
                    invert: false,
                }))
            },
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 128).unwrap(),
                    )),
                    invert: false,
                }))
            },
        );
        api.bound_sockets_diagnostics(&IpSocketMatcher::SrcAddr(matcher), &mut results, false);
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_dst_addr<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");
        api.connect(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), LOCAL_PORT_2)
            .expect("connect should succeed");

        let mut results = Vec::new();
        let matcher = I::map_ip_in(
            I::TEST_ADDRS.remote_ip.get(),
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 32).unwrap(),
                    )),
                    invert: false,
                }))
            },
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 128).unwrap(),
                    )),
                    invert: false,
                }))
            },
        );
        api.bound_sockets_diagnostics(&IpSocketMatcher::DstAddr(matcher), &mut results, false);
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::SynSent,
                tuple: TcpSocketDiagnosticTuple::Connected {
                    src_addr: I::TEST_ADDRS.local_ip.get(),
                    src_port: LOCAL_PORT_1,
                    dst_addr: I::TEST_ADDRS.remote_ip.get(),
                    dst_port: LOCAL_PORT_2,
                },
                cookie: s.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        let matcher = I::map_ip_in(
            I::TEST_ADDRS.local_ip.get(),
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 32).unwrap(),
                    )),
                    invert: false,
                }))
            },
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 128).unwrap(),
                    )),
                    invert: false,
                }))
            },
        );
        api.bound_sockets_diagnostics(&IpSocketMatcher::DstAddr(matcher), &mut results, false);
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_proto<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        api.bind(&s, None, Some(LOCAL_PORT_1)).expect("bind should succeed");
        api.listen(&s, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        let mut results = Vec::new();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(TcpSocketMatcher::Empty)),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound { src_addr: None, src_port: LOCAL_PORT_1 },
                cookie: s.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Udp(UdpSocketMatcher::Empty)),
            &mut results,
            false,
        );
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_device<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        api.set_device(&s, Some(FakeDeviceId)).expect("set device should succeed");
        api.bind(&s, None, Some(LOCAL_PORT_1)).expect("bind should succeed");
        api.listen(&s, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        let mut results = Vec::new();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::BoundInterface(BoundInterfaceMatcher::Bound(InterfaceMatcher::Name(
                FakeDeviceId::FAKE_NAME.to_string(),
            ))),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound { src_addr: None, src_port: LOCAL_PORT_1 },
                cookie: s.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::BoundInterface(BoundInterfaceMatcher::Unbound),
            &mut results,
            false,
        );
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_cookie<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        let socket_1 = api.create(Default::default());
        api.bind(&socket_1, None, Some(LOCAL_PORT_1)).expect("bind should succeed");
        api.listen(&socket_1, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");
        let socket_2 = api.create(Default::default());
        api.bind(&socket_2, None, Some(LOCAL_PORT_2)).expect("bind should succeed");
        api.listen(&socket_2, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        let mut results = Vec::new();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Cookie(SocketCookieMatcher {
                cookie: socket_1.socket_cookie().export_value(),
                invert: false,
            }),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound { src_addr: None, src_port: LOCAL_PORT_1 },
                cookie: socket_1.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Cookie(SocketCookieMatcher {
                cookie: socket_2.socket_cookie().export_value(),
                invert: false,
            }),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound { src_addr: None, src_port: LOCAL_PORT_2 },
                cookie: socket_2.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );
    }

    #[ip_test(I, test = false)]
    #[test_case::test_case(MarkDomain::Mark1; "mark_1")]
    #[test_case::test_case(MarkDomain::Mark2; "mark_2")]
    fn diagnostics_match_mark<I: TcpTestIpExt>(domain: MarkDomain)
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        let s = api.create(Default::default());
        api.bind(&s, None, Some(LOCAL_PORT_1)).expect("bind should succeed");
        api.listen(&s, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");
        api.set_mark(&s, domain, Mark(Some(MARK)));

        let mut results = Vec::new();
        let matcher = |query_mark| {
            IpSocketMatcher::Mark(netstack3_base::MarkInDomainMatcher {
                domain,
                matcher: MarkMatcher::Marked {
                    mask: MARK_MASK,
                    start: query_mark,
                    end: query_mark,
                    invert: false,
                },
            })
        };
        api.bound_sockets_diagnostics(&matcher(MARK), &mut results, false);
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound { src_addr: None, src_port: LOCAL_PORT_1 },
                cookie: s.socket_cookie(),
                marks: netstack3_base::MarkStorage::new([(domain, MARK)]),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(&matcher(MARK + 1), &mut results, false);
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_multiple<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        let socket_1 = api.create(Default::default());
        api.bind(&socket_1, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");
        api.connect(&socket_1, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_PORT_1)
            .expect("connect socket 1");
        let socket_2 = api.create(Default::default());
        api.bind(&socket_2, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_2))
            .expect("bind should succeed");
        api.connect(&socket_2, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_PORT_1)
            .expect("connect socket 2");
        let socket_3 = api.create(Default::default());
        api.bind(&socket_3, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_3))
            .expect("bind should succeed");
        api.connect(&socket_3, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_PORT_2)
            .expect("connect socket 3");

        let mut results = Vec::new();

        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::DstPort(BoundPortMatcher::Bound(PortMatcher {
                    range: REMOTE_PORT_1.get()..=REMOTE_PORT_1.get(),
                    invert: false,
                })),
            )),
            &mut results,
            false,
        );

        results.sort_by(|a, b| a.cookie.cmp(&b.cookie));
        let mut expected = vec![
            TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::SynSent,
                tuple: TcpSocketDiagnosticTuple::Connected {
                    src_addr: I::TEST_ADDRS.local_ip.get(),
                    src_port: LOCAL_PORT_1,
                    dst_addr: I::TEST_ADDRS.remote_ip.get(),
                    dst_port: REMOTE_PORT_1,
                },
                cookie: socket_1.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            },
            TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::SynSent,
                tuple: TcpSocketDiagnosticTuple::Connected {
                    src_addr: I::TEST_ADDRS.local_ip.get(),
                    src_port: LOCAL_PORT_2,
                    dst_addr: I::TEST_ADDRS.remote_ip.get(),
                    dst_port: REMOTE_PORT_1,
                },
                cookie: socket_2.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            },
        ];
        expected.sort_by(|a, b| a.cookie.cmp(&b.cookie));
        assert_eq!(results, expected);

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::DstPort(BoundPortMatcher::Bound(PortMatcher {
                    range: REMOTE_PORT_2.get()..=REMOTE_PORT_2.get(),
                    invert: false,
                })),
            )),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::SynSent,
                tuple: TcpSocketDiagnosticTuple::Connected {
                    src_addr: I::TEST_ADDRS.local_ip.get(),
                    src_port: LOCAL_PORT_3,
                    dst_addr: I::TEST_ADDRS.remote_ip.get(),
                    dst_port: REMOTE_PORT_2,
                },
                cookie: socket_3.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );
    }

    #[ip_test(I)]
    fn diagnostics_match_src_port<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");
        api.listen(&s, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        let mut results = Vec::new();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::SrcPort(BoundPortMatcher::Bound(PortMatcher {
                    range: LOCAL_PORT_1.get()..=LOCAL_PORT_1.get(),
                    invert: false,
                })),
            )),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound {
                    src_addr: Some(I::TEST_ADDRS.local_ip.get()),
                    src_port: LOCAL_PORT_1,
                },
                cookie: s.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::SrcPort(BoundPortMatcher::Bound(PortMatcher {
                    range: (LOCAL_PORT_1.get() + 1)..=(LOCAL_PORT_1.get() + 1),
                    invert: false,
                })),
            )),
            &mut results,
            false,
        );
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_dst_port<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");
        api.connect(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), LOCAL_PORT_2)
            .expect("connect should succeed");

        let mut results = Vec::new();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::DstPort(BoundPortMatcher::Bound(PortMatcher {
                    range: LOCAL_PORT_2.get()..=LOCAL_PORT_2.get(),
                    invert: false,
                })),
            )),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::SynSent,
                tuple: TcpSocketDiagnosticTuple::Connected {
                    src_addr: I::TEST_ADDRS.local_ip.get(),
                    src_port: LOCAL_PORT_1,
                    dst_addr: I::TEST_ADDRS.remote_ip.get(),
                    dst_port: LOCAL_PORT_2,
                },
                cookie: s.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::DstPort(BoundPortMatcher::Bound(PortMatcher {
                    range: (LOCAL_PORT_2.get() + 1)..=(LOCAL_PORT_2.get() + 1),
                    invert: false,
                })),
            )),
            &mut results,
            false,
        );
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_state<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        // Socket 1: LISTEN
        let listen_socket = api.create(Default::default());
        api.bind(
            &listen_socket,
            Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)),
            Some(LOCAL_PORT_1),
        )
        .expect("bind");
        api.listen(&listen_socket, NonZeroUsize::new(1).unwrap()).expect("listen");

        // Socket 2: SYN_SENT
        let syn_sent_socket = api.create(Default::default());
        api.bind(
            &syn_sent_socket,
            Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)),
            Some(LOCAL_PORT_2),
        )
        .expect("bind");
        // Connect to a remote address that won't respond immediately (since we don't step the
        // network).
        api.connect(
            &syn_sent_socket,
            Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
            LOCAL_PORT_3,
        )
        .expect("connect");

        let mut results = Vec::new();

        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(TcpSocketMatcher::State(
                TcpStateMatcher::LISTEN,
            ))),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound {
                    src_addr: Some(I::TEST_ADDRS.local_ip.get()),
                    src_port: LOCAL_PORT_1,
                },
                cookie: listen_socket.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(TcpSocketMatcher::State(
                TcpStateMatcher::SYN_SENT,
            ))),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::SynSent,
                tuple: TcpSocketDiagnosticTuple::Connected {
                    src_addr: I::TEST_ADDRS.local_ip.get(),
                    src_port: LOCAL_PORT_2,
                    dst_addr: I::TEST_ADDRS.remote_ip.get(),
                    dst_port: LOCAL_PORT_3,
                },
                cookie: syn_sent_socket.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(TcpSocketMatcher::State(
                TcpStateMatcher::ESTABLISHED,
            ))),
            &mut results,
            false,
        );
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_src_addr_unbound<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        // Bound to wildcard address.
        let s1 = api.create(Default::default());
        api.bind(&s1, None, Some(LOCAL_PORT_1)).expect("bind should succeed");
        api.listen(&s1, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        // Bound to specific address.
        let s2 = api.create(Default::default());
        api.bind(&s2, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_2))
            .expect("bind should succeed");
        api.listen(&s2, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        let mut results = Vec::new();
        let matcher = match I::VERSION {
            net_types::ip::IpVersion::V4 => BoundAddressMatcherEither::Unbound,
            net_types::ip::IpVersion::V6 => BoundAddressMatcherEither::Unbound,
        };
        api.bound_sockets_diagnostics(&IpSocketMatcher::SrcAddr(matcher), &mut results, false);
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound { src_addr: None, src_port: LOCAL_PORT_1 },
                cookie: s1.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        let matcher = I::map_ip_in(
            I::TEST_ADDRS.local_ip.get(),
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 32).unwrap(),
                    )),
                    invert: false,
                }))
            },
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 128).unwrap(),
                    )),
                    invert: false,
                }))
            },
        );
        api.bound_sockets_diagnostics(&IpSocketMatcher::SrcAddr(matcher), &mut results, false);
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound {
                    src_addr: Some(I::TEST_ADDRS.local_ip.get()),
                    src_port: LOCAL_PORT_2,
                },
                cookie: s2.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );
    }

    #[ip_test(I)]
    fn diagnostics_match_dst_addr_unbound<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        // Not connected, so no destination address.
        let s1 = api.create(Default::default());
        api.bind(&s1, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");
        api.listen(&s1, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        // Connected, so has a destination address.
        let s2 = api.create(Default::default());
        api.bind(&s2, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_2))
            .expect("bind should succeed");
        api.connect(&s2, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_PORT_1)
            .expect("connect should succeed");

        let mut results = Vec::new();
        let matcher = match I::VERSION {
            net_types::ip::IpVersion::V4 => BoundAddressMatcherEither::Unbound,
            net_types::ip::IpVersion::V6 => BoundAddressMatcherEither::Unbound,
        };
        api.bound_sockets_diagnostics(&IpSocketMatcher::DstAddr(matcher), &mut results, false);
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound {
                    src_addr: Some(I::TEST_ADDRS.local_ip.get()),
                    src_port: LOCAL_PORT_1
                },
                cookie: s1.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        let matcher = I::map_ip_in(
            I::TEST_ADDRS.remote_ip.get(),
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V4(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 32).unwrap(),
                    )),
                    invert: false,
                }))
            },
            |addr| {
                BoundAddressMatcherEither::Bound(AddressMatcherEither::V6(AddressMatcher {
                    matcher: AddressMatcherType::Subnet(SubnetMatcher(
                        Subnet::new(addr, 128).unwrap(),
                    )),
                    invert: false,
                }))
            },
        );
        api.bound_sockets_diagnostics(&IpSocketMatcher::DstAddr(matcher), &mut results, false);
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::SynSent,
                tuple: TcpSocketDiagnosticTuple::Connected {
                    src_addr: I::TEST_ADDRS.local_ip.get(),
                    src_port: LOCAL_PORT_2,
                    dst_addr: I::TEST_ADDRS.remote_ip.get(),
                    dst_port: REMOTE_PORT_1,
                },
                cookie: s2.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );
    }

    #[ip_test(I)]
    fn diagnostics_match_src_port_unbound<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        let s1 = api.create(Default::default());
        api.bind(&s1, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");
        api.listen(&s1, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        let mut results = Vec::new();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::SrcPort(BoundPortMatcher::Unbound),
            )),
            &mut results,
            false,
        );
        // All visible TCP sockets have a source port.
        assert_eq!(results, Vec::new());
    }

    #[ip_test(I)]
    fn diagnostics_match_dst_port_unbound<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
            TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();

        // Not connected, so no destination port.
        let s1 = api.create(Default::default());
        api.bind(&s1, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");
        api.listen(&s1, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");

        // Connected, so has a destination port.
        let s2 = api.create(Default::default());
        api.bind(&s2, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_2))
            .expect("bind should succeed");
        api.connect(&s2, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_PORT_1)
            .expect("connect should succeed");

        let mut results = Vec::new();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::DstPort(BoundPortMatcher::Unbound),
            )),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::Listen,
                tuple: TcpSocketDiagnosticTuple::Bound {
                    src_addr: Some(I::TEST_ADDRS.local_ip.get()),
                    src_port: LOCAL_PORT_1
                },
                cookie: s1.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );

        results.clear();
        api.bound_sockets_diagnostics(
            &IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
                TcpSocketMatcher::DstPort(BoundPortMatcher::Bound(PortMatcher {
                    range: REMOTE_PORT_1.get()..=REMOTE_PORT_1.get(),
                    invert: false,
                })),
            )),
            &mut results,
            false,
        );
        assert_eq!(
            results,
            vec![TcpSocketDiagnostics {
                state_machine: netstack3_base::TcpSocketState::SynSent,
                tuple: TcpSocketDiagnosticTuple::Connected {
                    src_addr: I::TEST_ADDRS.local_ip.get(),
                    src_port: LOCAL_PORT_2,
                    dst_addr: I::TEST_ADDRS.remote_ip.get(),
                    dst_port: REMOTE_PORT_1,
                },
                cookie: s2.socket_cookie(),
                marks: Marks::default(),
                tcp_info: None,
            }]
        );
    }

    const LOCAL: &'static str = "local";
    const REMOTE: &'static str = "remote";

    #[ip_test(I)]
    fn disconnect_connected<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();

        let mut net = FakeTcpNetworkSpec::new_network(
            [
                (
                    LOCAL,
                    TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
                        I::TEST_ADDRS.local_ip,
                        I::TEST_ADDRS.remote_ip,
                    )),
                ),
                (
                    REMOTE,
                    TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
                        I::TEST_ADDRS.remote_ip,
                        I::TEST_ADDRS.local_ip,
                    )),
                ),
            ],
            |net, meta| {
                if net == LOCAL {
                    alloc::vec![(REMOTE, meta, None)]
                } else {
                    alloc::vec![(LOCAL, meta, None)]
                }
            },
        );

        let client_socket = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let s: TcpSocketId<I, _, _> = api.create(Default::default());
            // Set device so we can check that it's not cleared.
            api.set_device(&s, Some(FakeDeviceId)).expect("set device should succeed");
            api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
                .expect("bind should succeed");
            api.connect(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_PORT_1)
                .expect("can connect");
            s
        });
        let server_listener = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let s = api.create(Default::default());
            api.bind(&s, None, Some(REMOTE_PORT_1)).expect("failed to bind the server socket");
            api.listen(&s, NonZeroUsize::MIN).expect("can listen");
            s
        });

        net.run_until_idle();

        let server_socket = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api();
            let (server_connection, _addr, _buffers) =
                api.accept(&server_listener).expect("connection is waiting");
            server_connection
        });

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let count = api.disconnect_bound(&IpSocketMatcher::Cookie(SocketCookieMatcher {
                cookie: client_socket.socket_cookie().export_value(),
                invert: false,
            }));
            assert_eq!(count, 1);

            ctx.core_ctx.with_socket(&client_socket, |s| {
                let conn = assert_matches!(
                    &s.socket_state,
                    TcpSocketStateInner::Connected {conn, ..} => conn
                );

                let info = I::get_conn_info(conn);
                assert_eq!(info.local_addr.ip.addr().get(), I::TEST_ADDRS.local_ip.get());
                assert_eq!(info.remote_addr.ip.addr().get(), I::TEST_ADDRS.remote_ip.get());
                assert_eq!(info.local_addr.port, LOCAL_PORT_1);
                assert_eq!(info.remote_addr.port, REMOTE_PORT_1);
                assert_eq!(info.device, Some(FakeDeviceId.downgrade()));
            });
        });

        // Deliver the RST from `client_socket` to `server_socket`.
        net.run_until_idle();

        net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api();
            assert_matches!(
                api.get_socket_error(&server_socket),
                Some(ConnectionError::ConnectionReset)
            );
        });

        // Trying to connect to the same remote will "succeed" because the
        // connect call is idempotent. However, the socket thinks it's already
        // connected so no SYN will be sent. This is done to align with the
        // implementation of TcpApi::shutdown.

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            assert_matches!(
                api.connect(
                    &client_socket,
                    Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
                    REMOTE_PORT_1
                ),
                Ok(())
            );
        });
        net.run_until_idle();
        net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api();
            assert_matches!(api.accept(&server_listener), Err(AcceptError::WouldBlock));
        });

        // Another socket can be created with the exact same tuple as the
        // disconnected socket. This matches Linux behavior.
        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let s: TcpSocketId<I, _, _> = api.create(Default::default());
            api.set_device(&s, Some(FakeDeviceId)).expect("set device should succeed");
            api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
                .expect("bind should succeed");
            api.connect(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_PORT_1)
                .expect("can connect");

            // Close both sockets to ensure nothing bad happens from them
            // having the same tuple.
            api.close(s);
            api.close(client_socket);
        });
    }

    #[ip_test(I)]
    fn disconnect_listener<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();

        let mut net = FakeTcpNetworkSpec::new_network(
            [
                (
                    LOCAL,
                    TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
                        I::TEST_ADDRS.local_ip,
                        I::TEST_ADDRS.remote_ip,
                    )),
                ),
                (
                    REMOTE,
                    TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
                        I::TEST_ADDRS.remote_ip,
                        I::TEST_ADDRS.local_ip,
                    )),
                ),
            ],
            |net, meta| {
                if net == LOCAL {
                    alloc::vec![(REMOTE, meta, None)]
                } else {
                    alloc::vec![(LOCAL, meta, None)]
                }
            },
        );

        let client_accepted_socket = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let s: TcpSocketId<I, _, _> = api.create(Default::default());
            api.connect(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_PORT_1)
                .expect("can connect");
            s
        });
        let client_unaccepted_socket = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let s: TcpSocketId<I, _, _> = api.create(Default::default());
            api.connect(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), REMOTE_PORT_1)
                .expect("can connect");
            s
        });
        let server_listener = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let s = api.create(Default::default());
            // Set device so we can check that it's not cleared.
            api.set_device(&s, Some(FakeDeviceId)).expect("set device should succeed");
            api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), Some(REMOTE_PORT_1))
                .expect("bind should succeed");
            api.listen(&s, NonZeroUsize::new(2).unwrap()).expect("listen should succeed");
            s
        });

        // Establish both connections.
        net.run_until_idle();

        net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api();
            let _: (TcpSocketId<_, _, _>, _, _) =
                api.accept(&server_listener).expect("connection is waiting");

            let count = api.disconnect_bound(&IpSocketMatcher::Cookie(SocketCookieMatcher {
                cookie: server_listener.socket_cookie().export_value(),
                invert: false,
            }));
            assert_eq!(count, 1);

            ctx.core_ctx.with_socket(&server_listener, |s| {
                let addr = assert_matches!(
                    &s.socket_state,
                    TcpSocketStateInner::Bound(BoundState { addr, .. }) => addr
                );
                let info = I::get_bound_info(addr);
                assert_eq!(
                    info.addr.expect("address is set").addr().get(),
                    I::TEST_ADDRS.remote_ip.get(),
                );
                assert_eq!(info.port, REMOTE_PORT_1);
                assert_eq!(info.device, Some(FakeDeviceId.downgrade()));
            });

            let mut api = ctx.tcp_api::<I>();
            api.listen(&server_listener, NonZeroUsize::new(1).unwrap())
                .expect("listen should succeed");
        });

        // Deliver the RSTs.
        net.run_until_idle();

        // Since the first socket was already accepted, it shouldn't have been
        // affected by the disconnection of the listener. However, the (pending)
        // second socket should have received an RST.
        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            assert_matches!(api.get_socket_error(&client_accepted_socket), None);
            assert_matches!(
                api.get_socket_error(&client_unaccepted_socket),
                Some(ConnectionError::ConnectionReset)
            );
        })
    }

    #[ip_test(I)]
    fn disconnect_bound<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();

        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        // Set device so we can check that it's not cleared.
        api.set_device(&s, Some(FakeDeviceId)).expect("set device should succeed");
        api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");

        let mut api = ctx.tcp_api::<I>();
        let count = api.disconnect_bound(&IpSocketMatcher::Cookie(SocketCookieMatcher {
            cookie: s.socket_cookie().export_value(),
            invert: false,
        }));
        assert_eq!(count, 1);

        ctx.core_ctx.with_socket(&s, |s| {
            let addr = assert_matches!(
                &s.socket_state,
                TcpSocketStateInner::Bound(BoundState { addr, .. }) => addr
            );
            let info = I::get_bound_info(addr);
            assert_eq!(
                info.addr.expect("address is set").addr().get(),
                I::TEST_ADDRS.local_ip.get(),
            );
            assert_eq!(info.port, LOCAL_PORT_1);
            assert_eq!(info.device, Some(FakeDeviceId.downgrade()));
        });
    }

    #[ip_test(I)]
    fn tcp_info_unbound<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        let info = api.get_tcp_info(&s);
        assert_eq!(
            info,
            TcpSocketInfo {
                state: netstack3_base::TcpSocketState::Close,
                ca_state: CongestionControlState::Open,
                rto: None,
                rtt: None,
                rtt_var: None,
                snd_ssthresh: 0,
                snd_cwnd: 0,
                retransmits: 0,
                last_ack_recv: None,
                segs_out: 0,
                segs_in: 0,
                last_data_sent: None,
                snd_mss: None,
                rcv_mss: None,
            }
        );
    }

    #[ip_test(I)]
    #[test_case(true, netstack3_base::TcpSocketState::Listen; "listen")]
    #[test_case(false, netstack3_base::TcpSocketState::Close; "bound")]
    fn tcp_info_bound<I: TcpTestIpExt>(
        should_listen: bool,
        expected_state: netstack3_base::TcpSocketState,
    ) where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();
        let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
            I::TEST_ADDRS.local_ip,
            I::TEST_ADDRS.remote_ip,
        ));
        let mut api = ctx.tcp_api::<I>();
        let s = api.create(Default::default());
        api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), Some(LOCAL_PORT_1))
            .expect("bind should succeed");
        if should_listen {
            api.listen(&s, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");
        }

        let info = api.get_tcp_info(&s);
        assert_eq!(
            info,
            TcpSocketInfo {
                state: expected_state,
                ca_state: CongestionControlState::Open,
                rto: None,
                rtt: None,
                rtt_var: None,
                snd_ssthresh: 0,
                snd_cwnd: 0,
                retransmits: 0,
                last_ack_recv: None,
                segs_out: 0,
                segs_in: 0,
                last_data_sent: None,
                snd_mss: None,
                rcv_mss: None,
            }
        );
    }

    #[ip_test(I)]
    fn tcp_info_connected<I: TcpTestIpExt>()
    where
        TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
                I,
                TcpBindingsCtx<FakeDeviceId>,
                SingleStackConverter = I::SingleStackConverter,
                DualStackConverter = I::DualStackConverter,
            >,
    {
        set_logger_for_test();

        let mut net = FakeTcpNetworkSpec::new_network(
            [
                (
                    LOCAL,
                    TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
                        I::TEST_ADDRS.local_ip,
                        I::TEST_ADDRS.remote_ip,
                    )),
                ),
                (
                    REMOTE,
                    TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
                        I::TEST_ADDRS.remote_ip,
                        I::TEST_ADDRS.local_ip,
                    )),
                ),
            ],
            |net, meta| {
                if net == LOCAL {
                    vec![(REMOTE, meta, Some(Duration::from_millis(100)))]
                } else {
                    vec![(LOCAL, meta, Some(Duration::from_millis(100)))]
                }
            },
        );

        let client_socket = net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let s: TcpSocketId<I, _, _> = api.create(Default::default());
            s
        });

        let server_socket = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api::<I>();
            let s = api.create(Default::default());
            api.set_device(&s, Some(FakeDeviceId)).expect("set device should succeed");
            api.bind(&s, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), Some(REMOTE_PORT_1))
                .expect("bind should succeed");
            api.listen(&s, NonZeroUsize::new(1).unwrap()).expect("listen should succeed");
            s
        });

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            api.connect(
                &client_socket,
                Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
                REMOTE_PORT_1,
            )
            .expect("should connect");
        });

        net.run_until_idle();

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let data = [1u8; 10];
            api.with_send_buffer(&client_socket, |buf| {
                buf.enqueue_data(&data[..]);
            })
            .expect("send buffer should be available");
            api.do_send(&client_socket);
        });

        net.run_until_idle();

        let accepted_socket = net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api();
            let (accepted, _, _) = api.accept(&server_socket).expect("accept should succeed");

            // Send data back to client to ensure client receives a data segment
            // (populating last_segment_at/last_ack_recv).
            let data = [2u8; 10];
            api.with_send_buffer(&accepted, |buf| {
                buf.enqueue_data(&data[..]);
            })
            .expect("send buffer should be available");
            api.do_send(&accepted);

            accepted
        });

        net.run_until_idle();

        net.with_context(LOCAL, |ctx| {
            let mut api = ctx.tcp_api();
            let info = api.get_tcp_info(&client_socket);

            assert_matches!(
                info,
                TcpSocketInfo {
                    state: netstack3_base::TcpSocketState::Established,
                    ca_state: CongestionControlState::Open,
                    retransmits: 0,
                    snd_ssthresh: u32::MAX,
                    segs_out,
                    segs_in,
                    snd_cwnd,
                    rto: Some(rto),
                    rtt: Some(rtt),
                    rtt_var: Some(rtt_var),
                    last_ack_recv: Some(last_ack_recv),
                    last_data_sent: Some(last_data_sent),
                    snd_mss: _,
                    rcv_mss: _,
                } => {
                    assert_eq!(segs_out, 4);
                    assert_eq!(segs_in, 3);
                    assert_gt!(snd_cwnd, 0);
                    assert_eq!(rto, Duration::from_millis(500));
                    assert_eq!(rtt, Duration::from_millis(200));
                    assert_eq!(rtt_var, Duration::from_millis(75));
                    assert_eq!(last_ack_recv, FakeInstant::from(Duration::from_millis(1800)));
                    assert_eq!(last_data_sent, FakeInstant::from(Duration::from_millis(1100)));
                }
            );
        });

        net.with_context(REMOTE, |ctx| {
            let mut api = ctx.tcp_api();
            let info = api.get_tcp_info(&accepted_socket);

            assert_matches!(
                info,
                TcpSocketInfo {
                    state: netstack3_base::TcpSocketState::Established,
                    ca_state: CongestionControlState::Open,
                    retransmits: 0,
                    snd_ssthresh: u32::MAX,
                    segs_out,
                    segs_in,
                    snd_cwnd,
                    rto: Some(rto),
                    rtt: Some(rtt),
                    rtt_var: Some(rtt_var),
                    last_ack_recv: Some(last_ack_recv),
                    last_data_sent: Some(last_data_sent),
                    snd_mss: _,
                    rcv_mss: _,
                } => {
                    assert_eq!(segs_out, 2);
                    assert_eq!(segs_in, 3);
                    assert_gt!(snd_cwnd, 0);
                    assert_eq!(rto, Duration::from_millis(500));
                    assert_eq!(rtt, Duration::from_millis(200));
                    assert_eq!(rtt_var, Duration::from_millis(75));
                    assert_eq!(last_ack_recv, FakeInstant::from(Duration::from_millis(1200)));
                    assert_eq!(last_data_sent, FakeInstant::from(Duration::from_millis(1700)));
                }
            );
        });
    }
}
