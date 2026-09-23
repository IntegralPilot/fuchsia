// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Debug;
use std::num::NonZeroU64;
use std::ops::RangeInclusive;
use std::sync::Arc;

use fidl_fuchsia_ebpf as febpf;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_filter_ext::Matchers;
use fidl_fuchsia_net_matchers as fnet_matchers;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use net_types::ip::{Ip, IpVersion};
use netemul::TestInterface;

use crate::ip_hooks::{
    IcmpSocket, Interfaces, IrrelevantToTest, Ports, SocketType, Subnets, TcpSocket, UdpSocket,
};

pub(crate) trait MatcherState {
    /// If possible, verifies that the matcher was executed with the specified
    /// parameters.
    fn verify_matched(
        &self,
        interface: &TestInterface<'_>,
        ip_version: IpVersion,
        expected_mark: u32,
        expected_uid: u32,
        expected_cookie: u64,
    );

    /// If the matcher was executed, verifies that the matcher was executed with the specified
    /// parameters.
    fn verify_maybe_matched(
        &self,
        interface: &TestInterface<'_>,
        ip_version: IpVersion,
        expected_mark: u32,
        expected_uid: u32,
        expected_cookie: Option<u64>,
    );

    // If possible, verifies that the matcher was not executed.
    fn verify_not_matched(&self);
}

/// `MatcherState` implementation that's used for stateless matchers, i.e. all
/// but eBPF matchers.
pub(crate) struct NoState;

impl MatcherState for NoState {
    fn verify_matched(
        &self,
        _interface: &TestInterface<'_>,
        _ip_version: IpVersion,
        _expected_mark: u32,
        _expected_uid: u32,
        _expected_cookie: u64,
    ) {
    }
    fn verify_maybe_matched(
        &self,
        _interface: &TestInterface<'_>,
        _ip_version: IpVersion,
        _expected_mark: u32,
        _expected_uid: u32,
        _expected_cookie: Option<u64>,
    ) {
    }
    fn verify_not_matched(&self) {}
}

pub(crate) trait MatcherDefinition: Clone + Debug {
    type State: MatcherState;
    type SocketType: SocketType;

    async fn create_matcher<I: Ip>(
        &self,
        interfaces: Interfaces<'_>,
        subnets: Subnets,
        ports: Ports,
    ) -> Matcher<Self::State>;
}

pub(crate) struct Matcher<S> {
    pub ebpf_program: Option<(febpf::ProgramHandle, febpf::VerifiedProgram)>,
    pub fidl_def: Matchers,
    pub state: S,
}

impl Matcher<NoState> {
    pub(crate) fn new(fidl_def: Matchers) -> Self {
        Matcher { ebpf_program: None, fidl_def, state: NoState }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Inversion {
    Default,
    InverseMatch,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AllTraffic;

impl MatcherDefinition for AllTraffic {
    type State = NoState;
    type SocketType = IrrelevantToTest;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        _subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        Matcher::new(Matchers::default())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InterfaceId;

impl MatcherDefinition for InterfaceId {
    type State = NoState;
    type SocketType = IrrelevantToTest;

    async fn create_matcher<I: Ip>(
        &self,
        interfaces: Interfaces<'_>,
        _subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        let Interfaces { ingress, egress } = interfaces;
        Matcher::new(Matchers {
            in_interface: ingress.map(|interface| {
                fnet_matchers_ext::Interface::Id(
                    NonZeroU64::new(interface.id()).expect("interface ID should be nonzero"),
                )
            }),
            out_interface: egress.map(|interface| {
                fnet_matchers_ext::Interface::Id(
                    NonZeroU64::new(interface.id()).expect("interface ID should be nonzero"),
                )
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InterfaceName;

impl MatcherDefinition for InterfaceName {
    type State = NoState;
    type SocketType = IrrelevantToTest;

    async fn create_matcher<I: Ip>(
        &self,
        interfaces: Interfaces<'_>,
        _subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        async fn get_interface_name(
            interface: &netemul::TestInterface<'_>,
        ) -> fnet_matchers_ext::Interface {
            fnet_matchers_ext::Interface::Name(
                interface.get_interface_name().await.expect("get interface name"),
            )
        }

        let Interfaces { ingress, egress } = interfaces;
        let in_interface = match ingress {
            Some(ingress) => Some(get_interface_name(ingress).await),
            None => None,
        };
        let out_interface = match egress {
            Some(egress) => Some(get_interface_name(egress).await),
            None => None,
        };
        Matcher::new(Matchers { in_interface, out_interface, ..Default::default() })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InterfaceDeviceClass;

impl MatcherDefinition for InterfaceDeviceClass {
    type State = NoState;
    type SocketType = IrrelevantToTest;

    async fn create_matcher<I: Ip>(
        &self,
        interfaces: Interfaces<'_>,
        _subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        async fn get_port_class(
            interface: &netemul::TestInterface<'_>,
        ) -> fnet_matchers_ext::Interface {
            fnet_matchers_ext::Interface::PortClass(
                interface.get_port_class().await.expect("get port class").into(),
            )
        }

        let Interfaces { ingress, egress } = interfaces;
        let in_interface = match ingress {
            Some(ingress) => Some(get_port_class(ingress).await),
            None => None,
        };
        let out_interface = match egress {
            Some(egress) => Some(get_port_class(egress).await),
            None => None,
        };
        Matcher::new(Matchers { in_interface, out_interface, ..Default::default() })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SrcAddressSubnet(pub(crate) Inversion);

impl MatcherDefinition for SrcAddressSubnet {
    type State = NoState;
    type SocketType = IrrelevantToTest;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        let Self(inversion) = self;
        let Subnets { src, other, dst: _ } = subnets;

        Matcher::new(Matchers {
            src_addr: Some(match inversion {
                Inversion::Default => fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                        fnet_ext::apply_subnet_mask(src)
                            .try_into()
                            .expect("subnet should be valid"),
                    ),
                    invert: false,
                },
                Inversion::InverseMatch => fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                        other.try_into().expect("subnet should be valid"),
                    ),
                    invert: true,
                },
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SrcAddressRange(pub(crate) Inversion);

impl MatcherDefinition for SrcAddressRange {
    type State = NoState;
    type SocketType = IrrelevantToTest;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        let Self(inversion) = self;
        let Subnets { src, other, dst: _ } = subnets;

        Matcher::new(Matchers {
            src_addr: Some(match inversion {
                Inversion::Default => fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Range(
                        fnet_matchers::AddressRange { start: src.addr, end: src.addr }
                            .try_into()
                            .expect("address range should be valid"),
                    ),
                    invert: false,
                },
                Inversion::InverseMatch => fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Range(
                        fnet_matchers::AddressRange { start: other.addr, end: other.addr }
                            .try_into()
                            .expect("address range should be valid"),
                    ),
                    invert: true,
                },
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DstAddressSubnet(pub(crate) Inversion);

impl MatcherDefinition for DstAddressSubnet {
    type State = NoState;
    type SocketType = IrrelevantToTest;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        let Self(inversion) = self;
        let Subnets { dst, other, src: _ } = subnets;

        Matcher::new(Matchers {
            dst_addr: Some(match inversion {
                Inversion::Default => fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                        fnet_ext::apply_subnet_mask(dst)
                            .try_into()
                            .expect("subnet should be valid"),
                    ),
                    invert: false,
                },
                Inversion::InverseMatch => fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                        other.try_into().expect("subnet should be valid"),
                    ),
                    invert: true,
                },
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DstAddressRange(pub(crate) Inversion);

impl MatcherDefinition for DstAddressRange {
    type State = NoState;
    type SocketType = IrrelevantToTest;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        let Self(inversion) = self;
        let Subnets { dst, other, src: _ } = subnets;

        Matcher::new(Matchers {
            dst_addr: Some(match inversion {
                Inversion::Default => fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Range(
                        fnet_matchers::AddressRange { start: dst.addr, end: dst.addr }
                            .try_into()
                            .expect("address range should be valid"),
                    ),
                    invert: false,
                },
                Inversion::InverseMatch => fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Range(
                        fnet_matchers::AddressRange { start: other.addr, end: other.addr }
                            .try_into()
                            .expect("address range should be valid"),
                    ),
                    invert: true,
                },
            }),
            ..Default::default()
        })
    }
}

fn unique_ephemeral_port(exclude: &[u16]) -> u16 {
    // RFC 6335 section 6 defines 49152-65535 as the ephemeral port range.
    const RANGE: RangeInclusive<u16> = 49152..=65535;
    for port in RANGE {
        if !exclude.contains(&port) {
            return port;
        }
    }
    panic!("could not find an available port in the ephemeral range")
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Tcp;

impl MatcherDefinition for Tcp {
    type State = NoState;
    type SocketType = TcpSocket;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        _subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        Matcher::new(Matchers {
            transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                src_port: None,
                dst_port: None,
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TcpSrcPort(pub(crate) Inversion);

impl MatcherDefinition for TcpSrcPort {
    type State = NoState;
    type SocketType = TcpSocket;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        _subnets: Subnets,
        ports: Ports,
    ) -> Matcher<NoState> {
        let Self(inversion) = self;
        let Ports { src, dst } = ports;

        Matcher::new(Matchers {
            transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                src_port: Some(match inversion {
                    Inversion::Default => {
                        fnet_matchers_ext::Port::new(src, src, /* invert */ false)
                            .expect("should be valid port range")
                    }
                    Inversion::InverseMatch => {
                        let port = unique_ephemeral_port(&[src, dst]);
                        fnet_matchers_ext::Port::new(port, port, /* invert */ true)
                            .expect("should be valid port range")
                    }
                }),
                dst_port: None,
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TcpDstPort(pub(crate) Inversion);

impl MatcherDefinition for TcpDstPort {
    type State = NoState;
    type SocketType = TcpSocket;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        _subnets: Subnets,
        ports: Ports,
    ) -> Matcher<NoState> {
        let Self(inversion) = self;
        let Ports { src, dst } = ports;

        Matcher::new(Matchers {
            transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                src_port: None,
                dst_port: Some(match inversion {
                    Inversion::Default => {
                        fnet_matchers_ext::Port::new(dst, dst, /* invert */ false)
                            .expect("should be valid port range")
                    }
                    Inversion::InverseMatch => {
                        let port = unique_ephemeral_port(&[src, dst]);
                        fnet_matchers_ext::Port::new(port, port, /* invert */ true)
                            .expect("should be valid port range")
                    }
                }),
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Udp;

impl MatcherDefinition for Udp {
    type State = NoState;
    type SocketType = UdpSocket;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        _subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        Matcher::new(Matchers {
            transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                src_port: None,
                dst_port: None,
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UdpSrcPort(pub(crate) Inversion);

impl MatcherDefinition for UdpSrcPort {
    type State = NoState;
    type SocketType = UdpSocket;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        _subnets: Subnets,
        ports: Ports,
    ) -> Matcher<NoState> {
        let Self(inversion) = self;
        let Ports { src, dst } = ports;

        Matcher::new(Matchers {
            transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                src_port: Some(match inversion {
                    Inversion::Default => {
                        fnet_matchers_ext::Port::new(src, src, /* invert */ false)
                            .expect("should be valid port range")
                    }
                    Inversion::InverseMatch => {
                        let port = unique_ephemeral_port(&[src, dst]);
                        fnet_matchers_ext::Port::new(port, port, /* invert */ true)
                            .expect("should be valid port range")
                    }
                }),
                dst_port: None,
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UdpDstPort(pub(crate) Inversion);

impl MatcherDefinition for UdpDstPort {
    type State = NoState;
    type SocketType = UdpSocket;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        _subnets: Subnets,
        ports: Ports,
    ) -> Matcher<NoState> {
        let Self(inversion) = self;
        let Ports { src, dst } = ports;

        Matcher::new(Matchers {
            transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                src_port: None,
                dst_port: Some(match inversion {
                    Inversion::Default => {
                        fnet_matchers_ext::Port::new(dst, dst, /* invert */ false)
                            .expect("should be valid port range")
                    }
                    Inversion::InverseMatch => {
                        let port = unique_ephemeral_port(&[src, dst]);
                        fnet_matchers_ext::Port::new(port, port, /* invert */ true)
                            .expect("should be valid port range")
                    }
                }),
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Icmp;

impl MatcherDefinition for Icmp {
    type State = NoState;
    type SocketType = IcmpSocket;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        _subnets: Subnets,
        _ports: Ports,
    ) -> Matcher<NoState> {
        Matcher::new(Matchers {
            transport_protocol: Some({
                match I::VERSION {
                    IpVersion::V4 => fnet_matchers_ext::TransportProtocol::Icmp,
                    IpVersion::V6 => fnet_matchers_ext::TransportProtocol::Icmpv6,
                }
            }),
            ..Default::default()
        })
    }
}

pub(crate) struct EbpfMatcherState {
    program: ebpf_test_util::TestProgram,
}

impl MatcherState for EbpfMatcherState {
    fn verify_matched(
        &self,
        interface: &TestInterface<'_>,
        ip_version: IpVersion,
        expected_mark: u32,
        expected_uid: u32,
        expected_cookie: u64,
    ) {
        let result = self.program.read_test_result();
        assert_eq!(
            result.ether_type,
            u32::from(u16::from(packet_formats::ethernet::EtherType::from_ip_version(ip_version)))
        );
        assert_eq!(result.ip_proto, u8::from(packet_formats::ip::IpProto::Udp));
        assert_eq!(result.ifindex, u32::try_from(interface.id()).unwrap());
        assert_eq!(result.mark, expected_mark);
        assert_eq!(result.uid, expected_uid);
        assert_eq!(result.cookie, expected_cookie);
    }

    fn verify_maybe_matched(
        &self,
        interface: &TestInterface<'_>,
        ip_version: IpVersion,
        expected_mark: u32,
        expected_uid: u32,
        expected_cookie: Option<u64>,
    ) {
        let result = self.program.read_test_result();
        if result.ether_type == 0 {
            return;
        }

        assert_eq!(
            result.ether_type,
            u32::from(u16::from(packet_formats::ethernet::EtherType::from_ip_version(ip_version)))
        );
        assert_eq!(result.ip_proto, u8::from(packet_formats::ip::IpProto::Udp));
        assert_eq!(result.ifindex, u32::try_from(interface.id()).unwrap());
        assert_eq!(result.mark, expected_mark);
        assert_eq!(result.uid, expected_uid);
        if let Some(cookie) = expected_cookie {
            assert_eq!(result.cookie, cookie);
        }
    }

    fn verify_not_matched(&self) {
        let result = self.program.read_test_result();
        assert_eq!(result.ether_type, 0);
        assert_eq!(result.ip_proto, 0);
    }
}

#[derive(Clone)]
pub(crate) struct EbpfMatcher {
    program: Arc<ebpf_test_util::TestProgramDefinition>,
}

impl Debug for EbpfMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EbpfMatcher").finish()
    }
}

impl EbpfMatcher {
    pub(crate) fn new() -> Self {
        Self {
            program: Arc::new(ebpf_test_util::TestProgramDefinition::load(
                ebpf_api::ProgramType::SocketFilter,
            )),
        }
    }
}

impl MatcherDefinition for EbpfMatcher {
    type State = EbpfMatcherState;
    type SocketType = UdpSocket;

    async fn create_matcher<I: Ip>(
        &self,
        _interfaces: Interfaces<'_>,
        _subnets: Subnets,
        ports: Ports,
    ) -> Matcher<EbpfMatcherState> {
        let program = self.program.instantiate();
        program.write_test_config(ebpf_test_util::TestConfig {
            src_port: ports.src,
            dst_port: ports.dst,
        });
        Matcher {
            ebpf_program: Some((program.get_program_handle(), program.get_fidl_program())),
            fidl_def: Matchers {
                ebpf_program: Some(program.get_program_id()),
                ..Default::default()
            },
            state: EbpfMatcherState { program },
        }
    }
}
