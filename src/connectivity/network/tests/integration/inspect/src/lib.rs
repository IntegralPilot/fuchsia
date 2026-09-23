// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]
// Needed for invocations of the `assert_data_tree` macro.
#![recursion_limit = "256"]

use std::collections::HashMap;
use std::convert::TryFrom as _;
use std::fs::File;
use std::io::BufReader;
use std::mem::MaybeUninit;
use std::num::{NonZeroU16, NonZeroU64};
use std::os::fd::AsFd;
use std::time::Duration;

use assert_matches::assert_matches;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext::IntoExt as _;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext as fnet_filter_ext;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_multicast_admin as fnet_multicast_admin;
use fidl_fuchsia_net_multicast_ext::{
    self as fnet_multicast_ext, FidlMulticastAdminIpExt, TableControllerProxy,
};
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_netemul as fnetemul;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_packet as fposix_socket_packet;
use fidl_fuchsia_posix_socket_raw as fposix_socket_raw;
use fnet_filter_ext::{
    Action, Change, Controller, ControllerId, Domain, InstalledIpRoutine, InstalledNatRoutine,
    IpHook, Matchers, Namespace, NamespaceId, NatHook, PortRange, Resource, Routine, RoutineId,
    RoutineType, Rule, RuleId,
};
use futures::StreamExt as _;
use itertools::Itertools as _;
use net_declare::{
    fidl_mac, fidl_subnet, net_ip_v4, net_ip_v6, net_subnet_v4, std_ip_v4, std_ip_v6,
};
use net_types::ethernet::Mac;
use net_types::ip::{Ip, IpAddress, IpVersion, Ipv4, Ipv4Addr, Ipv6};
use net_types::{AddrAndPortFormatter, Witness as _};
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack3, NetstackVersion, TestSandboxExt as _,
};
use netstack_testing_common::{constants, get_inspect_data};
use netstack_testing_macros::netstack_test;
use packet::{
    NestableSerializer as _, NoOpSerializationContext, ParseBuffer as _, Serializer as _,
};
use packet_formats::arp::{ArpOp, ArpPacket};
use packet_formats::ethernet::testutil::{ETHERNET_HDR_LEN_NO_TAG, ETHERNET_MIN_BODY_LEN_NO_TAG};
use packet_formats::ethernet::{
    EtherType, EthernetFrame, EthernetFrameBuilder, EthernetFrameLengthCheck,
};
use packet_formats::ip::IpProto;
use packet_formats::ipv4::Ipv4PacketBuilder;
use packet_formats::udp::UdpPacketBuilder;
use regex::Regex;
use sampler_config::runtime::ProjectConfig;
use test_case::test_case;

enum TcpSocketState {
    Unbound,
    Bound,
    Listener,
    Connected,
}

const MARK_1: u32 = 100;
const MARK_2: u32 = 200;

async fn set_marks(socket: &impl AsFd) {
    let channel = fdio::clone_channel(socket).expect("failed to clone channel");
    let proxy = fposix_socket::BaseSocketProxy::new(fidl::AsyncChannel::from_channel(channel));
    proxy
        .set_mark(fnet::MarkDomain::Mark1, &fposix_socket::OptionalUint32::Value(MARK_1))
        .await
        .expect("fidl error")
        .expect("set mark");
    proxy
        .set_mark(fnet::MarkDomain::Mark2, &fposix_socket::OptionalUint32::Value(MARK_2))
        .await
        .expect("fidl error")
        .expect("set mark");
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(TcpSocketState::Unbound; "unbound")]
#[test_case(TcpSocketState::Bound; "bound")]
#[test_case(TcpSocketState::Listener; "listener")]
#[test_case(TcpSocketState::Connected; "connected")]
async fn inspect_tcp_sockets<I: Ip>(name: &str, socket_state: TcpSocketState) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let dev = realm.join_network(&network, "dev").await.expect("join network");
    let link_local =
        netstack_testing_common::interfaces::wait_for_v6_ll(&interfaces_state, dev.id())
            .await
            .expect("wait for v6 link local");
    let scope = dev.id().try_into().unwrap();

    // Ensure ns3 has started and that there is a Socket to collect inspect data about.
    const LOCAL_PORT: u16 = 8080;
    const REMOTE_PORT: u16 = 9999;
    const BACKLOG: u64 = 123;

    let (domain, local_addr) = match (I::VERSION, &socket_state) {
        (IpVersion::V4, TcpSocketState::Unbound) => (fposix_socket::Domain::Ipv4, None),
        // NB: For bound sockets, use the any address, which guards against
        // a regression where the any address is shown as "[NOT BOUND]".
        (IpVersion::V4, TcpSocketState::Bound) => (
            fposix_socket::Domain::Ipv4,
            Some(std::net::SocketAddr::from((std_ip_v4!("0.0.0.0"), LOCAL_PORT))),
        ),
        (IpVersion::V4, TcpSocketState::Listener) | (IpVersion::V4, TcpSocketState::Connected) => {
            // NB: Ensure the address exists on the device so that we can bind
            // to it. This only applies to IPv4 since IPv6 is using the link
            // local address.
            dev.add_address(fidl_subnet!("192.0.2.1/24")).await.expect("add address");
            (
                fposix_socket::Domain::Ipv4,
                Some(std::net::SocketAddr::from((std_ip_v4!("192.0.2.1"), LOCAL_PORT))),
            )
        }
        (IpVersion::V6, TcpSocketState::Unbound) => (fposix_socket::Domain::Ipv6, None),
        // NB: For bound sockets, use the any address, which guards against
        // a regression where the any address is shown as "[NOT BOUND]".
        (IpVersion::V6, TcpSocketState::Bound) => (
            fposix_socket::Domain::Ipv6,
            Some(std::net::SocketAddr::from((std_ip_v6!("::"), LOCAL_PORT))),
        ),
        (IpVersion::V6, TcpSocketState::Listener) | (IpVersion::V6, TcpSocketState::Connected) => (
            fposix_socket::Domain::Ipv6,
            Some(std::net::SocketAddr::from(std::net::SocketAddrV6::new(
                link_local.into(),
                LOCAL_PORT,
                0,
                scope,
            ))),
        ),
    };

    let tcp_socket = realm
        .stream_socket(domain, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("create TCP socket");
    if let Some(local_addr) = local_addr {
        tcp_socket.bind(&local_addr.into()).expect("bind");
    }

    set_marks(&tcp_socket).await;

    match socket_state {
        TcpSocketState::Unbound | TcpSocketState::Bound => {}
        TcpSocketState::Listener => tcp_socket.listen(BACKLOG.try_into().unwrap()).expect("listen"),
        TcpSocketState::Connected => {
            let (default_subnet, peer_addr) = match I::VERSION {
                IpVersion::V4 => (
                    fidl_subnet!("0.0.0.0/0"),
                    std::net::SocketAddr::from(std::net::SocketAddrV4::new(
                        std_ip_v4!("192.0.2.2"),
                        REMOTE_PORT,
                    )),
                ),
                IpVersion::V6 => (
                    fidl_subnet!("::/0"),
                    std::net::SocketAddr::from(std::net::SocketAddrV6::new(
                        std_ip_v6!("2001:db8::2"),
                        REMOTE_PORT,
                        0,
                        0,
                    )),
                ),
            };
            // Add a default route to the device, allowing the connection to
            // begin. Note that because there is no peer end to accept the
            // connection we setup the socket as non-blocking, and give a large
            // connection timeout. This ensures the socket is still a "pending
            // connection" when we fetch the inspect data.
            dev.add_subnet_route(default_subnet).await.expect("add_default_route");
            tcp_socket.set_nonblocking(true).expect("set nonblocking");
            const DAY: Duration = Duration::from_secs(60 * 60 * 24);
            tcp_socket.set_tcp_user_timeout(Some(DAY)).expect("set timeout");
            let err = assert_matches!(tcp_socket.connect(&peer_addr.into()), Err(e) => e);
            assert_eq!(err.raw_os_error(), Some(libc::EINPROGRESS));
        }
    }

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);

    // NB: The sockets are keyed by an opaque debug identifier; get that here.
    let sockets = data.get_child("Sockets").unwrap();
    let sock_name = assert_matches!(&sockets.children[..], [socket] => socket.name.clone());

    match (I::VERSION, socket_state) {
        (IpVersion::V4, TcpSocketState::Unbound) => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                "Sockets": {
                    sock_name => {
                        LocalAddress: "[NOT BOUND]",
                        RemoteAddress: "[NOT CONNECTED]",
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: "TCP",
                        NetworkProtocol: "IPv4",
                        TcpInfo: {
                            CongestionControlState: "Open",
                            SndCwnd: 0u64,
                            SndSsthresh: 0u64,
                        },
                        Counters: {
                            PassiveConnectionOpenings: 0u64,
                            ActiveConnectionOpenings: 0u64,
                            FastRecovery: 0u64,
                            SackRecovery: 0u64,
                            LossRecovered: 0u64,
                            EstablishedClosed: 0u64,
                            EstablishedResets: 0u64,
                            EstablishedTimedout: 0u64,
                            "Rx": {
                                ReceivedSegmentsDispatched: 0u64,
                                ResetsReceived: 0u64,
                                SynsReceived: 0u64,
                                FinsReceived: 0u64,
                                DupAcks: 0u64,
                                "Errors": {
                                    ListenerQueueOverflow: 0u64,
                                    PassiveOpenNoRouteErrors: 0u64,
                                },
                            },
                            "Tx": {
                                SegmentsSent: 0u64,
                                ResetsSent: 0u64,
                                SynsSent: 0u64,
                                FinsSent: 0u64,
                                Timeouts: 0u64,
                                Retransmits: 0u64,
                                FastRetransmits: 0u64,
                                SackRetransmits: 0u64,
                                SlowStartRetransmits: 0u64,
                                "Errors": {
                                    SegmentSendErrors: 0u64,
                                    ActiveOpenNoRouteErrors: 0u64,
                                }
                            },
                            "Errors": {
                                FailedConnectionOpenings: 0u64,
                                FailedPortReservations: 0u64,
                            }
                        },
                    },
                }
            })
        }
        (IpVersion::V4, TcpSocketState::Bound) => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                "Sockets": {
                    sock_name => {
                        LocalAddress: format!("0.0.0.0:{LOCAL_PORT}"),
                        RemoteAddress: "[NOT CONNECTED]",
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: "TCP",
                        NetworkProtocol: "IPv4",
                        TcpInfo: {
                            CongestionControlState: "Open",
                            SndCwnd: 0u64,
                            SndSsthresh: 0u64,
                        },
                        Counters: {
                            PassiveConnectionOpenings: 0u64,
                            ActiveConnectionOpenings: 0u64,
                            FastRecovery: 0u64,
                            SackRecovery: 0u64,
                            LossRecovered: 0u64,
                            EstablishedClosed: 0u64,
                            EstablishedResets: 0u64,
                            EstablishedTimedout: 0u64,
                            "Rx": {
                                ReceivedSegmentsDispatched: 0u64,
                                ResetsReceived: 0u64,
                                SynsReceived: 0u64,
                                FinsReceived: 0u64,
                                DupAcks: 0u64,
                                "Errors": {
                                    ListenerQueueOverflow: 0u64,
                                    PassiveOpenNoRouteErrors: 0u64,
                                },
                            },
                            "Tx": {
                                SegmentsSent: 0u64,
                                ResetsSent: 0u64,
                                SynsSent: 0u64,
                                FinsSent: 0u64,
                                Timeouts: 0u64,
                                Retransmits: 0u64,
                                FastRetransmits: 0u64,
                                SackRetransmits: 0u64,
                                SlowStartRetransmits: 0u64,
                                "Errors": {
                                    SegmentSendErrors: 0u64,
                                    ActiveOpenNoRouteErrors: 0u64,
                                }
                            },
                            "Errors": {
                                FailedConnectionOpenings: 0u64,
                                FailedPortReservations: 0u64,
                            }
                        },
                    },
                }
            })
        }
        (IpVersion::V4, TcpSocketState::Listener) => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                "Sockets": {
                    sock_name => {
                        LocalAddress: format!("192.0.2.1:{LOCAL_PORT}"),
                        RemoteAddress: "[NOT CONNECTED]",
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: "TCP",
                        NetworkProtocol: "IPv4",
                        TcpInfo: {
                            CongestionControlState: "Open",
                            SndCwnd: 0u64,
                            SndSsthresh: 0u64,
                        },
                        AcceptQueue: {
                            BacklogSize: BACKLOG,
                            NumPending: 0u64,
                            NumReady: 0u64,
                            Contents: "{}",
                        },
                        Counters: {
                            PassiveConnectionOpenings: 0u64,
                            ActiveConnectionOpenings: 0u64,
                            FastRecovery: 0u64,
                            SackRecovery: 0u64,
                            LossRecovered: 0u64,
                            EstablishedClosed: 0u64,
                            EstablishedResets: 0u64,
                            EstablishedTimedout: 0u64,
                            "Rx": {
                                ReceivedSegmentsDispatched: 0u64,
                                ResetsReceived: 0u64,
                                SynsReceived: 0u64,
                                FinsReceived: 0u64,
                                DupAcks: 0u64,
                                "Errors": {
                                    ListenerQueueOverflow: 0u64,
                                    PassiveOpenNoRouteErrors: 0u64,
                                },
                            },
                            "Tx": {
                                SegmentsSent: 0u64,
                                ResetsSent: 0u64,
                                SynsSent: 0u64,
                                FinsSent: 0u64,
                                Timeouts: 0u64,
                                Retransmits: 0u64,
                                FastRetransmits: 0u64,
                                SackRetransmits: 0u64,
                                SlowStartRetransmits: 0u64,
                                "Errors": {
                                    SegmentSendErrors: 0u64,
                                    ActiveOpenNoRouteErrors: 0u64,
                                }
                            },
                            "Errors": {
                                FailedConnectionOpenings: 0u64,
                                FailedPortReservations: 0u64,
                            }
                        },
                    },
                }
            })
        }
        (IpVersion::V4, TcpSocketState::Connected) => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                "Sockets": {
                    sock_name => {
                        LocalAddress: format!("192.0.2.1:{LOCAL_PORT}"),
                        RemoteAddress: format!("192.0.2.2:{REMOTE_PORT}"),
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: "TCP",
                        NetworkProtocol: "IPv4",
                        State: "SynSent",
                        TcpInfo: {
                            CongestionControlState: "Open",
                            SndCwnd: 0u64,
                            SndSsthresh: 0u64,
                        },
                        Counters: {
                            PassiveConnectionOpenings: 0u64,
                            ActiveConnectionOpenings: 1u64,
                            FastRecovery: 0u64,
                            SackRecovery: 0u64,
                            LossRecovered: 0u64,
                            EstablishedClosed: 0u64,
                            EstablishedResets: 0u64,
                            EstablishedTimedout: 0u64,
                            "Rx": {
                                ReceivedSegmentsDispatched: 0u64,
                                ResetsReceived: 0u64,
                                SynsReceived: 0u64,
                                FinsReceived: 0u64,
                                DupAcks: 0u64,
                                "Errors": {
                                    ListenerQueueOverflow: 0u64,
                                    PassiveOpenNoRouteErrors: 0u64,
                                },
                            },
                            "Tx": {
                                SegmentsSent: diagnostics_assertions::NonZeroUintProperty,
                                ResetsSent: 0u64,
                                SynsSent: diagnostics_assertions::NonZeroUintProperty,
                                FinsSent: 0u64,
                                Timeouts: 0u64,
                                Retransmits: 0u64,
                                FastRetransmits: 0u64,
                                SackRetransmits: 0u64,
                                SlowStartRetransmits: 0u64,
                                "Errors": {
                                    SegmentSendErrors: 0u64,
                                    ActiveOpenNoRouteErrors: 0u64,
                                }
                            },
                            "Errors": {
                                FailedConnectionOpenings: 0u64,
                                FailedPortReservations: 0u64,
                            }
                        },
                    },
                }
            })
        }
        (IpVersion::V6, TcpSocketState::Unbound) => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                "Sockets": {
                    sock_name => {
                        LocalAddress: "[NOT BOUND]",
                        RemoteAddress: "[NOT CONNECTED]",
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: "TCP",
                        NetworkProtocol: "IPv6",
                        TcpInfo: {
                            CongestionControlState: "Open",
                            SndCwnd: 0u64,
                            SndSsthresh: 0u64,
                        },
                        Counters: {
                            PassiveConnectionOpenings: 0u64,
                            ActiveConnectionOpenings: 0u64,
                            FastRecovery: 0u64,
                            SackRecovery: 0u64,
                            LossRecovered: 0u64,
                            EstablishedClosed: 0u64,
                            EstablishedResets: 0u64,
                            EstablishedTimedout: 0u64,
                            "Rx": {
                                ReceivedSegmentsDispatched: 0u64,
                                ResetsReceived: 0u64,
                                SynsReceived: 0u64,
                                FinsReceived: 0u64,
                                DupAcks: 0u64,
                                "Errors": {
                                    ListenerQueueOverflow: 0u64,
                                    PassiveOpenNoRouteErrors: 0u64,
                                },
                            },
                            "Tx": {
                                SegmentsSent: 0u64,
                                ResetsSent: 0u64,
                                SynsSent: 0u64,
                                FinsSent: 0u64,
                                Timeouts: 0u64,
                                Retransmits: 0u64,
                                FastRetransmits: 0u64,
                                SackRetransmits: 0u64,
                                SlowStartRetransmits: 0u64,
                                "Errors": {
                                    SegmentSendErrors: 0u64,
                                    ActiveOpenNoRouteErrors: 0u64,
                                }
                            },
                            "Errors": {
                                FailedConnectionOpenings: 0u64,
                                FailedPortReservations: 0u64,
                            }
                        },
                    }
                }
            })
        }
        (IpVersion::V6, TcpSocketState::Bound) => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                "Sockets": {
                    sock_name => {
                        LocalAddress: format!("[::]:{LOCAL_PORT}"),
                        RemoteAddress: "[NOT CONNECTED]",
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: "TCP",
                        NetworkProtocol: "IPv6",
                        TcpInfo: {
                            CongestionControlState: "Open",
                            SndCwnd: 0u64,
                            SndSsthresh: 0u64,
                        },
                        Counters: {
                            PassiveConnectionOpenings: 0u64,
                            ActiveConnectionOpenings: 0u64,
                            FastRecovery: 0u64,
                            SackRecovery: 0u64,
                            LossRecovered: 0u64,
                            EstablishedClosed: 0u64,
                            EstablishedResets: 0u64,
                            EstablishedTimedout: 0u64,
                            "Rx": {
                                ReceivedSegmentsDispatched: 0u64,
                                ResetsReceived: 0u64,
                                SynsReceived: 0u64,
                                FinsReceived: 0u64,
                                DupAcks: 0u64,
                                "Errors": {
                                    ListenerQueueOverflow: 0u64,
                                    PassiveOpenNoRouteErrors: 0u64,
                                },
                            },
                            "Tx": {
                                SegmentsSent: 0u64,
                                ResetsSent: 0u64,
                                SynsSent: 0u64,
                                FinsSent: 0u64,
                                Timeouts: 0u64,
                                Retransmits: 0u64,
                                FastRetransmits: 0u64,
                                SackRetransmits: 0u64,
                                SlowStartRetransmits: 0u64,
                                "Errors": {
                                    SegmentSendErrors: 0u64,
                                    ActiveOpenNoRouteErrors: 0u64,
                                }
                            },
                            "Errors": {
                                FailedConnectionOpenings: 0u64,
                                FailedPortReservations: 0u64,
                            }
                        },
                    }
                }
            })
        }
        (IpVersion::V6, TcpSocketState::Listener) => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                "Sockets": {
                    sock_name => {
                        LocalAddress: format!("[{link_local}%{scope}]:{LOCAL_PORT}"),
                        RemoteAddress: "[NOT CONNECTED]",
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: "TCP",
                        NetworkProtocol: "IPv6",
                        TcpInfo: {
                            CongestionControlState: "Open",
                            SndCwnd: 0u64,
                            SndSsthresh: 0u64,
                        },
                        AcceptQueue: {
                            BacklogSize: BACKLOG,
                            NumPending: 0u64,
                            NumReady: 0u64,
                            Contents: "{}",
                        },
                        Counters: {
                            PassiveConnectionOpenings: 0u64,
                            ActiveConnectionOpenings: 0u64,
                            FastRecovery: 0u64,
                            SackRecovery: 0u64,
                            LossRecovered: 0u64,
                            EstablishedClosed: 0u64,
                            EstablishedResets: 0u64,
                            EstablishedTimedout: 0u64,
                            "Rx": {
                                ReceivedSegmentsDispatched: 0u64,
                                ResetsReceived: 0u64,
                                SynsReceived: 0u64,
                                FinsReceived: 0u64,
                                DupAcks: 0u64,
                                "Errors": {
                                    ListenerQueueOverflow: 0u64,
                                    PassiveOpenNoRouteErrors: 0u64,
                                },
                            },
                            "Tx": {
                                SegmentsSent: 0u64,
                                ResetsSent: 0u64,
                                SynsSent: 0u64,
                                FinsSent: 0u64,
                                Timeouts: 0u64,
                                Retransmits: 0u64,
                                FastRetransmits: 0u64,
                                SackRetransmits: 0u64,
                                SlowStartRetransmits: 0u64,
                                "Errors": {
                                    SegmentSendErrors: 0u64,
                                    ActiveOpenNoRouteErrors: 0u64,
                                }
                            },
                            "Errors": {
                                FailedConnectionOpenings: 0u64,
                                FailedPortReservations: 0u64,
                            }
                        },
                    }
                }
            })
        }
        (IpVersion::V6, TcpSocketState::Connected) => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                "Sockets": {
                    sock_name => {
                        LocalAddress: format!("[{link_local}%{scope}]:{LOCAL_PORT}"),
                        RemoteAddress: format!("[2001:db8::2]:{REMOTE_PORT}"),
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: "TCP",
                        NetworkProtocol: "IPv6",
                        State: "SynSent",
                        TcpInfo: {
                            CongestionControlState: "Open",
                            SndCwnd: 0u64,
                            SndSsthresh: 0u64,
                        },
                        Counters: {
                            PassiveConnectionOpenings: 0u64,
                            ActiveConnectionOpenings: 1u64,
                            FastRecovery: 0u64,
                            SackRecovery: 0u64,
                            LossRecovered: 0u64,
                            EstablishedClosed: 0u64,
                            EstablishedResets: 0u64,
                            EstablishedTimedout: 0u64,
                            "Rx": {
                                ReceivedSegmentsDispatched: 0u64,
                                ResetsReceived: 0u64,
                                SynsReceived: 0u64,
                                FinsReceived: 0u64,
                                DupAcks: 0u64,
                                "Errors": {
                                    ListenerQueueOverflow: 0u64,
                                    PassiveOpenNoRouteErrors: 0u64,
                                },
                            },
                            "Tx": {
                                SegmentsSent: diagnostics_assertions::NonZeroUintProperty,
                                ResetsSent: 0u64,
                                SynsSent: diagnostics_assertions::NonZeroUintProperty,
                                FinsSent: 0u64,
                                Timeouts: 0u64,
                                Retransmits: 0u64,
                                FastRetransmits: 0u64,
                                SackRetransmits: 0u64,
                                SlowStartRetransmits: 0u64,
                                "Errors": {
                                    SegmentSendErrors: 0u64,
                                    ActiveOpenNoRouteErrors: 0u64,
                                }
                            },
                            "Errors": {
                                FailedConnectionOpenings: 0u64,
                                FailedPortReservations: 0u64,
                            }
                        },
                    }
                }
            })
        }
    }
}

enum SocketState {
    Bound,
    Connected,
}

trait TestIpExt: net_types::ip::Ip {
    const DOMAIN: fposix_socket::Domain;
}

impl TestIpExt for Ipv4 {
    const DOMAIN: fposix_socket::Domain = fposix_socket::Domain::Ipv4;
}

impl TestIpExt for Ipv6 {
    const DOMAIN: fposix_socket::Domain = fposix_socket::Domain::Ipv6;
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(
    fposix_socket::DatagramSocketProtocol::Udp, SocketState::Bound;
    "udp_bound"
)]
#[test_case(
    fposix_socket::DatagramSocketProtocol::IcmpEcho, SocketState::Bound;
    "icmp_bound"
)]
#[test_case(
    fposix_socket::DatagramSocketProtocol::Udp, SocketState::Connected;
    "udp_connected"
)]
#[test_case(
    fposix_socket::DatagramSocketProtocol::IcmpEcho, SocketState::Connected;
    "icmp_connected"
)]
async fn inspect_datagram_sockets<I: TestIpExt>(
    name: &str,
    proto: fposix_socket::DatagramSocketProtocol,
    socket_state: SocketState,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");

    // Ensure ns3 has started and that there is a Socket to collect inspect data about.
    let socket = realm.datagram_socket(I::DOMAIN, proto).await.expect("create datagram socket");
    set_marks(&socket).await;
    const SRC_PORT: u16 = 1234;
    const DST_PORT: u16 = 5678;
    let addr = std::net::IpAddr::from(
        match socket_state {
            SocketState::Bound => I::UNSPECIFIED_ADDRESS,
            SocketState::Connected => I::LOOPBACK_ADDRESS.get(),
        }
        .to_ip_addr(),
    );
    socket.bind(&std::net::SocketAddr::from((addr, SRC_PORT)).into()).expect("bind");

    match socket_state {
        SocketState::Connected => {
            socket.connect(&std::net::SocketAddr::from((addr, DST_PORT)).into()).expect("connect");
        }
        SocketState::Bound => {}
    }

    let want_local = AddrAndPortFormatter::<_, _, I>::new(addr, SRC_PORT).to_string();
    let want_remote = match socket_state {
        SocketState::Connected => AddrAndPortFormatter::<_, _, I>::new(addr, DST_PORT).to_string(),
        SocketState::Bound => "[NOT CONNECTED]".to_string(),
    };
    let want_proto = match proto {
        fposix_socket::DatagramSocketProtocol::Udp => "UDP",
        fposix_socket::DatagramSocketProtocol::IcmpEcho => "ICMP_ECHO",
    };

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    // NB: The sockets are keyed by an opaque debug identifier.
    let sockets = data.get_child("Sockets").unwrap();
    let sock_name = assert_matches!(&sockets.children[..], [socket] => socket.name.clone());
    match proto {
        fposix_socket::DatagramSocketProtocol::Udp => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                Sockets: {
                    sock_name => {
                        LocalAddress: want_local,
                        RemoteAddress: want_remote,
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: want_proto,
                        NetworkProtocol: I::NAME,
                        MulticastGroupMemberships: {},
                        Counters: {
                            Rx: {
                                "Delivered": 0u64,
                                Errors: {
                                    "DroppedQueueFull": 0u64,
                                    "HardIcmpErrors": 0u64,
                                }
                            },
                            Tx: {
                                "Sent": 0u64,
                                "Errors": 0u64,
                            },
                        },
                    },
                }
            })
        }
        fposix_socket::DatagramSocketProtocol::IcmpEcho => {
            diagnostics_assertions::assert_data_tree!(data, "root": contains {
                Sockets: {
                    sock_name => {
                        LocalAddress: want_local,
                        RemoteAddress: want_remote,
                        Mark1: MARK_1,
                        Mark2: MARK_2,
                        TransportProtocol: want_proto,
                        NetworkProtocol: I::NAME,
                        MulticastGroupMemberships: {},
                    },
                }
            })
        }
    }
}

#[netstack_test]
#[variant(I, Ip)]
async fn inspect_multicast_group_memberships<I: TestIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");

    let loopback_id =
        u32::try_from(get_loopback_id(&realm).await).expect("loopback ID should fit in u32");

    // Ensure ns3 has started and that there is a Socket to collect inspect data about.
    let socket = realm
        .datagram_socket(I::DOMAIN, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("create datagram socket");

    // Join a multicast group on the loopback interface.
    let multicast_addr = match I::VERSION {
        IpVersion::V4 => {
            let mcast_addr = net_declare::std_ip_v4!("224.0.0.1");
            let loopback_index = socket2::InterfaceIndexOrAddress::Index(loopback_id);
            socket
                .join_multicast_v4_n(&mcast_addr, &loopback_index)
                .expect("failed to join multicast_group");
            std::net::IpAddr::V4(mcast_addr)
        }
        IpVersion::V6 => {
            let mcast_addr = net_declare::std_ip_v6!("ff00::1");
            socket
                .join_multicast_v6(&mcast_addr, loopback_id)
                .expect("failed to join multicast_group");
            std::net::IpAddr::V6(mcast_addr)
        }
    };

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    // NB: The sockets are keyed by an opaque debug identifier.
    let sockets = data.get_child("Sockets").unwrap();
    let sock_name = assert_matches!(&sockets.children[..], [socket] => socket.name.clone());
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        Sockets: {
            sock_name => {
                LocalAddress: "[NOT BOUND]",
                RemoteAddress: "[NOT CONNECTED]",
                TransportProtocol: "UDP",
                NetworkProtocol: I::NAME,
                // NB: The "Counters" schema is verified elsewhere; ignore it.
                Counters: contains {},
                MulticastGroupMemberships: {
                    "0": {
                        MulticastGroup: format!("{multicast_addr}"),
                        Device: u64::from(loopback_id),
                    },
                },
            },
        }
    })
}

#[netstack_test]
#[variant(I, Ip)]
async fn inspect_raw_ip_sockets<I: TestIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");

    // Ensure ns3 has started and that there is a Socket to collect inspect data about.
    let raw_socket = realm
        .raw_socket(
            I::DOMAIN,
            fposix_socket_raw::ProtocolAssociation::Associated(u8::from(IpProto::Tcp)),
        )
        .await
        .expect("create raw socket");
    set_marks(&raw_socket).await;

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    // NB: The sockets are keyed by an opaque debug identifier.
    let sockets = data.get_child("Sockets").unwrap();
    let sock_name = assert_matches!(&sockets.children[..], [socket] => socket.name.clone());
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        Sockets: {
            sock_name => {
                LocalAddress: "[NOT BOUND]",
                RemoteAddress: "[NOT CONNECTED]",
                Mark1: MARK_1,
                Mark2: MARK_2,
                TransportProtocol: "TCP",
                NetworkProtocol: I::NAME,
                BoundDevice: "None",
                IcmpFilter: "None",
                Counters: {
                    Rx: {
                        DeliveredPackets: 0u64,
                        DroppedQueueFull: 0u64,
                        ChecksumErrors: 0u64,
                        IcmpPacketsFiltered: 0u64,
                    },
                    Tx: {
                        SentPackets: 0u64,
                        ChecksumErrors: 0u64,
                    },
                },
            },
        }
    })
}

#[netstack_test]
async fn inspect_device_sockets(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");

    // Ensure ns3 has started and that there is a Socket to collect inspect data about.
    let _device_socket =
        realm.packet_socket(fposix_socket_packet::Kind::Link).await.expect("create device socket");

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    // NB: The sockets are keyed by an opaque debug identifier.
    let sockets = data.get_child("Sockets").unwrap();
    let sock_name = assert_matches!(&sockets.children[..], [socket] => socket.name.clone());
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        Sockets: {
            sock_name => {
                Protocol: "None",
                Device: "Any",
                Counters: {
                    Rx: {
                        DeliveredFrames: 0u64,
                        DroppedQueueFull: 0u64,
                    },
                    Tx: {
                        SentFrames: 0u64,
                        QueueFullError: 0u64,
                        AllocError: 0u64,
                        SizeConstraintError: 0u64,
                    },
                },
            },
        }
    })
}

/// Helper function that returns the ID of the loopback interface.
async fn get_loopback_id(realm: &netemul::TestRealm<'_>) -> u64 {
    let (id, _name) = get_loopback_id_and_name(realm).await;
    id
}

async fn get_loopback_id_and_name(realm: &netemul::TestRealm<'_>) -> (u64, String) {
    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("failed to connect to fuchsia.net.interfaces/State");

    fnet_interfaces_ext::wait_interface(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interfaces_state,
            Default::default(),
        )
        .expect("failed to create event stream"),
        &mut HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
        |if_map| {
            if_map.values().find_map(
                |fnet_interfaces_ext::PropertiesAndState {
                     properties: fnet_interfaces_ext::Properties { port_class, id, name, .. },
                     state: (),
                 }| { port_class.is_loopback().then(|| (id.get(), name.clone())) },
            )
        },
    )
    .await
    .expect("getting loopback id")
}

#[netstack_test]
async fn inspect_rules(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let rule_table_v4 = realm
        .connect_to_protocol::<fidl_fuchsia_net_routes_admin::RuleTableV4Marker>()
        .expect("failed to connect to fuchsia.net.routes.admin/RuleTableV4");
    const PRIORITY: fnet_routes_ext::rules::RuleSetPriority =
        fnet_routes_ext::rules::RuleSetPriority::new(100);
    let rule_set = fnet_routes_ext::rules::new_rule_set::<Ipv4>(&rule_table_v4, PRIORITY)
        .expect("failed to create rule set");

    fnet_routes_ext::rules::add_rule(
        &rule_set,
        0.into(),
        fnet_routes_ext::rules::RuleMatcher::<Ipv4> {
            from: Some(net_subnet_v4!("192.168.0.0/24")),
            ..Default::default()
        },
        fnet_routes_ext::rules::RuleAction::Unreachable,
    )
    .await
    .expect("fidl")
    .expect("failed to add rule");

    fnet_routes_ext::rules::add_rule(
        &rule_set,
        1.into(),
        fnet_routes_ext::rules::RuleMatcher::<Ipv4> {
            locally_generated: Some(true),
            ..Default::default()
        },
        fnet_routes_ext::rules::RuleAction::Unreachable,
    )
    .await
    .expect("fidl")
    .expect("failed to add rule");

    fnet_routes_ext::rules::add_rule(
        &rule_set,
        2.into(),
        fnet_routes_ext::rules::RuleMatcher::<Ipv4> {
            locally_generated: Some(true),
            bound_device: Some(fnet_matchers_ext::BoundInterface::Unbound),
            ..Default::default()
        },
        fnet_routes_ext::rules::RuleAction::Unreachable,
    )
    .await
    .expect("fidl")
    .expect("failed to add rule");

    fnet_routes_ext::rules::add_rule(
        &rule_set,
        3.into(),
        fnet_routes_ext::rules::RuleMatcher::<Ipv4> {
            locally_generated: Some(true),
            bound_device: Some(fnet_matchers_ext::BoundInterface::Bound(
                fnet_matchers_ext::Interface::Name("device-name".to_owned()),
            )),
            ..Default::default()
        },
        fnet_routes_ext::rules::RuleAction::Unreachable,
    )
    .await
    .expect("fidl")
    .expect("failed to add rule");

    fnet_routes_ext::rules::add_rule(
        &rule_set,
        4.into(),
        fnet_routes_ext::rules::RuleMatcher::<Ipv4> {
            mark_1: Some(fnet_matchers_ext::Mark::Unmarked),
            ..Default::default()
        },
        fnet_routes_ext::rules::RuleAction::Unreachable,
    )
    .await
    .expect("fidl")
    .expect("failed to add rule");

    fnet_routes_ext::rules::add_rule(
        &rule_set,
        5.into(),
        fnet_routes_ext::rules::RuleMatcher::<Ipv4> {
            mark_2: Some(fnet_matchers_ext::Mark::Marked {
                mask: 100,
                between: 0..=100,
                invert: false,
            }),
            ..Default::default()
        },
        fnet_routes_ext::rules::RuleAction::Unreachable,
    )
    .await
    .expect("fidl")
    .expect("failed to add rule");

    fnet_routes_ext::rules::add_rule(
        &rule_set,
        6.into(),
        fnet_routes_ext::rules::RuleMatcher::<Ipv4> {
            locally_generated: Some(false),
            ..Default::default()
        },
        fnet_routes_ext::rules::RuleAction::Unreachable,
    )
    .await
    .expect("fidl")
    .expect("failed to add rule");

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        Routes: {
            Ipv4: contains {
                Rules: {
                    "0": {
                        Matchers: {
                            SourceAddressFrom: "192.168.0.0/24",
                        },
                        Action: "Unreachable",
                    },
                    "1": {
                        Matchers: {
                            LocalOrigin: {},
                        },
                        Action: "Unreachable",
                    },
                    "2": {
                        Matchers: {
                            LocalOrigin: {
                                BoundDevice: "Unbound",
                            }
                        },
                        Action: "Unreachable",
                    },
                    "3": {
                        Matchers: {
                            LocalOrigin: {
                                BoundDevice: "Name(device-name)",
                            }
                        },
                        Action: "Unreachable",
                    },
                    "4": {
                        Matchers: {
                            Mark1: "Unmarked",
                        },
                        Action: "Unreachable",
                    },
                    "5": {
                        Matchers: {
                            Mark2: {
                                Mask: 100u64,
                                Range: {
                                    StartInclusive: 0u64,
                                    EndInclusive: 100u64,
                                },
                                Invert: false,
                            }
                        },
                        Action: "Unreachable",
                    },
                    "6": {
                        Matchers: {
                            TrafficOrigin: "NonLocal",
                        },
                        Action: "Unreachable",
                    },
                    // This is the default rule, not installed above.
                    "7": {
                        Matchers: {},
                        Action: {
                            Lookup: 0u64,
                        },
                    },
                },
            },
            Ipv6: contains {
                Rules: {
                    // This is the default rule, not installed above.
                    "0": {
                        Matchers: {},
                        Action: {
                            Lookup: 1u64,
                        }
                    }
                },
            }
        }
    })
}

#[netstack_test]
async fn inspect_routes(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let loopback_id = get_loopback_id(&realm).await;
    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        Routes: {
            Ipv4: {
                Rules: {
                    "0": {
                        Matchers: {},
                        Action: {
                            Lookup: 0u64,
                        }
                    }
                },
                RoutingTables: {
                    "0": {
                        "0": {
                            Destination: "127.0.0.0/8",
                            InterfaceId: loopback_id,
                            Gateway: "[NONE]",
                            Metric: 1000u64,
                            MetricTracksInterface: true,
                            RoutePreference: "Medium",
                        },
                        "1": {
                            Destination: "224.0.0.0/4",
                            InterfaceId: loopback_id,
                            Gateway: "[NONE]",
                            Metric: 1000u64,
                            MetricTracksInterface: true,
                            RoutePreference: "Medium",
                        },
                    },
                }
            },
            Ipv6: {
                Rules: {
                    "0": {
                        Matchers: {},
                        Action: {
                            Lookup: 1u64,
                        }
                    }
                },
                RoutingTables: {
                    "1": {
                        "0": {
                            Destination: "::1/128",
                            InterfaceId: loopback_id,
                            Gateway: "[NONE]",
                            Metric: 1000u64,
                            MetricTracksInterface: true,
                            RoutePreference: "Medium",
                        },
                        "1": {
                            Destination: "ff00::/8",
                            InterfaceId: loopback_id,
                            Gateway: "[NONE]",
                            Metric: 1000u64,
                            MetricTracksInterface: true,
                            RoutePreference: "Medium",
                        },
                    },
                }
            }
        }
    })
}

#[netstack_test]
async fn inspect_multicast_routes(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let network = sandbox.create_network("network").await.expect("create network failed");

    let dev1 = realm.join_network(&network, "DEV1").await.expect("join network failed");
    let dev2 = realm.join_network(&network, "DEV2").await.expect("join network failed");

    // Enable multicast routing for IPv6, but not IPv4. This allows us to assert
    // on the shape of inspect data for both enabled/disabled states.
    let controller = realm
        .connect_to_protocol::<<Ipv6 as FidlMulticastAdminIpExt>::TableControllerMarker>()
        .expect("failed to connect to Ipv6 multicast admin");

    // Add an arbitrary multicast route to ensure we have interesting data.
    TableControllerProxy::<Ipv6>::add_route(
        &controller,
        fnet_multicast_ext::UnicastSourceAndMulticastDestination {
            unicast_source: net_ip_v6!("2001:db8::1"),
            multicast_destination: net_ip_v6!("ff0e::1"),
        },
        fnet_multicast_ext::Route {
            expected_input_interface: dev1.id(),
            action: fnet_multicast_admin::Action::OutgoingInterfaces(vec![
                fnet_multicast_admin::OutgoingInterfaces { id: dev2.id(), min_ttl: 1 },
            ]),
        },
    )
    .await
    .expect("send request failed")
    .expect("add multicast route failed");

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    let data =
        data.get_child("MulticastForwarding").expect("multicast forwarding data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    diagnostics_assertions::assert_data_tree!(data, "MulticastForwarding": {
        "IPv4": {
            ForwardingEnabled: false,
        },
        "IPv6": {
            ForwardingEnabled: true,
            "Routes": {
                "0": {
                    SourceAddress: "2001:db8::1",
                    DestinationAddress: "ff0e::1",
                    InputInterface: dev1.id(),
                    "ForwardingTargets": {
                        "0": {
                            OutputInterface: dev2.id(),
                            MinTTL: 1u64,
                        },
                    },
                    "Statistics": {
                        LastUsed: diagnostics_assertions::NonZeroUintProperty,
                    }
                },
            },
            "PendingRoutes": {
                NumRoutes: 0u64,
            },
        },
    })
}

#[netstack_test]
async fn inspect_devices_ipv6_disabled(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    // Disable the use of opaque IIDs in SLAAC address generation so addresses are
    // deterministic (based on the MAC address) and we can assert on them below.
    let mut netstack: fnetemul::ChildDef =
        KnownServiceProvider::Netstack(NetstackVersion::Netstack3).into();
    netstack_testing_common::realms::set_netstack3_opaque_iids(&mut netstack, false);
    let realm = sandbox.create_realm(name, [netstack]).expect("failed to create realm");

    // Install netdevice device so that non-Loopback device Inspect properties can be asserted upon.
    const NETDEV_NAME: &str = "test-eth";
    let max_frame_size = netemul::DEFAULT_MTU
        + u16::try_from(ETHERNET_HDR_LEN_NO_TAG)
            .expect("should fit ethernet header length in a u16");
    let netdev = realm
        .join_network_with(
            &network,
            "netdev-ep",
            netemul::new_endpoint_config(max_frame_size, Some(fidl_mac!("02:00:00:00:00:01"))),
            netemul::InterfaceConfig {
                name: Some(NETDEV_NAME.into()),
                ipv6_dad_transmits: Some(u16::MAX),
                ..Default::default()
            },
        )
        .await
        .expect("failed to join network with netdevice endpoint");

    // Disable IPv6.
    let _ = netdev
        .control()
        .set_configuration(&fidl_fuchsia_net_interfaces_admin::Configuration {
            ipv6: Some(fidl_fuchsia_net_interfaces_admin::Ipv6Configuration {
                enabled: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("fidl")
        .expect("set configuration");

    netdev
        .add_address_and_subnet_route(fidl_subnet!("192.168.0.1/24"))
        .await
        .expect("configure address");

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        "Devices": {
            "1": {
                Name: "lo",
                InterfaceId: 1u64,
                AdminEnabled: true,
                IPv4Enabled: true,
                IPv6Enabled: true,
                MTU: 65536u64,
                Loopback: true,
                TxOffloadSpec: {
                    TxChecksumOffloadGeneric: true,
                },
                IPv4: {
                    Addresses: {
                        "127.0.0.1/8": {
                            ValidUntil: "infinite",
                            PreferredLifetime: "infinite",
                            Assigned: true,
                        }
                    },
                    Configuration: {
                        "GmpEnabled": false,
                        "ForwardingEnabled": false,
                        "MulticastForwardingEnabled": false,
                        "DadTransmits": 0u64,
                    },
                    GMP: {
                        Mode: "IGMPv3",
                        Groups: {
                            "224.0.0.1": {
                                Refs: 1u64,
                            }
                        }
                    }
                },
                IPv6: {
                    Addresses: {
                        "::1/128": {
                            ValidUntil: "infinite",
                            PreferredLifetime: "infinite",
                            IsSlaac: false,
                            Assigned: true,
                            Temporary: false,
                        }
                    },
                    Configuration: {
                        "GmpEnabled": false,
                        "ForwardingEnabled": false,
                        "MulticastForwardingEnabled": false,
                        "DadTransmits": 0u64,
                    },
                    GMP: {
                        Mode: "MLDv2",
                        Groups: {
                            "ff02::1:ff00:1": {
                                Refs: 1u64,
                            },
                            "ff02::1": {
                                Refs: 1u64,
                            }
                        }
                    }
                },
                Counters: {
                    Rx: {
                        TotalFrames: 0u64,
                        TotalBytes: 0u64,
                        Malformed: 0u64,
                        Ipv4Delivered: 0u64,
                        Ipv6Delivered: 0u64,
                    },
                    Tx: {
                        TotalFrames: 0u64,
                        Sent: 0u64,
                        SentBytes: 0u64,
                        SendIpv4Frame: 0u64,
                        SendIpv6Frame: 0u64,
                        NoQueue: 0u64,
                        QueueFull: 0u64,
                        DequeueDrop: 0u64,
                        SerializeError: 0u64,
                    },
                    Ethernet: {
                        Rx: {
                            NoEthertype: 0u64,
                            UnsupportedEthertype: 0u64,
                            SingleCsumOffloaded: 0u64,
                            MultipleCsumsOffloaded: 0u64,
                            AllCsumsOffloaded: 0u64,
                        },
                    },
                    IPv4: {
                        PacketTx: {
                            Sent: 0u64,
                            IllegalLoopbackAddress: 0u64,
                            SocketEgressFilterDropped: 0u64,
                        },
                        PacketRx: {
                            Received: 0u64,
                            Dispatched: 0u64,
                            DeliveredUnicast: 0u64,
                            DeliveredMulticast: 0u64,
                            DeliveredBroadcast: 0u64,
                            OtherHost: 0u64,
                            ParameterProblem: 0u64,
                            UnspecifiedDst: 0u64,
                            UnspecifiedSrc: 0u64,
                            InvalidSrc: 0u64,
                            Dropped: 0u64,
                            DroppedTentativeDst: 0u64,
                            MulticastNoInterest: 0u64,
                            InvalidCachedConntrackEntry: 0u64,
                            UnparsablePacket: 0u64,
                        },
                        Forwarding: {
                            Forwarded: 0u64,
                            ForwardingDisabled: 0u64,
                            NoRouteToHost: 0u64,
                            MtuExceeded: 0u64,
                            TtlExpired: 0u64,
                        },
                        RxIcmpError: 0u64,
                        FragmentsRx: {
                            ReassemblyError: 0u64,
                            NeedMoreFragments: 0u64,
                            InvalidFragment: 0u64,
                            CacheFull: 0u64,
                        },
                        FragmentsTx: {
                            FragmentationRequired: 0u64,
                            Fragments: 0u64,
                            ErrorNotAllowed: 0u64,
                            ErrorMtuTooSmall: 0u64,
                            ErrorBodyTooLong: 0u64,
                            ErrorInnerSizeLimitExceeded: 0u64,
                            ErrorFragmentedSerializer: 0u64,
                        },
                    },
                    IPv6: {
                        PacketTx: {
                            Sent: 0u64,
                            IllegalLoopbackAddress: 0u64,
                            SocketEgressFilterDropped: 0u64,
                        },
                        PacketRx: {
                            Received: 0u64,
                            Dispatched: 0u64,
                            DeliveredMulticast: 0u64,
                            DeliveredUnicast: 0u64,
                            OtherHost: 0u64,
                            ParameterProblem: 0u64,
                            UnspecifiedDst: 0u64,
                            UnspecifiedSrc: 0u64,
                            InvalidSrc: 0u64,
                            Dropped: 0u64,
                            DroppedTentativeDst: 0u64,
                            DroppedExtensionHeader: 0u64,
                            DroppedLoopedBackDadProbe: 0u64,
                            MulticastNoInterest: 0u64,
                            InvalidCachedConntrackEntry: 0u64,
                            UnparsablePacket: 0u64,
                        },
                        Forwarding: {
                            Forwarded: 0u64,
                            ForwardingDisabled: 0u64,
                            NoRouteToHost: 0u64,
                            MtuExceeded: 0u64,
                            TtlExpired: 0u64,
                        },
                        RxIcmpError: 0u64,
                        FragmentsRx: {
                            ReassemblyError: 0u64,
                            NeedMoreFragments: 0u64,
                            InvalidFragment: 0u64,
                            CacheFull: 0u64,
                        },
                        FragmentsTx: {
                            FragmentationRequired: 0u64,
                            Fragments: 0u64,
                            ErrorNotAllowed: 0u64,
                            ErrorMtuTooSmall: 0u64,
                            ErrorBodyTooLong: 0u64,
                            ErrorInnerSizeLimitExceeded: 0u64,
                            ErrorFragmentedSerializer: 0u64,
                        },
                    },
                    IGMP: {
                        Rx: {
                            IGMPv1Query: 0u64,
                            IGMPv2Query: 0u64,
                            IGMPv3Query: 0u64,
                            IGMPv1Report: 0u64,
                            IGMPv2Report: 0u64,
                            IGMPv3Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                ParseFailed: 0u64,
                                MissingRouterAlertInQuery: 0u64,
                                RejectedGeneralQuery: 0u64,
                                BadTTL: 0u64,
                            },
                        },
                        Tx: {
                            IGMPv1Report: 0u64,
                            IGMPv2Report: 0u64,
                            IGMPv3Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                SendFailed: 0u64,
                            },
                        },
                    },
                    MLD: {
                        Rx: {
                            MLDv1Query: 0u64,
                            MLDv2Query: 0u64,
                            MLDv1Report: 0u64,
                            MLDv2Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                MissingRouterAlert: 0u64,
                                BadSourceAddress: 0u64,
                                BadHopLimit: 0u64,
                            },
                        },
                        Tx: {
                            MLDv1Report: 0u64,
                            MLDv2Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                SendFailed: 0u64,
                            },
                        },
                    },
                }
            },
            "2": {
                Name: NETDEV_NAME,
                InterfaceId: 2u64,
                AdminEnabled: true,
                IPv4Enabled: true,
                IPv6Enabled: false,
                MTU: u64::from(netemul::DEFAULT_MTU),
                Loopback: false,
                TxOffloadSpec: {
                    TxChecksumOffloadGeneric: false,
                },
                IPv4: {
                    "Addresses": {
                        "192.168.0.1/24": {
                            ValidUntil: "infinite",
                            PreferredLifetime: "infinite",
                            Assigned: true,
                        }
                    },
                    Configuration: {
                        "GmpEnabled": true,
                        "ForwardingEnabled": false,
                        "MulticastForwardingEnabled": false,
                        "DadTransmits": 3u64,
                    },
                    GMP: {
                        Mode: "IGMPv3",
                        Groups: {
                            "224.0.0.1": {
                                Refs: 1u64,
                            }
                        }
                    }
                },
                IPv6: {
                    "Addresses": {},
                    Configuration: {
                        "GmpEnabled": true,
                        "ForwardingEnabled": false,
                        "MulticastForwardingEnabled": false,
                        "DadTransmits": u16::MAX,
                    },
                    GMP: {
                        Mode: "MLDv2",
                        Groups: {}
                    }
                },
                NetworkDevice: {
                    MacAddress: "02:00:00:00:00:01",
                    PhyUp: true,
                },
                Counters: {
                    Rx: {
                        TotalFrames: 0u64,
                        Malformed: 0u64,
                        TotalBytes: 0u64,
                        Ipv4Delivered: 0u64,
                        Ipv6Delivered: 0u64,
                    },
                    Tx: {
                        TotalFrames: diagnostics_assertions::AnyUintProperty,
                        Sent: diagnostics_assertions::AnyUintProperty,
                        SentBytes: diagnostics_assertions::AnyUintProperty,
                        SendIpv4Frame: diagnostics_assertions::AnyUintProperty,
                        SendIpv6Frame: diagnostics_assertions::AnyUintProperty,
                        NoQueue: 0u64,
                        QueueFull: 0u64,
                        DequeueDrop: 0u64,
                        SerializeError: 0u64,
                    },
                    Ethernet: {
                        Rx: {
                            NoEthertype: 0u64,
                            UnsupportedEthertype: 0u64,
                            SingleCsumOffloaded: 0u64,
                            MultipleCsumsOffloaded: 0u64,
                            AllCsumsOffloaded: 0u64,
                        },
                    },
                    IPv4: {
                        PacketTx: {
                            Sent: diagnostics_assertions::AnyUintProperty,
                            IllegalLoopbackAddress: 0u64,
                            SocketEgressFilterDropped: 0u64,
                        },
                        PacketRx: {
                            Received: 0u64,
                            Dispatched: 0u64,
                            DeliveredUnicast: 0u64,
                            DeliveredMulticast: 0u64,
                            DeliveredBroadcast: 0u64,
                            OtherHost: 0u64,
                            ParameterProblem: 0u64,
                            UnspecifiedDst: 0u64,
                            UnspecifiedSrc: 0u64,
                            InvalidSrc: 0u64,
                            Dropped: 0u64,
                            DroppedTentativeDst: 0u64,
                            MulticastNoInterest: 0u64,
                            InvalidCachedConntrackEntry: 0u64,
                            UnparsablePacket: 0u64,
                        },
                        Forwarding: {
                            Forwarded: 0u64,
                            ForwardingDisabled: 0u64,
                            NoRouteToHost: 0u64,
                            MtuExceeded: 0u64,
                            TtlExpired: 0u64,
                        },
                        RxIcmpError: 0u64,
                        FragmentsRx: {
                            ReassemblyError: 0u64,
                            NeedMoreFragments: 0u64,
                            InvalidFragment: 0u64,
                            CacheFull: 0u64,
                        },
                        FragmentsTx: {
                            FragmentationRequired: 0u64,
                            Fragments: 0u64,
                            ErrorNotAllowed: 0u64,
                            ErrorMtuTooSmall: 0u64,
                            ErrorBodyTooLong: 0u64,
                            ErrorInnerSizeLimitExceeded: 0u64,
                            ErrorFragmentedSerializer: 0u64,
                        },
                    },
                    IPv6: {
                        PacketTx: {
                            Sent: diagnostics_assertions::AnyUintProperty,
                            IllegalLoopbackAddress: 0u64,
                            SocketEgressFilterDropped: 0u64,
                        },
                        PacketRx: {
                            Received: 0u64,
                            Dispatched: 0u64,
                            DeliveredMulticast: 0u64,
                            DeliveredUnicast: 0u64,
                            OtherHost: 0u64,
                            ParameterProblem: 0u64,
                            UnspecifiedDst: 0u64,
                            UnspecifiedSrc: 0u64,
                            InvalidSrc: 0u64,
                            Dropped: 0u64,
                            DroppedTentativeDst: 0u64,
                            DroppedExtensionHeader: 0u64,
                            DroppedLoopedBackDadProbe: 0u64,
                            MulticastNoInterest: 0u64,
                            InvalidCachedConntrackEntry: 0u64,
                            UnparsablePacket: 0u64,
                        },
                        Forwarding: {
                            Forwarded: 0u64,
                            ForwardingDisabled: 0u64,
                            NoRouteToHost: 0u64,
                            MtuExceeded: 0u64,
                            TtlExpired: 0u64,
                        },
                        RxIcmpError: 0u64,
                        FragmentsRx: {
                            ReassemblyError: 0u64,
                            NeedMoreFragments: 0u64,
                            InvalidFragment: 0u64,
                            CacheFull: 0u64,
                        },
                        FragmentsTx: {
                            FragmentationRequired: 0u64,
                            Fragments: 0u64,
                            ErrorNotAllowed: 0u64,
                            ErrorMtuTooSmall: 0u64,
                            ErrorBodyTooLong: 0u64,
                            ErrorInnerSizeLimitExceeded: 0u64,
                            ErrorFragmentedSerializer: 0u64,
                        },
                    },
                    IGMP: {
                        Rx: {
                            IGMPv1Query: 0u64,
                            IGMPv2Query: 0u64,
                            IGMPv3Query: 0u64,
                            IGMPv1Report: 0u64,
                            IGMPv2Report: 0u64,
                            IGMPv3Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                ParseFailed: 0u64,
                                MissingRouterAlertInQuery: 0u64,
                                RejectedGeneralQuery: 0u64,
                                BadTTL: 0u64,
                            },
                        },
                        Tx: {
                            IGMPv1Report: 0u64,
                            IGMPv2Report: 0u64,
                            IGMPv3Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                SendFailed: 0u64,
                            },
                        },
                    },
                    MLD: {
                        Rx: {
                            MLDv1Query: 0u64,
                            MLDv2Query: 0u64,
                            MLDv1Report: 0u64,
                            MLDv2Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                MissingRouterAlert: 0u64,
                                BadSourceAddress: 0u64,
                                BadHopLimit: 0u64,
                            },
                        },
                        Tx: {
                            MLDv1Report: 0u64,
                            // Note: Duplicate address detection (DAD) joins the
                            // solicited-node multicast group for the interface.
                            // This may or may not cause a report to have been
                            // sent by the netstack (depending on if the join
                            // finishes before inspect counters are fetched).
                            MLDv2Report: diagnostics_assertions::AnyUintProperty,
                            LeaveGroup: 0u64,
                            Errors: {
                                SendFailed: 0u64,
                            },
                        },
                    },
                }
            }
        }
    })
}

#[netstack_test]
async fn inspect_devices(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    // Disable the use of opaque IIDs in SLAAC address generation so addresses are
    // deterministic (based on the MAC address) and we can assert on them below.
    let mut netstack: fnetemul::ChildDef =
        KnownServiceProvider::Netstack(NetstackVersion::Netstack3).into();
    netstack_testing_common::realms::set_netstack3_opaque_iids(&mut netstack, false);
    let realm = sandbox.create_realm(name, [netstack]).expect("failed to create realm");

    // Install netdevice device so that non-Loopback device Inspect properties can be asserted upon.
    const NETDEV_NAME: &str = "test-eth";
    let max_frame_size = netemul::DEFAULT_MTU
        + u16::try_from(ETHERNET_HDR_LEN_NO_TAG)
            .expect("should fit ethernet header length in a u16");
    let netdev = realm
        .join_network_with(
            &network,
            "netdev-ep",
            netemul::new_endpoint_config(max_frame_size, Some(fidl_mac!("02:00:00:00:00:01"))),
            netemul::InterfaceConfig {
                name: Some(NETDEV_NAME.into()),
                ipv6_dad_transmits: Some(u16::MAX),
                ..Default::default()
            },
        )
        .await
        .expect("failed to join network with netdevice endpoint");

    netdev
        .add_address_and_subnet_route(fidl_subnet!("192.168.0.1/24"))
        .await
        .expect("configure address");

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        "Devices": {
            "1": {
                Name: "lo",
                InterfaceId: 1u64,
                AdminEnabled: true,
                IPv4Enabled: true,
                IPv6Enabled: true,
                MTU: 65536u64,
                Loopback: true,
                TxOffloadSpec: {
                    TxChecksumOffloadGeneric: true,
                },
                IPv4: {
                    Addresses: {
                        "127.0.0.1/8": {
                            ValidUntil: "infinite",
                            PreferredLifetime: "infinite",
                            Assigned: true,
                        }
                    },
                    Configuration: {
                        "GmpEnabled": false,
                        "ForwardingEnabled": false,
                        "MulticastForwardingEnabled": false,
                        "DadTransmits": 0u64,
                    },
                    GMP: {
                        Mode: "IGMPv3",
                        Groups: {
                            "224.0.0.1": {
                                Refs: 1u64,
                            }
                        }
                    }
                },
                IPv6: {
                    Addresses: {
                        "::1/128": {
                            ValidUntil: "infinite",
                            PreferredLifetime: "infinite",
                            IsSlaac: false,
                            Assigned: true,
                            Temporary: false,
                        }
                    },
                    Configuration: {
                        "GmpEnabled": false,
                        "ForwardingEnabled": false,
                        "MulticastForwardingEnabled": false,
                        "DadTransmits": 0u64,
                    },
                    GMP: {
                        Mode: "MLDv2",
                        Groups: {
                            "ff02::1:ff00:1": {
                                Refs: 1u64,
                            },
                            "ff02::1": {
                                Refs: 1u64,
                            }
                        }
                    }
                },
                Counters: {
                    Rx: {
                        TotalFrames: 0u64,
                        TotalBytes: 0u64,
                        Malformed: 0u64,
                        Ipv4Delivered: 0u64,
                        Ipv6Delivered: 0u64,
                    },
                    Tx: {
                        TotalFrames: 0u64,
                        Sent: 0u64,
                        SentBytes: 0u64,
                        SendIpv4Frame: 0u64,
                        SendIpv6Frame: 0u64,
                        NoQueue: 0u64,
                        QueueFull: 0u64,
                        DequeueDrop: 0u64,
                        SerializeError: 0u64,
                    },
                    Ethernet: {
                        Rx: {
                            NoEthertype: 0u64,
                            UnsupportedEthertype: 0u64,
                            SingleCsumOffloaded: 0u64,
                            MultipleCsumsOffloaded: 0u64,
                            AllCsumsOffloaded: 0u64,
                        },
                    },
                    IPv4: {
                        PacketTx: {
                            Sent: 0u64,
                            IllegalLoopbackAddress: 0u64,
                            SocketEgressFilterDropped: 0u64,
                        },
                        PacketRx: {
                            Received: 0u64,
                            Dispatched: 0u64,
                            DeliveredUnicast: 0u64,
                            DeliveredMulticast: 0u64,
                            DeliveredBroadcast: 0u64,
                            OtherHost: 0u64,
                            ParameterProblem: 0u64,
                            UnspecifiedDst: 0u64,
                            UnspecifiedSrc: 0u64,
                            InvalidSrc: 0u64,
                            Dropped: 0u64,
                            DroppedTentativeDst: 0u64,
                            MulticastNoInterest: 0u64,
                            InvalidCachedConntrackEntry: 0u64,
                            UnparsablePacket: 0u64,
                        },
                        Forwarding: {
                            Forwarded: 0u64,
                            ForwardingDisabled: 0u64,
                            NoRouteToHost: 0u64,
                            MtuExceeded: 0u64,
                            TtlExpired: 0u64,
                        },
                        RxIcmpError: 0u64,
                        FragmentsRx: {
                            ReassemblyError: 0u64,
                            NeedMoreFragments: 0u64,
                            InvalidFragment: 0u64,
                            CacheFull: 0u64,
                        },
                        FragmentsTx: {
                            FragmentationRequired: 0u64,
                            Fragments: 0u64,
                            ErrorNotAllowed: 0u64,
                            ErrorMtuTooSmall: 0u64,
                            ErrorBodyTooLong: 0u64,
                            ErrorInnerSizeLimitExceeded: 0u64,
                            ErrorFragmentedSerializer: 0u64,
                        },
                    },
                    IPv6: {
                        PacketTx: {
                            Sent: 0u64,
                            IllegalLoopbackAddress: 0u64,
                            SocketEgressFilterDropped: 0u64,
                        },
                        PacketRx: {
                            Received: 0u64,
                            Dispatched: 0u64,
                            DeliveredMulticast: 0u64,
                            DeliveredUnicast: 0u64,
                            OtherHost: 0u64,
                            ParameterProblem: 0u64,
                            UnspecifiedDst: 0u64,
                            UnspecifiedSrc: 0u64,
                            InvalidSrc: 0u64,
                            Dropped: 0u64,
                            DroppedTentativeDst: 0u64,
                            DroppedExtensionHeader: 0u64,
                            DroppedLoopedBackDadProbe: 0u64,
                            MulticastNoInterest: 0u64,
                            InvalidCachedConntrackEntry: 0u64,
                            UnparsablePacket: 0u64,
                        },
                        Forwarding: {
                            Forwarded: 0u64,
                            ForwardingDisabled: 0u64,
                            NoRouteToHost: 0u64,
                            MtuExceeded: 0u64,
                            TtlExpired: 0u64,
                        },
                        RxIcmpError: 0u64,
                        FragmentsRx: {
                            ReassemblyError: 0u64,
                            NeedMoreFragments: 0u64,
                            InvalidFragment: 0u64,
                            CacheFull: 0u64,
                        },
                        FragmentsTx: {
                            FragmentationRequired: 0u64,
                            Fragments: 0u64,
                            ErrorNotAllowed: 0u64,
                            ErrorMtuTooSmall: 0u64,
                            ErrorBodyTooLong: 0u64,
                            ErrorInnerSizeLimitExceeded: 0u64,
                            ErrorFragmentedSerializer: 0u64,
                        },
                    },
                    IGMP: {
                        Rx: {
                            IGMPv1Query: 0u64,
                            IGMPv2Query: 0u64,
                            IGMPv3Query: 0u64,
                            IGMPv1Report: 0u64,
                            IGMPv2Report: 0u64,
                            IGMPv3Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                ParseFailed: 0u64,
                                MissingRouterAlertInQuery: 0u64,
                                RejectedGeneralQuery: 0u64,
                                BadTTL: 0u64,
                            },
                        },
                        Tx: {
                            IGMPv1Report: 0u64,
                            IGMPv2Report: 0u64,
                            IGMPv3Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                SendFailed: 0u64,
                            },
                        },
                    },
                    MLD: {
                        Rx: {
                            MLDv1Query: 0u64,
                            MLDv2Query: 0u64,
                            MLDv1Report: 0u64,
                            MLDv2Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                MissingRouterAlert: 0u64,
                                BadSourceAddress: 0u64,
                                BadHopLimit: 0u64,
                            },
                        },
                        Tx: {
                            MLDv1Report: 0u64,
                            MLDv2Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                SendFailed: 0u64,
                            },
                        },
                    },
                }
            },
            "2": {
                Name: NETDEV_NAME,
                InterfaceId: 2u64,
                AdminEnabled: true,
                IPv4Enabled: true,
                IPv6Enabled: true,
                MTU: u64::from(netemul::DEFAULT_MTU),
                Loopback: false,
                TxOffloadSpec: {
                    TxChecksumOffloadGeneric: false,
                },
                IPv4: {
                    "Addresses": {
                        "192.168.0.1/24": {
                            ValidUntil: "infinite",
                            PreferredLifetime: "infinite",
                            Assigned: true,
                        }
                    },
                    Configuration: {
                        "GmpEnabled": true,
                        "ForwardingEnabled": false,
                        "MulticastForwardingEnabled": false,
                        "DadTransmits": 3u64,
                    },
                    GMP: {
                        Mode: "IGMPv3",
                        Groups: {
                            "224.0.0.1": {
                                Refs: 1u64,
                            }
                        }
                    }
                },
                IPv6: {
                    "Addresses": {
                        "fe80::ff:fe00:1/64": {
                            ValidUntil: "infinite",
                            PreferredLifetime: "infinite",
                            IsSlaac: true,
                            // This will always be `false` because DAD will never complete; we set
                            // the number of DAD transmits to `u16::MAX` above.
                            Assigned: false,
                            Temporary: false,
                            CreationTime: diagnostics_assertions::AnyUintProperty,
                            DadCounter: diagnostics_assertions::AnyUintProperty,
                            RegenCounter: 0u64,
                        }
                    },
                    Configuration: {
                        "GmpEnabled": true,
                        "ForwardingEnabled": false,
                        "MulticastForwardingEnabled": false,
                        "DadTransmits": u16::MAX,
                    },
                    GMP: {
                        Mode: "MLDv2",
                        Groups: {
                            "ff02::1:ff00:1": {
                                Refs: 1u64,
                            },
                            "ff02::1": {
                                Refs: 1u64,
                            },
                        }
                    }
                },
                NetworkDevice: {
                    MacAddress: "02:00:00:00:00:01",
                    PhyUp: true,
                },
                Counters: {
                    Rx: {
                        TotalFrames: 0u64,
                        Malformed: 0u64,
                        TotalBytes: 0u64,
                        Ipv4Delivered: 0u64,
                        Ipv6Delivered: 0u64,
                    },
                    Tx: {
                        TotalFrames: diagnostics_assertions::AnyUintProperty,
                        Sent: diagnostics_assertions::AnyUintProperty,
                        SentBytes: diagnostics_assertions::AnyUintProperty,
                        SendIpv4Frame: diagnostics_assertions::AnyUintProperty,
                        SendIpv6Frame: diagnostics_assertions::AnyUintProperty,
                        NoQueue: 0u64,
                        QueueFull: 0u64,
                        DequeueDrop: 0u64,
                        SerializeError: 0u64,
                    },
                    Ethernet: {
                        Rx: {
                            NoEthertype: 0u64,
                            UnsupportedEthertype: 0u64,
                            SingleCsumOffloaded: 0u64,
                            MultipleCsumsOffloaded: 0u64,
                            AllCsumsOffloaded: 0u64,
                        },
                    },
                    IPv4: {
                        PacketTx: {
                            Sent: diagnostics_assertions::AnyUintProperty,
                            IllegalLoopbackAddress: 0u64,
                            SocketEgressFilterDropped: 0u64,
                        },
                        PacketRx: {
                            Received: 0u64,
                            Dispatched: 0u64,
                            DeliveredUnicast: 0u64,
                            DeliveredMulticast: 0u64,
                            DeliveredBroadcast: 0u64,
                            OtherHost: 0u64,
                            ParameterProblem: 0u64,
                            UnspecifiedDst: 0u64,
                            UnspecifiedSrc: 0u64,
                            InvalidSrc: 0u64,
                            Dropped: 0u64,
                            DroppedTentativeDst: 0u64,
                            MulticastNoInterest: 0u64,
                            InvalidCachedConntrackEntry: 0u64,
                            UnparsablePacket: 0u64,
                        },
                        Forwarding: {
                            Forwarded: 0u64,
                            ForwardingDisabled: 0u64,
                            NoRouteToHost: 0u64,
                            MtuExceeded: 0u64,
                            TtlExpired: 0u64,
                        },
                        RxIcmpError: 0u64,
                        FragmentsRx: {
                            ReassemblyError: 0u64,
                            NeedMoreFragments: 0u64,
                            InvalidFragment: 0u64,
                            CacheFull: 0u64,
                        },
                        FragmentsTx: {
                            FragmentationRequired: 0u64,
                            Fragments: 0u64,
                            ErrorNotAllowed: 0u64,
                            ErrorMtuTooSmall: 0u64,
                            ErrorBodyTooLong: 0u64,
                            ErrorInnerSizeLimitExceeded: 0u64,
                            ErrorFragmentedSerializer: 0u64,
                        },
                    },
                    IPv6: {
                        PacketTx: {
                            Sent: diagnostics_assertions::AnyUintProperty,
                            IllegalLoopbackAddress: 0u64,
                            SocketEgressFilterDropped: 0u64,
                        },
                        PacketRx: {
                            Received: 0u64,
                            Dispatched: 0u64,
                            DeliveredMulticast: 0u64,
                            DeliveredUnicast: 0u64,
                            OtherHost: 0u64,
                            ParameterProblem: 0u64,
                            UnspecifiedDst: 0u64,
                            UnspecifiedSrc: 0u64,
                            InvalidSrc: 0u64,
                            Dropped: 0u64,
                            DroppedTentativeDst: 0u64,
                            DroppedExtensionHeader: 0u64,
                            DroppedLoopedBackDadProbe: 0u64,
                            MulticastNoInterest: 0u64,
                            InvalidCachedConntrackEntry: 0u64,
                            UnparsablePacket: 0u64,
                        },
                        Forwarding: {
                            Forwarded: 0u64,
                            ForwardingDisabled: 0u64,
                            NoRouteToHost: 0u64,
                            MtuExceeded: 0u64,
                            TtlExpired: 0u64,
                        },
                        RxIcmpError: 0u64,
                        FragmentsRx: {
                            ReassemblyError: 0u64,
                            NeedMoreFragments: 0u64,
                            InvalidFragment: 0u64,
                            CacheFull: 0u64,
                        },
                        FragmentsTx: {
                            FragmentationRequired: 0u64,
                            Fragments: 0u64,
                            ErrorNotAllowed: 0u64,
                            ErrorMtuTooSmall: 0u64,
                            ErrorBodyTooLong: 0u64,
                            ErrorInnerSizeLimitExceeded: 0u64,
                            ErrorFragmentedSerializer: 0u64,
                        },
                    },
                    IGMP: {
                        Rx: {
                            IGMPv1Query: 0u64,
                            IGMPv2Query: 0u64,
                            IGMPv3Query: 0u64,
                            IGMPv1Report: 0u64,
                            IGMPv2Report: 0u64,
                            IGMPv3Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                ParseFailed: 0u64,
                                MissingRouterAlertInQuery: 0u64,
                                RejectedGeneralQuery: 0u64,
                                BadTTL: 0u64,
                            },
                        },
                        Tx: {
                            IGMPv1Report: 0u64,
                            IGMPv2Report: 0u64,
                            IGMPv3Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                SendFailed: 0u64,
                            },
                        },
                    },
                    MLD: {
                        Rx: {
                            MLDv1Query: 0u64,
                            MLDv2Query: 0u64,
                            MLDv1Report: 0u64,
                            MLDv2Report: 0u64,
                            LeaveGroup: 0u64,
                            Errors: {
                                MissingRouterAlert: 0u64,
                                BadSourceAddress: 0u64,
                                BadHopLimit: 0u64,
                            },
                        },
                        Tx: {
                            MLDv1Report: 0u64,
                            // Note: Duplicate address detection (DAD) joins the
                            // solicited-node multicast group for the interface.
                            // This may or may not cause a report to have been
                            // sent by the netstack (depending on if the join
                            // finishes before inspect counters are fetched).
                            MLDv2Report: diagnostics_assertions::AnyUintProperty,
                            LeaveGroup: 0u64,
                            Errors: {
                                SendFailed: 0u64,
                            },
                        },
                    },
                }
            }
        }
    })
}

#[netstack_test]
async fn inspect_counters(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");

    // Send a packet over loopback to increment Tx and Rx count by 1.
    let sender = realm
        .datagram_socket(fposix_socket::Domain::Ipv4, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("datagram socket creation failed");
    let receiver = realm
        .datagram_socket(fposix_socket::Domain::Ipv4, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("datagram socket creation failed");

    let addr = net_declare::std_socket_addr!("127.0.0.1:8080");
    receiver.bind(&addr.into()).expect("binding receiver");

    const MSG_SIZE: usize = 8;
    let buf = [0; MSG_SIZE];
    let bytes_sent = sender.send_to(&buf, &addr.into()).expect("socket send to failed");
    assert_eq!(bytes_sent, buf.len());

    // We don't care about the bytes, we just want to make sure we receive it so
    // we know the message has made through the stack and the counters are as we
    // expect.
    let mut recv_buf = [MaybeUninit::<u8>::uninit(); MSG_SIZE];
    let bytes_received = receiver.recv(&mut recv_buf[..]).expect("receiving bytes");
    assert_eq!(bytes_received, MSG_SIZE);

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");

    // Debug print the tree to make debugging easier in case of failures.
    println!("Got inspect data: {:#?}", data);
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        "Counters": {
            "Bindings": {
                "Power": {
                    DroppedRxLeases: 0u64,
                },
                "MulticastAdmin": {
                    "V4": {
                        DroppedRoutingEvents: 0u64,
                    },
                    "V6": {
                        DroppedRoutingEvents: 0u64,
                    },
                },
            },
            "Device": {
                "Rx": {
                    TotalFrames: 1u64,
                    TotalBytes: 50u64,
                    Malformed: 0u64,
                    Ipv4Delivered: 1u64,
                    Ipv6Delivered: 0u64,
                },
                "Tx": {
                    TotalFrames: 1u64,
                    Sent: 1u64,
                    SentBytes: 50u64,
                    SendIpv4Frame: 1u64,
                    SendIpv6Frame: 0u64,
                    NoQueue: 0u64,
                    QueueFull: 0u64,
                    DequeueDrop: 0u64,
                    SerializeError: 0u64,
                },
                "Ethernet": {
                    "Rx": {
                        NoEthertype: 0u64,
                        UnsupportedEthertype: 0u64,
                        SingleCsumOffloaded: 0u64,
                        MultipleCsumsOffloaded: 0u64,
                        AllCsumsOffloaded: 1u64,
                    },
                },
            },
            "Arp": {
                "Rx": {
                    TotalPackets: 0u64,
                    Requests: 0u64,
                    Responses: 0u64,
                    Malformed: 0u64,
                    Echoed: 0u64,
                    NonLocalDstAddr: 0u64,
                },
                "Tx": {
                    Requests: 0u64,
                    RequestsNonLocalSrcAddr: 0u64,
                    Responses: 0u64,
                },
            },
            "NUD": {
                "V4": {
                    IcmpDestUnreachableDropped: 0u64,
                },
                "V6": {
                    IcmpDestUnreachableDropped: 0u64,
                },
            },
            "ICMP": {
                "V4": {
                    "Rx": {
                        EchoRequest: 0u64,
                        EchoReply: 0u64,
                        TimestampRequest: 0u64,
                        "DestUnreachable": {
                            DestNetworkUnreachable: 0u64,
                            DestHostUnreachable: 0u64,
                            DestProtocolUnreachable: 0u64,
                            DestPortUnreachable: 0u64,
                            FragmentationRequired: 0u64,
                            SourceRouteFailed: 0u64,
                            DestNetworkUnknown: 0u64,
                            DestHostUnknown: 0u64,
                            SourceHostIsolated: 0u64,
                            NetworkAdministrativelyProhibited: 0u64,
                            HostAdministrativelyProhibited: 0u64,
                            NetworkUnreachableForTos: 0u64,
                            HostUnreachableForTos: 0u64,
                            CommAdministrativelyProhibited: 0u64,
                            HostPrecedenceViolation: 0u64,
                            PrecedenceCutoffInEffect: 0u64,
                        },
                        "TimeExceeded": {
                            TtlExpired: 0u64,
                            FragmentReassemblyTimeExceeded: 0u64,
                        },
                        "ParameterProblem": {
                            PointerIndicatesError: 0u64,
                            MissingRequiredOption: 0u64,
                            BadLength: 0u64,
                        },
                        PacketTooBig: 0u64,
                        Error: 0u64,
                        ErrorDeliveredToTransportLayer: 0u64,
                        ErrorDeliveredToSocket: 0u64,
                        DroppedQueueFull: 0u64,
                    },
                    "Tx": {
                        Reply: 0u64,
                        PacketTooBig: 0u64,
                        Error: 0u64,
                        "DestUnreachable": {
                            DestNetworkUnreachable: 0u64,
                            DestHostUnreachable: 0u64,
                            DestProtocolUnreachable: 0u64,
                            DestPortUnreachable: 0u64,
                            FragmentationRequired: 0u64,
                            SourceRouteFailed: 0u64,
                            DestNetworkUnknown: 0u64,
                            DestHostUnknown: 0u64,
                            SourceHostIsolated: 0u64,
                            NetworkAdministrativelyProhibited: 0u64,
                            HostAdministrativelyProhibited: 0u64,
                            NetworkUnreachableForTos: 0u64,
                            HostUnreachableForTos: 0u64,
                            CommAdministrativelyProhibited: 0u64,
                            HostPrecedenceViolation: 0u64,
                            PrecedenceCutoffInEffect: 0u64,
                        },
                        "TimeExceeded": {
                            TtlExpired: 0u64,
                            FragmentReassemblyTimeExceeded: 0u64,
                        },
                        "ParameterProblem": {
                            PointerIndicatesError: 0u64,
                            MissingRequiredOption: 0u64,
                            BadLength: 0u64,
                        },
                    },
                },
                "V6": {
                    "Rx": {
                        EchoRequest: 0u64,
                        EchoReply: 0u64,
                        TimestampRequest: 0u64,
                        "DestUnreachable": {
                            NoRoute: 0u64,
                            CommunicationAdministrativelyProhibited: 0u64,
                            BeyondScope: 0u64,
                            AddrUnreachable: 0u64,
                            PortUnreachable: 0u64,
                            SrcAddrFailedPolicy: 0u64,
                            RejectRoute: 0u64,
                        },
                        "TimeExceeded": {
                            HopLimitExceeded: 0u64,
                            FragmentReassemblyTimeExceeded: 0u64,
                        },
                        "ParameterProblem": {
                            ErroneousHeaderField: 0u64,
                            UnrecognizedNextHeaderType: 0u64,
                            UnrecognizedIpv6Option: 0u64,
                        },
                        PacketTooBig: 0u64,
                        Error: 0u64,
                        ErrorDeliveredToTransportLayer: 0u64,
                        ErrorDeliveredToSocket: 0u64,
                        DroppedQueueFull: 0u64,
                        "NDP": {
                            NeighborSolicitation: 0u64,
                            NeighborAdvertisement: 0u64,
                            RouterSolicitation: 0u64,
                            RouterAdvertisement: 0u64,
                        },
                    },
                    "Tx": {
                        Reply: 0u64,
                        PacketTooBig: 0u64,
                        Error: 0u64,
                        "DestUnreachable": {
                            NoRoute: 0u64,
                            CommunicationAdministrativelyProhibited: 0u64,
                            BeyondScope: 0u64,
                            AddrUnreachable: 0u64,
                            PortUnreachable: 0u64,
                            SrcAddrFailedPolicy: 0u64,
                            RejectRoute: 0u64,
                        },
                        "TimeExceeded": {
                            HopLimitExceeded: 0u64,
                            FragmentReassemblyTimeExceeded: 0u64,
                        },
                        "ParameterProblem": {
                            ErroneousHeaderField: 0u64,
                            UnrecognizedNextHeaderType: 0u64,
                            UnrecognizedIpv6Option: 0u64,
                        },
                        "NDP": {
                            NeighborAdvertisement: 0u64,
                            NeighborSolicitation: 0u64,
                        },
                    },
                },
            },
            "IGMP": {
                Rx: {
                    IGMPv1Query: 0u64,
                    IGMPv2Query: 0u64,
                    IGMPv3Query: 0u64,
                    IGMPv1Report: 0u64,
                    IGMPv2Report: 0u64,
                    IGMPv3Report: 0u64,
                    LeaveGroup: 0u64,
                    Errors: {
                        ParseFailed: 0u64,
                        MissingRouterAlertInQuery: 0u64,
                        RejectedGeneralQuery: 0u64,
                        BadTTL: 0u64,
                    },
                },
                Tx: {
                    IGMPv1Report: 0u64,
                    IGMPv2Report: 0u64,
                    IGMPv3Report: 0u64,
                    LeaveGroup: 0u64,
                    Errors: {
                        SendFailed: 0u64,
                    },
                },
            },
            "MLD": {
                Rx: {
                    MLDv1Query: 0u64,
                    MLDv2Query: 0u64,
                    MLDv1Report: 0u64,
                    MLDv2Report: 0u64,
                    LeaveGroup: 0u64,
                    Errors: {
                        MissingRouterAlert: 0u64,
                        BadSourceAddress: 0u64,
                        BadHopLimit: 0u64,
                    },
                },
                Tx: {
                    MLDv1Report: 0u64,
                    MLDv2Report: 0u64,
                    LeaveGroup: 0u64,
                    Errors: {
                        SendFailed: 0u64,
                    },
                },
            },
            "IPv4": {
                PacketTx: {
                    Sent: 1u64,
                    IllegalLoopbackAddress: 0u64,
                    SocketEgressFilterDropped: 0u64,
                },
                "PacketRx": {
                    Received: 1u64,
                    Dispatched: 1u64,
                    DeliveredUnicast: 1u64,
                    DeliveredMulticast: 0u64,
                    DeliveredBroadcast: 0u64,
                    OtherHost: 0u64,
                    ParameterProblem: 0u64,
                    UnspecifiedDst: 0u64,
                    UnspecifiedSrc: 0u64,
                    InvalidSrc: 0u64,
                    Dropped: 0u64,
                    DroppedTentativeDst: 0u64,
                    MulticastNoInterest: 0u64,
                    InvalidCachedConntrackEntry: 0u64,
                    UnparsablePacket: 0u64,
                },
                "Forwarding": {
                    Forwarded: 0u64,
                    ForwardingDisabled: 0u64,
                    NoRouteToHost: 0u64,
                    MtuExceeded: 0u64,
                    TtlExpired: 0u64,
                },
                RxIcmpError: 0u64,
                "FragmentsRx": {
                    ReassemblyError: 0u64,
                    NeedMoreFragments: 0u64,
                    InvalidFragment: 0u64,
                    CacheFull: 0u64,
                },
                "FragmentsTx": {
                    FragmentationRequired: 0u64,
                    Fragments: 0u64,
                    ErrorNotAllowed: 0u64,
                    ErrorMtuTooSmall: 0u64,
                    ErrorBodyTooLong: 0u64,
                    ErrorInnerSizeLimitExceeded: 0u64,
                    ErrorFragmentedSerializer: 0u64,
                },
            },
            "IPv6": {
                PacketTx: {
                    Sent: 0u64,
                    IllegalLoopbackAddress: 0u64,
                    SocketEgressFilterDropped: 0u64,
                },
                "PacketRx": {
                    Received: 0u64,
                    Dispatched: 0u64,
                    DeliveredMulticast: 0u64,
                    DeliveredUnicast: 0u64,
                    OtherHost: 0u64,
                    ParameterProblem: 0u64,
                    UnspecifiedDst: 0u64,
                    UnspecifiedSrc: 0u64,
                    InvalidSrc: 0u64,
                    Dropped: 0u64,
                    DroppedTentativeDst: 0u64,
                    DroppedExtensionHeader: 0u64,
                    DroppedLoopedBackDadProbe: 0u64,
                    MulticastNoInterest: 0u64,
                    InvalidCachedConntrackEntry: 0u64,
                    UnparsablePacket: 0u64,
                },
                "Forwarding": {
                    Forwarded: 0u64,
                    ForwardingDisabled: 0u64,
                    NoRouteToHost: 0u64,
                    MtuExceeded: 0u64,
                    TtlExpired: 0u64,
                },
                RxIcmpError: 0u64,
                "FragmentsRx": {
                    ReassemblyError: 0u64,
                    NeedMoreFragments: 0u64,
                    InvalidFragment: 0u64,
                    CacheFull: 0u64,
                },
                "FragmentsTx": {
                    FragmentationRequired: 0u64,
                    Fragments: 0u64,
                    ErrorNotAllowed: 0u64,
                    ErrorMtuTooSmall: 0u64,
                    ErrorBodyTooLong: 0u64,
                    ErrorInnerSizeLimitExceeded: 0u64,
                    ErrorFragmentedSerializer: 0u64,
                },
            },
            "MulticastForwarding": {
                "V4": {
                    PacketsReceived: 0u64,
                    PacketsForwarded: 0u64,
                    "PacketsNotForwardedWithReason": {
                        InvalidKey: 0u64,
                        ForwardingDisabledOnInputDevice: 0u64,
                        ForwardingDisabledForStack: 0u64,
                        WrongInputDevice: 0u64,
                    },
                    PendingPackets: 0u64,
                    PendingPacketsForwarded: 0u64,
                    "PendingPacketsNotForwardedWithReason": {
                        QueueFull: 0u64,
                        ForwardingDisabledOnInputDevice: 0u64,
                        WrongInputDevice: 0u64,
                        GarbageCollected: 0u64,
                    },
                    PendingTableGcRuns: 0u64,
                },
                "V6": {
                    PacketsReceived: 0u64,
                    PacketsForwarded: 0u64,
                    "PacketsNotForwardedWithReason": {
                        InvalidKey: 0u64,
                        ForwardingDisabledOnInputDevice: 0u64,
                        ForwardingDisabledForStack: 0u64,
                        WrongInputDevice: 0u64,
                    },
                    PendingPackets: 0u64,
                    PendingPacketsForwarded: 0u64,
                    "PendingPacketsNotForwardedWithReason": {
                        QueueFull: 0u64,
                        ForwardingDisabledOnInputDevice: 0u64,
                        WrongInputDevice: 0u64,
                        GarbageCollected: 0u64,
                    },
                    PendingTableGcRuns: 0u64,
                },
            },
            "RawIpSockets": {
                "V4": {
                    "Rx": {
                        DeliveredPackets: 0u64,
                        ChecksumErrors: 0u64,
                        IcmpPacketsFiltered: 0u64,
                        DroppedQueueFull: 0u64,
                    },
                    "Tx": {
                        SentPackets: 0u64,
                        ChecksumErrors: 0u64,
                    },
                },
                "V6": {
                    "Rx": {
                        DeliveredPackets: 0u64,
                        ChecksumErrors: 0u64,
                        IcmpPacketsFiltered: 0u64,
                        DroppedQueueFull: 0u64,
                    },
                    "Tx": {
                        SentPackets: 0u64,
                        ChecksumErrors: 0u64,
                    },
                },
            },
            "DeviceSockets": {
                "Rx": {
                    DeliveredFrames: 0u64,
                    DroppedQueueFull: 0u64,
                },
                "Tx": {
                    SentFrames: 0u64,
                    QueueFullError: 0u64,
                    AllocError: 0u64,
                    SizeConstraintError: 0u64,
                },
            },
            "UDP": {
                "V4": {
                    "Rx": {
                        Received: 1u64,
                        Delivered: 1u64,
                        "Errors": {
                            MappedAddr: 0u64,
                            UnknownDstPort: 0u64,
                            Malformed: 0u64,
                            DroppedQueueFull: 0u64,
                            HardIcmpErrors: 0u64,
                        },
                    },
                    "Tx": {
                        Sent: 1u64,
                        Errors: 0u64,
                    },
                    "IcmpErrors": {
                        Count: 0u64,
                        Soft: 0u64,
                        Hard: 0u64,
                        Malformed: 0u64,
                        NoSocket: 0u64,
                    },
                },
                "V6": {
                    "Rx": {
                        Received: 0u64,
                        Delivered: 0u64,
                        "Errors": {
                            MappedAddr: 0u64,
                            UnknownDstPort: 0u64,
                            Malformed: 0u64,
                            DroppedQueueFull: 0u64,
                            HardIcmpErrors: 0u64,
                        },
                    },
                    "Tx": {
                        Sent: 0u64,
                        Errors: 0u64,
                    },
                    "IcmpErrors": {
                        Count: 0u64,
                        Soft: 0u64,
                        Hard: 0u64,
                        Malformed: 0u64,
                        NoSocket: 0u64,
                    },
                },
            },
            "TCP": {
                "V4": {
                    PassiveConnectionOpenings: 0u64,
                    ActiveConnectionOpenings: 0u64,
                    FastRecovery: 0u64,
                    SackRecovery: 0u64,
                    LossRecovered: 0u64,
                    EstablishedClosed: 0u64,
                    EstablishedResets: 0u64,
                    EstablishedTimedout: 0u64,
                    "Rx": {
                        ValidSegmentsReceived: 0u64,
                        ReceivedSegmentsDispatched: 0u64,
                        ResetsReceived: 0u64,
                        SynsReceived: 0u64,
                        FinsReceived: 0u64,
                        DupAcks: 0u64,
                        "Errors": {
                            ChecksumErrors: 0u64,
                            InvalidIpAddrsReceived: 0u64,
                            InvalidSegmentsReceived: 0u64,
                            ReceivedSegmentsNoDispatch: 0u64,
                            ListenerQueueOverflow: 0u64,
                            PassiveOpenNoRouteErrors: 0u64,
                        },
                    },
                    "Tx": {
                        SegmentsSent: 0u64,
                        ResetsSent: 0u64,
                        SynsSent: 0u64,
                        FinsSent: 0u64,
                        Timeouts: 0u64,
                        Retransmits: 0u64,
                        FastRetransmits: 0u64,
                        SackRetransmits: 0u64,
                        SlowStartRetransmits: 0u64,
                        "Errors": {
                            SegmentSendErrors: 0u64,
                            ActiveOpenNoRouteErrors: 0u64,
                        }
                    },
                    "Errors": {
                        FailedConnectionOpenings: 0u64,
                        FailedPortReservations: 0u64,
                    }
                },
                "V6": {
                    PassiveConnectionOpenings: 0u64,
                    ActiveConnectionOpenings: 0u64,
                    FastRecovery: 0u64,
                    SackRecovery: 0u64,
                    LossRecovered: 0u64,
                    EstablishedClosed: 0u64,
                    EstablishedResets: 0u64,
                    EstablishedTimedout: 0u64,
                    "Rx": {
                        ValidSegmentsReceived: 0u64,
                        ReceivedSegmentsDispatched: 0u64,
                        ResetsReceived: 0u64,
                        SynsReceived: 0u64,
                        FinsReceived: 0u64,
                        DupAcks: 0u64,
                        "Errors": {
                            ChecksumErrors: 0u64,
                            InvalidIpAddrsReceived: 0u64,
                            InvalidSegmentsReceived: 0u64,
                            ReceivedSegmentsNoDispatch: 0u64,
                            ListenerQueueOverflow: 0u64,
                            PassiveOpenNoRouteErrors: 0u64,
                        },
                    },
                    "Tx": {
                        SegmentsSent: 0u64,
                        ResetsSent: 0u64,
                        SynsSent: 0u64,
                        FinsSent: 0u64,
                        Timeouts: 0u64,
                        Retransmits: 0u64,
                        FastRetransmits: 0u64,
                        SackRetransmits: 0u64,
                        SlowStartRetransmits: 0u64,
                        "Errors": {
                            SegmentSendErrors: 0u64,
                            ActiveOpenNoRouteErrors: 0u64,
                        }
                    },
                    "Errors": {
                        FailedConnectionOpenings: 0u64,
                        FailedPortReservations: 0u64,
                    }
                },
            },
        }
    })
}

#[netstack_test]
async fn inspect_filtering_state(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let control = realm
        .connect_to_protocol::<fnet_filter::ControlMarker>()
        .expect("connect to filter control");
    let id = ControllerId(String::from("inspect"));
    let mut controller = Controller::new(&control, &id).await.expect("open filter controller");

    // By default, the netstack should report all the filtering hooks for both
    // IP versions, but have no filtering state configured.
    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");
    // Debug print the tree to make debugging easier in case of failures.
    println!("got inspect data: {:#?}", data);
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        "Filtering State": {
            "IPv4": {
                "IP": {
                    "ingress": {
                        "routines": 0u64,
                    },
                    "local_ingress": {
                        "routines": 0u64,
                    },
                    "forwarding": {
                        "routines": 0u64,
                    },
                    "local_egress": {
                        "routines": 0u64,
                    },
                    "egress": {
                        "routines": 0u64,
                    },
                },
                "NAT": {
                    "ingress": {
                        "routines": 0u64,
                    },
                    "local_ingress": {
                        "routines": 0u64,
                    },
                    "local_egress": {
                        "routines": 0u64,
                    },
                    "egress": {
                        "routines": 0u64,
                    },
                },
                "uninstalled": {
                    "routines": 0u64,
                },
                "conntrack": {
                    "table_limit_drops": 0u64,
                    "table_limit_hits": 0u64,
                    "num_entries": 0u64,
                    "connections": {},
                },
            },
            "IPv6": {
                "IP": {
                    "ingress": {
                        "routines": 0u64,
                    },
                    "local_ingress": {
                        "routines": 0u64,
                    },
                    "forwarding": {
                        "routines": 0u64,
                    },
                    "local_egress": {
                        "routines": 0u64,
                    },
                    "egress": {
                        "routines": 0u64,
                    },
                },
                "NAT": {
                    "ingress": {
                        "routines": 0u64,
                    },
                    "local_ingress": {
                        "routines": 0u64,
                    },
                    "local_egress": {
                        "routines": 0u64,
                    },
                    "egress": {
                        "routines": 0u64,
                    },
                },
                "uninstalled": {
                    "routines": 0u64,
                },
                "conntrack": {
                    "table_limit_drops": 0u64,
                    "table_limit_hits": 0u64,
                    "num_entries": 0u64,
                    "connections": {},
                },
            },
        },
    });

    const NONZERO_PORT: NonZeroU16 = NonZeroU16::new(8080).unwrap();

    let namespace = NamespaceId(String::from("test-namespace"));
    let ingress_routine = RoutineId { namespace: namespace.clone(), name: String::from("ingress") };
    let egress_routine = RoutineId { namespace: namespace.clone(), name: String::from("egress") };
    let nat_routine = RoutineId { namespace: namespace.clone(), name: String::from("nat") };
    let target_routine_name = String::from("target");
    controller
        .push_changes(
            [
                Resource::Namespace(Namespace { id: namespace.clone(), domain: Domain::AllIp }),
                Resource::Routine(Routine {
                    id: ingress_routine.clone(),
                    routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                        hook: IpHook::Ingress,
                        priority: -10,
                    })),
                }),
                Resource::Rule(Rule {
                    id: RuleId { routine: ingress_routine.clone(), index: 20 },
                    matchers: Matchers {
                        in_interface: Some(fnet_matchers_ext::Interface::Id(
                            NonZeroU64::new(1).unwrap(),
                        )),
                        ..Default::default()
                    },
                    action: Action::Drop,
                }),
                Resource::Rule(Rule {
                    id: RuleId { routine: ingress_routine, index: 10 },
                    matchers: Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: None,
                            dst_port: Some(
                                fnet_matchers_ext::Port::new(22, 22, /* invert */ false).unwrap(),
                            ),
                        }),
                        ..Default::default()
                    },
                    action: Action::Drop,
                }),
                Resource::Routine(Routine {
                    id: RoutineId { namespace, name: target_routine_name.clone() },
                    routine_type: RoutineType::Ip(None),
                }),
                Resource::Routine(Routine {
                    id: egress_routine.clone(),
                    routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                        hook: IpHook::Egress,
                        priority: -10,
                    })),
                }),
                Resource::Rule(Rule {
                    id: RuleId { routine: egress_routine, index: 0 },
                    matchers: Matchers {
                        dst_addr: Some(fnet_matchers_ext::Address {
                            matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                                fidl_subnet!("127.0.0.0/8").try_into().unwrap(),
                            ),
                            invert: false,
                        }),
                        ..Default::default()
                    },
                    action: Action::Jump(target_routine_name),
                }),
                Resource::Routine(Routine {
                    id: nat_routine.clone(),
                    routine_type: RoutineType::Nat(Some(InstalledNatRoutine {
                        hook: NatHook::LocalEgress,
                        priority: 0,
                    })),
                }),
                Resource::Rule(Rule {
                    id: RuleId { routine: nat_routine, index: 0 },
                    matchers: Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                            src_port: None,
                            dst_port: None,
                        }),
                        ..Default::default()
                    },
                    action: Action::Redirect {
                        dst_port: Some(PortRange(NONZERO_PORT..=NONZERO_PORT)),
                    },
                }),
            ]
            .into_iter()
            .map(Change::Create)
            .collect(),
        )
        .await
        .expect("push filter changes");
    controller.commit().await.expect("commit filter changes");

    let data =
        get_inspect_data(&realm, "netstack", "root").await.expect("inspect data should be present");
    // Debug print the tree to make debugging easier in case of failures.
    println!("got inspect data: {:#?}", data);
    diagnostics_assertions::assert_data_tree!(data, "root": contains {
        "Filtering State": {
            "IPv4": {
                "IP": {
                    "ingress": {
                        "routines": 1u64,
                        "0": {
                            "rules": 2u64,
                            "0": {
                                "matchers": {
                                    "transport_protocol": "TransportProtocolMatcher { \
                                        proto: TCP, \
                                        src_port: None, \
                                        dst_port: Some(PortMatcher { range: 22..=22, invert: false }) \
                                    }",
                                },
                                "action": "Drop",
                            },
                            "1": {
                                "matchers": {
                                    "in_interface": "Id(1)",
                                },
                                "action": "Drop",
                            },
                        },
                    },
                    "local_ingress": {
                        "routines": 0u64,
                    },
                    "forwarding": {
                        "routines": 0u64,
                    },
                    "local_egress": {
                        "routines": 0u64,
                    },
                    "egress": {
                        "routines": 1u64,
                        "0": {
                            "rules": 1u64,
                            "0": {
                                // Note that this rule is only included in the IPv4 filtering state
                                // because it has an address matcher with an IPv4 subnet.
                                "matchers": {
                                    "dst_address": {
                                        "subnet": "127.0.0.0/8",
                                        "invert": false,
                                    },
                                },
                                "action": "Jump(UninstalledRoutine(1))",
                            },
                        },
                    },
                },
                "NAT": {
                    "ingress": {
                        "routines": 0u64,
                    },
                    "local_ingress": {
                        "routines": 0u64,
                    },
                    "local_egress": {
                        "routines": 1u64,
                        "0": {
                            "rules": 1u64,
                            "0": {
                                "matchers": {
                                    "transport_protocol": "TransportProtocolMatcher { \
                                        proto: UDP, \
                                        src_port: None, \
                                        dst_port: None \
                                    }",
                                },
                                "action": "Redirect { dst_port: Some(8080..=8080) }",
                            },
                        },
                    },
                    "egress": {
                        "routines": 0u64,
                    },
                },
                // Because the uninstalled routine is only jumped to from an IPv4 routine, it
                // only exists in the IPv4 filtering state.
                "uninstalled": {
                    "routines": 1u64,
                    "1": {
                        "rules": 0u64,
                    },
                },
                "conntrack": {
                    "table_limit_drops": 0u64,
                    "table_limit_hits": 0u64,
                    "num_entries": 0u64,
                    "connections": {},
                }
            },
            "IPv6": {
                "IP": {
                    "ingress": {
                        "routines": 1u64,
                        "0": {
                            "rules": 2u64,
                            "0": {
                                "matchers": {
                                    "transport_protocol": "TransportProtocolMatcher { \
                                        proto: TCP, \
                                        src_port: None, \
                                        dst_port: Some(PortMatcher { range: 22..=22, invert: false }) \
                                    }",
                                },
                                "action": "Drop",
                            },
                            "1": {
                                "matchers": {
                                    "in_interface": "Id(1)",
                                },
                                "action": "Drop",
                            },
                        },
                    },
                    "local_ingress": {
                        "routines": 0u64,
                    },
                    "forwarding": {
                        "routines": 0u64,
                    },
                    "local_egress": {
                        "routines": 0u64,
                    },
                    "egress": {
                        "routines": 1u64,
                        "0": {
                            "rules": 0u64,
                        },
                    },
                },
                "NAT": {
                    "ingress": {
                        "routines": 0u64,
                    },
                    "local_ingress": {
                        "routines": 0u64,
                    },
                    "local_egress": {
                        "routines": 1u64,
                        "0": {
                            "rules": 1u64,
                            "0": {
                                "matchers": {
                                    "transport_protocol": "TransportProtocolMatcher { \
                                        proto: UDP, \
                                        src_port: None, \
                                        dst_port: None \
                                    }",
                                },
                                "action": "Redirect { dst_port: Some(8080..=8080) }",
                            },
                        },
                    },
                    "egress": {
                        "routines": 0u64,
                    },
                },
                "uninstalled": {
                    "routines": 0u64,
                },
                "conntrack": {
                    "table_limit_drops": 0u64,
                    "table_limit_hits": 0u64,
                    "num_entries": 0u64,
                    "connections": {},
                }
            },
        }
    });
}

#[netstack_test]
async fn inspect_conntrack_nat_state(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network("net").await.expect("create network");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    const LOCAL_ADDR_SUBNET: fnet::Subnet = fidl_subnet!("192.168.0.1/24");
    const LOCAL_IP: Ipv4Addr = net_ip_v4!("192.168.0.1");
    const SRC_IP: Ipv4Addr = net_ip_v4!("192.168.0.2");
    const DST_IP: Ipv4Addr = net_ip_v4!("192.168.0.3");
    const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(11111).unwrap();
    const SRC_PORT: NonZeroU16 = NonZeroU16::new(22222).unwrap();
    const DST_PORT: NonZeroU16 = NonZeroU16::new(44444).unwrap();

    let interface = realm.join_network(&network, "ep").await.expect("install interface");
    interface.add_address_and_subnet_route(LOCAL_ADDR_SUBNET).await.expect("configure address");
    interface.set_ipv4_forwarding_enabled(true).await.expect("enable forwarding");

    // Install a Masquerade rule on EGRESS.
    let control = realm
        .connect_to_protocol::<fnet_filter::ControlMarker>()
        .expect("connect to filter control");
    let id = ControllerId(String::from("inspect"));
    let mut controller = Controller::new(&control, &id).await.expect("open filter controller");
    let namespace = NamespaceId(String::from("test-namespace"));
    let nat_routine = RoutineId { namespace: namespace.clone(), name: String::from("nat") };
    controller
        .push_changes(
            [
                Resource::Namespace(Namespace { id: namespace, domain: Domain::AllIp }),
                Resource::Routine(Routine {
                    id: nat_routine.clone(),
                    routine_type: RoutineType::Nat(Some(InstalledNatRoutine {
                        hook: NatHook::Egress,
                        priority: 0,
                    })),
                }),
                Resource::Rule(Rule {
                    id: RuleId { routine: nat_routine, index: 0 },
                    matchers: Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                            src_port: None,
                            dst_port: None,
                        }),
                        ..Default::default()
                    },
                    action: Action::Masquerade {
                        src_port: Some(PortRange(LOCAL_PORT..=LOCAL_PORT)),
                    },
                }),
            ]
            .into_iter()
            .map(Change::Create)
            .collect(),
        )
        .await
        .expect("push filter changes");
    controller.commit().await.expect("commit filter changes");

    // Inject an incoming UDP packet to be forwarded (and masqueraded) by the
    // netstack.
    let mut payload = [1u8, 2, 3, 4];
    let frame = packet::Buf::new(&mut payload, ..)
        .wrap_in(UdpPacketBuilder::new(SRC_IP, DST_IP, Some(SRC_PORT), DST_PORT))
        .wrap_in(Ipv4PacketBuilder::new(
            SRC_IP,
            DST_IP,
            u8::MAX, /* ttl */
            IpProto::Udp.into(),
        ))
        .wrap_in(EthernetFrameBuilder::new(
            constants::eth::MAC_ADDR,         /* src_mac */
            interface.mac().await.into_ext(), /* dst_mac */
            EtherType::Ipv4,
            ETHERNET_MIN_BODY_LEN_NO_TAG,
        ))
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .expect("serialize UDP packet IP packet in ethernet frame")
        .unwrap_b();
    let fake_ep = network.create_fake_endpoint().expect("create fake endpoint");
    fake_ep.write(frame.as_ref()).await.expect("inject incoming UDP packet");

    // Wait to see the ARP request from the netstack attempting to perform neighbor
    // resolution so we know it has inserted the forwarded flow in the conntrack
    // table.
    let mut frames = fake_ep.frame_stream();
    loop {
        let (data, dropped) = frames
            .next()
            .await
            .expect("should see forwarded packet before frame stream ends")
            .expect("should not get FIDL error");
        assert_eq!(dropped, 0);

        let mut buffer = &data[..];
        let _ethernet = buffer
            .parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::NoCheck)
            .expect("parse ethernet frame");
        let Ok(arp) = buffer.parse::<ArpPacket<_, Mac, Ipv4Addr>>() else {
            continue;
        };
        assert_eq!(arp.operation(), ArpOp::Request);
        assert_eq!(arp.sender_protocol_address(), LOCAL_IP);
        assert_eq!(arp.target_protocol_address(), DST_IP);
        break;
    }

    let data =
        get_inspect_data(&realm, "netstack", "root/Filtering\\ State/IPv4/conntrack/connections")
            .await
            .expect("inspect data should be present");
    // Debug print the tree to make debugging easier in case of failures.
    println!("got inspect data: {:#?}", data);

    // Expect the NAT configuration to report the dynamically assigned IP address
    // that it is using to masquerade the forwarded connection.
    let weak_address_id_regex =
        Regex::new(r"WeakAddressId\([0-9]+:0x[0-9a-fA-F]+ => 192\.168\.0\.1/24\)").unwrap();
    diagnostics_assertions::assert_data_tree!(data, "root": {
        "Filtering State": {
            "IPv4": {
                "conntrack": {
                    "connections": {
                        "0": contains {
                            "original_tuple": {
                                "src_addr": SRC_IP.to_string(),
                                "dst_addr": DST_IP.to_string(),
                                "src_port_or_id": u64::from(SRC_PORT.get()),
                                "dst_port_or_id": u64::from(DST_PORT.get()),
                                "protocol": "UDP",
                            },
                            "reply_tuple": {
                                "src_addr": DST_IP.to_string(),
                                "dst_addr": LOCAL_IP.to_string(),
                                "src_port_or_id": u64::from(DST_PORT.get()),
                                "dst_port_or_id": u64::from(LOCAL_PORT.get()),
                                "protocol": "UDP",
                            },
                            "external_data": {
                                "NAT": {
                                    "Destination": {
                                        "Status": "No-op",
                                    },
                                    "Source": {
                                        "Status": "NAT",
                                        "To": weak_address_id_regex,
                                    },
                                },
                            },
                        },
                    },
                },
            },
        },
    });
}

/// The location where [`inspect_for_sampler`] will pull Sampler config from.
/// The test must make sure to output sampler config into that directory (minus
/// the leading "/pkg") in its dependencies.
const SAMPLER_CONFIG_FILE: &str = "/pkg/data/sampler-config/netstack.json";

/// Validates that the selectors used in the Sampler config are present in the
/// inspect data.
#[netstack_test]
async fn inspect_for_sampler(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    // Connect to netstack service to spawn a netstack instance.
    let _ = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");

    let file = File::open(SAMPLER_CONFIG_FILE).expect("open config file");
    let mut reader = BufReader::new(file);
    let project_config: ProjectConfig =
        serde_json5::from_reader(&mut reader).expect("loaded sampler config");
    for metric_config in project_config.data_sets.iter().flat_map(|ds| ds.metrics.iter()) {
        let selector = match &metric_config.selectors[..] {
            [selector] => selector,
            selectors => panic!("expected one selector but got {:#?}", selectors),
        };
        let fidl_fuchsia_diagnostics::Selector { tree_selector, .. } = selector;
        let (tree_selector, expected_key) = match tree_selector.as_ref().expect("tree_selector") {
            fidl_fuchsia_diagnostics::TreeSelector::PropertySelector(
                fidl_fuchsia_diagnostics::PropertySelector { node_path, target_properties },
            ) => {
                let tree_selector = node_path
                    .iter()
                    .map(|selector| match selector {
                        fidl_fuchsia_diagnostics::StringSelector::ExactMatch(segment) => {
                            selectors::sanitize_string_for_selectors(segment)
                        }
                        selector => panic!("expected exact match selector but got {:#?}", selector),
                    })
                    .join("/");
                let expected_key = match target_properties {
                    fidl_fuchsia_diagnostics::StringSelector::ExactMatch(segment) => segment,
                    selector => panic!("expected exact match selector but got {:#?}", selector),
                };
                (tree_selector, expected_key)
            }
            selector => panic!("expected property selector but got {:#?}", selector),
        };
        let data = get_inspect_data(&realm, "netstack", &format!("{tree_selector}:{expected_key}"))
            .await
            .expect("inspect data should be present");
        let properties: Vec<_> = data
            .property_iter()
            .filter_map(|(_hierarchy_path, property_opt): (Vec<&str>, _)| property_opt)
            .collect();
        match &properties[..] {
            [diagnostics_hierarchy::Property::Uint(key, _)] => {
                if key != expected_key {
                    panic!(
                        "wrong key {:#?} found (expected {:#?}) for selector {:#?}",
                        key, expected_key, selector
                    );
                }
            }
            [] => {
                panic!("no properties found for selector {:#?}", selector)
            }
            properties => {
                panic!("wrong properties {:#?} found for selector {:#?}", properties, selector);
            }
        }
    }
}

#[netstack_test]
async fn inspect_sampled_stats(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("net").await.expect("failed to create network");

    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");

    let (lo_id, lo_name) = get_loopback_id_and_name(&realm).await;

    // Create a blackhole interface, that should not gather sampled stats.
    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");
    let (control, control_server_end) =
        fnet_interfaces_ext::admin::Control::create_endpoints().expect("create proxy");
    installer
        .install_blackhole_interface(
            control_server_end,
            fidl_fuchsia_net_interfaces_admin::Options {
                name: Some("blackhole".to_string()),
                metric: None,
                ..Default::default()
            },
        )
        .expect("create blackhole interface");

    let _blachole_id = control.get_id().await.expect("get id");

    // Install a non-loopback interface.
    const ETH_NAME: &str = "test-eth";
    let eth = realm
        .join_network_with_if_config(
            &network,
            "netdev-ep",
            netemul::InterfaceConfig { name: Some(ETH_NAME.into()), ..Default::default() },
        )
        .await
        .expect("failed to join network with netdevice endpoint");

    fn time_series_assertion(name: &str, metadata: bool) -> diagnostics_assertions::TreeAssertion {
        let mut assertion = diagnostics_assertions::tree_assertion!(
            var name: {
                "data": diagnostics_assertions::AnyBytesProperty,
                "type": diagnostics_assertions::AnyStringProperty,
            }
        );
        if metadata {
            assertion.add_child_assertion(
                diagnostics_assertions::tree_assertion!("metadata": contains {}),
            );
        }
        assertion
    }

    fn interface_stats_tree_assertion(
        id: u64,
        name: &str,
        status: bool,
    ) -> diagnostics_assertions::TreeAssertion {
        let mut assertion =
            diagnostics_assertions::TreeAssertion::new(&format!("{id}=>{name}"), true);
        assertion.add_child_assertion(time_series_assertion("rx", false));
        assertion.add_child_assertion(time_series_assertion("tx", false));
        if status {
            assertion.add_child_assertion(time_series_assertion("status", true));
        }
        assertion
    }

    let assertion = diagnostics_assertions::tree_assertion!("root": {
        "sampled_stats": {
            "interfaces": {
                interface_stats_tree_assertion(lo_id, &lo_name, false),
                interface_stats_tree_assertion(eth.id(), ETH_NAME, true),
            }
        }
    });

    // The statistics are sampled periodically and we have no means of
    // synchronizing so we have to give it a few tries until we match on the
    // expected value.
    const WAIT_INTERVAL: Duration = Duration::from_secs(1);
    const MAX_ATTEMPTS: usize = 20;
    for i in 1..=MAX_ATTEMPTS {
        let data = get_inspect_data(&realm, "netstack", "root/sampled_stats")
            .await
            .expect("inspect data should be present");
        match assertion.run(&data) {
            Ok(()) => break,
            Err(err) => {
                if i == MAX_ATTEMPTS {
                    panic!("failed to observe samples {err:?}");
                }
            }
        }
        fuchsia_async::Timer::new(WAIT_INTERVAL).await;
    }
}
