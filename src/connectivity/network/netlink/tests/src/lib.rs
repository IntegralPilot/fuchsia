// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for the integration between the netlink worker and netstack FIDL APIs, via a hermetic
//! netemul realm.

use std::borrow::Cow;
use std::collections::HashSet;
use std::num::NonZeroU64;

use assert_matches::assert_matches;
use fidl::endpoints::Proxy as _;
use fuchsia_async::{self as fasync, TimeoutExt};

use futures::channel::{mpsc, oneshot};
use futures::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _, StreamExt as _};
use linux_uapi::{
    rt_class_t_RT_TABLE_COMPAT, rt_class_t_RT_TABLE_MAIN, rtnetlink_groups_RTNLGRP_IPV4_ROUTE,
    rtnetlink_groups_RTNLGRP_IPV6_ROUTE, rtnetlink_groups_RTNLGRP_ND_USEROPT,
};
use net_declare::{fidl_mac, net_ip_v6, std_ip};
use net_types::Witness;
use net_types::ip::{Ip, IpAddress, IpVersion, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use netemul::{RealmTcpListener as _, RealmTcpStream as _, RealmUdpSocket as _, TestRealm};
use netlink::messaging::{
    AccessControl, MessageWithPermission, NetlinkMessageWithCreds, Permission,
};
use netlink::multicast_groups::ModernGroup;
use netlink::protocol_family::NetlinkClient as _;
use netlink_packet_core::{
    ErrorMessage, NLM_F_ACK, NLM_F_REQUEST, NetlinkDeserializable, NetlinkMessage, NetlinkPayload,
    NetlinkSerializable,
};
use netlink_packet_route::route::{
    RouteAttribute, RouteFlags, RouteHeader, RouteMessage, RouteProtocol, RouteScope, RouteType,
};
use netlink_packet_route::rule::{RuleAction, RuleAttribute, RuleFlags, RuleHeader, RuleMessage};
use netlink_packet_route::{AddressFamily, RouteNetlinkMessage};
use netlink_packet_sock_diag::inet::{ExtensionFlags, InetRequest, SocketId, StateFlags};
use netlink_packet_sock_diag::{SockDiagRequest, SockDiagResponse};
use netlink_packet_utils::DecodeError;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt};
use netstack_testing_common::{
    ASYNC_EVENT_CHECK_INTERVAL, ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT,
    ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
};
use netstack_testing_macros::netstack_test;
use packet_formats::icmp::ndp as packet_formats_ndp;
use smallvec::smallvec;
use test_case::{test_case, test_matrix};

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_ext::FromExt as _;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_ndp as fnet_ndp;
use fidl_fuchsia_net_neighbor as fnet_neighbor;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_routes_ext::FidlRouteIpExt;
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;
use fidl_fuchsia_net_routes_ext::rules::FidlRuleIpExt;
use fidl_fuchsia_net_sockets as fnet_sockets;
use fidl_fuchsia_posix_socket as fposix_socket;

fn connect_to_netlink_protocols_in_realm(
    realm: &TestRealm<'_>,
) -> netlink::NetlinkWorkerDiscoverableProtocols {
    let root_interfaces = realm
        .connect_to_protocol::<fnet_root::InterfacesMarker>()
        .expect("connect to fuchsia.net.root.Interfaces");
    let interfaces_state = realm
        .connect_to_protocol::<fnet_interfaces::StateMarker>()
        .expect("connect to fuchsia.net.interfaces");
    let v4_routes_state = realm
        .connect_to_protocol::<<Ipv4 as fnet_routes_ext::FidlRouteIpExt>::StateMarker>()
        .expect("connect to fuchsia.net.routes");
    let v6_routes_state = realm
        .connect_to_protocol::<<Ipv6 as fnet_routes_ext::FidlRouteIpExt>::StateMarker>()
        .expect("connect to fuchsia.net.routes");
    let v4_main_route_table = realm.connect_to_protocol::<
        <Ipv4 as fnet_routes_ext::admin::FidlRouteAdminIpExt>::RouteTableMarker,
    >()
    .expect("connect to fuchsia.net.routes.admin");
    let v6_main_route_table = realm.connect_to_protocol::<
        <Ipv6 as fnet_routes_ext::admin::FidlRouteAdminIpExt>::RouteTableMarker,
    >()
    .expect("connect to fuchsia.net.routes.admin");
    let v4_route_table_provider = realm.connect_to_protocol::<
        <Ipv4 as fnet_routes_ext::admin::FidlRouteAdminIpExt>::RouteTableProviderMarker,
    >()
    .expect("connect to fuchsia.net.routes.admin");
    let v6_route_table_provider = realm.connect_to_protocol::<
        <Ipv6 as fnet_routes_ext::admin::FidlRouteAdminIpExt>::RouteTableProviderMarker,
    >()
    .expect("connect to fuchsia.net.routes.admin");
    let v4_rule_table = realm.connect_to_protocol::<
        <Ipv4 as fnet_routes_ext::rules::FidlRuleAdminIpExt>::RuleTableMarker,
    >()
    .expect("connect to fuchsia.net.routes.admin");
    let v6_rule_table = realm.connect_to_protocol::<
        <Ipv6 as fnet_routes_ext::rules::FidlRuleAdminIpExt>::RuleTableMarker,
    >()
    .expect("connect to fuchsia.net.routes.admin");
    let ndp_option_watcher_provider = realm
        .connect_to_protocol::<fnet_ndp::RouterAdvertisementOptionWatcherProviderMarker>()
        .expect("connect to fuchsia.net.ndp");
    let socket_diagnostics = realm
        .connect_to_protocol::<fnet_sockets::DiagnosticsMarker>()
        .expect("connect to fuchsia.net.sockets");
    let socket_control = realm
        .connect_to_protocol::<fnet_sockets::ControlMarker>()
        .expect("connect to fuchsia.net.sockets");
    let neighbors_view = realm
        .connect_to_protocol::<fnet_neighbor::ViewMarker>()
        .expect("connect to fuchsia.net.neighbor.View");
    let neighbors_controller = realm
        .connect_to_protocol::<fnet_neighbor::ControllerMarker>()
        .expect("connect to fuchsia.net.neighbor.Controller");

    netlink::NetlinkWorkerDiscoverableProtocols {
        root_interfaces,
        interfaces_state,
        v4_routes_state,
        v6_routes_state,
        v4_main_route_table,
        v6_main_route_table,
        v4_route_table_provider,
        v6_route_table_provider,
        v4_rule_table,
        v6_rule_table,
        ndp_option_watcher_provider,
        socket_diagnostics,
        socket_control,
        neighbors_view,
        neighbors_controller,
    }
}

struct NoopInterfacesHandler;

impl netlink::interfaces::InterfacesHandler for NoopInterfacesHandler {
    fn handle_new_link(&mut self, _name: &str, _interface_id: NonZeroU64) {}

    fn handle_deleted_link(&mut self, _name: &str) {}
}

#[derive(Clone, Debug)]
struct SentNetlinkMessage<M> {
    message: NetlinkMessage<M>,
    group: Option<ModernGroup>,
}

#[derive(Clone)]
struct Sender<M>(mpsc::UnboundedSender<M>);

impl<M> Sender<M> {
    fn new_pair() -> (Self, Receiver<M>) {
        let (sender, receiver) = mpsc::unbounded();
        (Self(sender), receiver)
    }
}

impl<M: Clone + Send> netlink::messaging::Sender<M> for Sender<SentNetlinkMessage<M>> {
    fn send(&mut self, message: NetlinkMessage<M>, group: Option<ModernGroup>) {
        self.0
            .unbounded_send(SentNetlinkMessage { message, group })
            .expect("should not be disconnected")
    }
}

type Receiver<M> = mpsc::UnboundedReceiver<M>;

#[derive(Default, Debug, Clone)]
pub(crate) struct FakeCreds;

impl FakeCreds {
    fn attach<M: MessageWithPermission + NetlinkSerializable>(
        msg: NetlinkMessage<M>,
    ) -> NetlinkMessageWithCreds<NetlinkMessage<M>, Self> {
        NetlinkMessageWithCreds::new(msg, Self::default())
    }
}

#[derive(Default, Clone)]
pub(crate) struct FakeAccessControl {}

impl AccessControl<FakeCreds> for FakeAccessControl {
    fn grant_assess(&self, _creds: &FakeCreds, _perm: Permission) -> Result<(), netlink::Errno> {
        Ok(())
    }
}

enum NetlinkContext {}

impl netlink::messaging::NetlinkContext for NetlinkContext {
    type Creds = FakeCreds;
    type Sender<M: Clone + NetlinkSerializable + Send> = Sender<SentNetlinkMessage<M>>;
    type Receiver<
        M: Send + MessageWithPermission + NetlinkDeserializable<Error: Into<DecodeError>>,
    > = Receiver<NetlinkMessageWithCreds<NetlinkMessage<M>, FakeCreds>>;
    type AccessControl<'a> = FakeAccessControl;
}

struct NetlinkClient {
    client: netlink::protocol_family::route::NetlinkRouteClient,

    // `sender` and `receiver` do not contain the same types because the netlink worker's `Sender`
    // trait wants to be able to specify which group it's sending to, but its `Receiver` trait
    // just wants to receive messages with no group specification.
    sender: Sender<NetlinkMessageWithCreds<NetlinkMessage<RouteNetlinkMessage>, FakeCreds>>,
    receiver: Receiver<SentNetlinkMessage<RouteNetlinkMessage>>,
}

fn add_route_client(netlink: &netlink::Netlink<NetlinkContext>) -> NetlinkClient {
    let (server_sender, client_receiver) = Sender::new_pair();
    let (client_sender, server_receiver) = Sender::new_pair();
    let client = netlink
        .new_route_client(server_sender, server_receiver)
        .expect("should create new client successfully");
    NetlinkClient { client, sender: client_sender, receiver: client_receiver }
}

struct TestPeer<'a> {
    _network: netemul::TestNetwork<'a>,
    main_interface: netemul::TestInterface<'a>,
    peer_realm: netemul::TestRealm<'a>,
    _peer_interface: netemul::TestInterface<'a>,
}

impl<'a> TestPeer<'a> {
    async fn create(
        sandbox: &'a netemul::TestSandbox,
        main_realm: &netemul::TestRealm<'a>,
        peer_realm: netemul::TestRealm<'a>,
        name_suffix: &str,
        main_address: fnet::Subnet,
        peer_address: fnet::Subnet,
    ) -> TestPeer<'a> {
        let network = sandbox
            .create_network(format!("network{}", name_suffix))
            .await
            .expect("create network");
        let main_interface = main_realm
            .join_network(&network, format!("ep{}", name_suffix))
            .await
            .expect("join network");
        main_interface.add_address(main_address).await.expect("add address to main");
        let peer_interface = peer_realm
            .join_network(&network, format!("peerep{}", name_suffix))
            .await
            .expect("join network");
        peer_interface
            .add_address_and_subnet_route(peer_address)
            .await
            .expect("add address to peer");
        TestPeer { _network: network, main_interface, peer_realm, _peer_interface: peer_interface }
    }
}

/// Identifies one of multiple possible subnets used during a test. Each subnet has a "main" address
/// (for the local/sending netstack against which we are exercising the netlink worker) and a "peer"
/// address (for the remote/receiving netstack).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestSubnet {
    A,
    B,
    C,
}

const TEST_SUBNET_LENGTH: u8 = 24;
const TEST_SUBNETS: [TestSubnet; 3] = [TestSubnet::A, TestSubnet::B, TestSubnet::C];

impl TestSubnet {
    fn peer_subnet_byte(&self) -> u8 {
        match self {
            Self::A => 1,
            Self::B => 2,
            Self::C => 3,
        }
    }

    fn table_index(&self) -> u8 {
        self.peer_subnet_byte()
    }

    fn mark(&self) -> u32 {
        self.peer_subnet_byte().into()
    }

    fn subnet<I: Ip>(&self) -> fnet::IpAddress {
        match I::VERSION {
            IpVersion::V4 => fnet::IpAddress::Ipv4(fnet::Ipv4Address {
                addr: [192, 168, self.peer_subnet_byte(), 0],
            }),
            IpVersion::V6 => fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                addr: [0x20, 0, self.peer_subnet_byte(), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            }),
        }
    }

    fn main_address<I: Ip>(&self) -> fnet::IpAddress {
        match I::VERSION {
            IpVersion::V4 => fnet::IpAddress::Ipv4(fnet::Ipv4Address {
                addr: [192, 168, self.peer_subnet_byte(), 1],
            }),
            IpVersion::V6 => fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                addr: [0x20, 0, self.peer_subnet_byte(), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            }),
        }
    }

    fn peer_address<I: Ip>(&self) -> fnet::IpAddress {
        match I::VERSION {
            IpVersion::V4 => fnet::IpAddress::Ipv4(fnet::Ipv4Address {
                addr: [192, 168, self.peer_subnet_byte(), 2],
            }),
            IpVersion::V6 => fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                addr: [0x20, 0, self.peer_subnet_byte(), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2],
            }),
        }
    }
}

fn address_family<I: Ip>() -> AddressFamily {
    match I::VERSION {
        IpVersion::V4 => AddressFamily::Inet,
        IpVersion::V6 => AddressFamily::Inet6,
    }
}

fn route_group<I: Ip>() -> ModernGroup {
    ModernGroup(match I::VERSION {
        IpVersion::V4 => rtnetlink_groups_RTNLGRP_IPV4_ROUTE,
        IpVersion::V6 => rtnetlink_groups_RTNLGRP_IPV6_ROUTE,
    })
}

const DEFAULT_ROUTE_PRIORITY: u32 = 0;

fn create_route_in_table<I: Ip>(
    table: u32,
    test_subnet: TestSubnet,
    interface_id: u32,
    priority: u32,
) -> RouteMessage {
    let mut route_message = RouteMessage::default();
    let need_table_attr = table > (u8::MAX as u32);
    route_message.header = RouteHeader {
        address_family: address_family::<I>(),
        destination_prefix_length: TEST_SUBNET_LENGTH,
        source_prefix_length: 0,
        tos: 0,
        table: if need_table_attr { rt_class_t_RT_TABLE_COMPAT as u8 } else { table as u8 },
        protocol: RouteProtocol::Kernel,
        scope: RouteScope::Universe,
        kind: RouteType::Unicast,
        flags: RouteFlags::empty(),
    };

    let fnet_ext::IpAddress(destination) = test_subnet.subnet::<I>().into();

    route_message.attributes.extend([
        RouteAttribute::Destination(destination.into()),
        RouteAttribute::Oif(interface_id),
        RouteAttribute::Priority(priority),
    ]);
    if need_table_attr {
        route_message.attributes.push(RouteAttribute::Table(table));
    }
    route_message
}

async fn wait_for_message_outcome<I: Ip>(
    client: &mut NetlinkClient,
    mut sent_message: RouteNetlinkMessage,
) {
    match &mut sent_message {
        RouteNetlinkMessage::NewRoute(m) | RouteNetlinkMessage::DelRoute(m) => {
            m.attributes.iter_mut().for_each(|att| match att {
                // We need to massage the priority on the expected return
                // message given the handling in netlink for the default
                // priority.
                RouteAttribute::Priority(priority) => {
                    if *priority != DEFAULT_ROUTE_PRIORITY {
                        return;
                    }
                    *priority = match I::VERSION {
                        IpVersion::V4 => netlink::routes::DEFAULT_IPV4_ROUTE_PRIORITY,
                        IpVersion::V6 => netlink::routes::DEFAULT_IPV6_ROUTE_PRIORITY,
                    };
                }
                _ => {}
            })
        }
        m => unimplemented!("waiting for outcome of {m:?} not implemented"),
    }

    let receiver = &mut client.receiver;
    // We then receive notification of the route being added to the table we actually requested.
    let SentNetlinkMessage { message: _, group } = receiver
        .filter_map(|received| {
            let msg = assert_matches!(
                &received.message.payload, NetlinkPayload::InnerMessage(msg) => msg);
            futures::future::ready((msg == &sent_message).then_some(received))
        })
        .next()
        .await
        .expect("should not be disconnected");
    assert_eq!(group, Some(route_group::<I>()));
}

async fn add_route_in_table_and_await_installed<I: Ip>(
    client: &mut NetlinkClient,
    test_subnet: TestSubnet,
    interface_id: u32,
    table_id: u32,
    priority: u32,
) {
    let new_route_message = RouteNetlinkMessage::NewRoute(create_route_in_table::<I>(
        table_id,
        test_subnet,
        interface_id,
        priority,
    ));
    let mut message: NetlinkMessage<RouteNetlinkMessage> = new_route_message.clone().into();
    message.finalize();
    client.sender.0.unbounded_send(FakeCreds::attach(message)).expect("should not be disconnected");
    wait_for_message_outcome::<I>(client, new_route_message).await;
}

async fn remove_route_in_table_and_await_uninstalled<I: Ip>(
    client: &mut NetlinkClient,
    test_subnet: TestSubnet,
    interface_id: u32,
    table_id: u32,
    priority: u32,
) {
    let remove_route_message = RouteNetlinkMessage::DelRoute(create_route_in_table::<I>(
        table_id,
        test_subnet,
        interface_id,
        priority,
    ));
    let mut message: NetlinkMessage<RouteNetlinkMessage> = remove_route_message.clone().into();
    message.finalize();
    client.sender.0.unbounded_send(FakeCreds::attach(message)).expect("should not be disconnected");
    wait_for_message_outcome::<I>(client, remove_route_message).await;
}

async fn add_route_and_await_installed<I: Ip>(
    client: &mut NetlinkClient,
    test_subnet: TestSubnet,
    interface_id: u32,
) {
    add_route_in_table_and_await_installed::<I>(
        client,
        test_subnet,
        interface_id,
        test_subnet.table_index().into(),
        DEFAULT_ROUTE_PRIORITY,
    )
    .await
}

async fn start_test_netlink_with_interfaces_handler(
    realm: &TestRealm<'_>,
    interfaces_handler: impl netlink::interfaces::InterfacesHandler,
) -> (netlink::Netlink<NetlinkContext>, fasync::Task<()>) {
    let protocols = connect_to_netlink_protocols_in_realm(&realm);
    let (on_initialized, initialized) = oneshot::channel();
    let (netlink, worker_params) = netlink::Netlink::<NetlinkContext>::new(interfaces_handler);
    let worker = netlink::run_netlink_worker_with_protocols(
        worker_params,
        protocols,
        Some(on_initialized),
        Default::default(),
    );
    let join_handle = fasync::Task::spawn(worker);
    initialized.await.expect("should not be dropped");
    (netlink, join_handle)
}

async fn start_test_netlink(
    realm: &TestRealm<'_>,
) -> (netlink::Netlink<NetlinkContext>, fasync::Task<()>) {
    start_test_netlink_with_interfaces_handler(realm, NoopInterfacesHandler).await
}

#[netstack_test]
#[variant(I, Ip)]
async fn rules_select_correct_table_for_marked_socket<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let main_realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let (netlink, _join_handle) = start_test_netlink(&main_realm).await;

    let create_peer = |test_subnet: TestSubnet| {
        let sandbox = &sandbox;
        let main_realm = &main_realm;
        async move {
            let name_suffix = format!("{test_subnet:?}");
            let peer_realm = sandbox
                .create_netstack_realm::<Netstack3, _>(format!("{name}-{test_subnet:?}"))
                .expect("create peer realm");
            let peer = TestPeer::create(
                sandbox,
                main_realm,
                peer_realm,
                name_suffix.as_str(),
                fnet::Subnet {
                    prefix_len: TEST_SUBNET_LENGTH,
                    addr: test_subnet.main_address::<I>(),
                },
                fnet::Subnet {
                    prefix_len: TEST_SUBNET_LENGTH,
                    addr: test_subnet.peer_address::<I>(),
                },
            )
            .await;
            peer
        }
    };

    let test_peers = futures::stream::iter(TEST_SUBNETS)
        .then(|test_subnet| create_peer(test_subnet).map(move |peer| (test_subnet, peer)))
        .collect::<Vec<_>>()
        .await;

    let mut client = add_route_client(&netlink);
    assert!(
        client
            .client
            .add_membership(route_group::<I>())
            .expect("should add membership successfully")
            .is_noop(),
        "should not produce blocking work"
    );

    for &(test_subnet, ref peer) in &test_peers {
        add_route_and_await_installed::<I>(
            &mut client,
            test_subnet,
            peer.main_interface.id().try_into().unwrap(),
        )
        .await;
    }

    let mut add_route = async |header, attributes| {
        let new_rule_message = RouteNetlinkMessage::NewRule({
            let mut rule_message = RuleMessage::default();
            rule_message.header = header;
            rule_message.attributes.extend(attributes);
            rule_message
        });
        let mut message: NetlinkMessage<RouteNetlinkMessage> = new_rule_message.into();
        message.header.flags |= NLM_F_ACK;
        message.finalize();
        client
            .sender
            .0
            .unbounded_send(FakeCreds::attach(message))
            .expect("should not be disconnected");

        // Wait for the ACK.
        let SentNetlinkMessage { message: received_msg, group: _ } =
            client.receiver.next().await.expect("should not be disconnected");
        assert_matches!(
            received_msg.payload,
            NetlinkPayload::Error(ErrorMessage { code: None, .. })
        )
    };

    // We install a default rule to drop all traffic, which helps us prove we only see traffic if
    // sockets are correctly marked.
    add_route(
        RuleHeader {
            family: address_family::<I>(),
            dst_len: 0,
            src_len: 0,
            tos: 0,
            table: 0,
            action: RuleAction::Unreachable,
            flags: RuleFlags::empty(),
        },
        [RuleAttribute::Priority(10), RuleAttribute::FwMark(0), RuleAttribute::FwMask(0)],
    )
    .await;

    for &(test_subnet, ref _peer) in &test_peers {
        add_route(
            RuleHeader {
                family: address_family::<I>(),
                dst_len: 0,
                src_len: 0,
                tos: 0,
                table: test_subnet.table_index(),
                action: RuleAction::ToTable,
                flags: RuleFlags::empty(),
            },
            [
                RuleAttribute::Priority(9),
                RuleAttribute::FwMark(test_subnet.mark()),
                RuleAttribute::FwMask(u32::MAX),
            ],
        )
        .await;
    }

    let provider = main_realm
        .connect_to_protocol::<fposix_socket::ProviderMarker>()
        .expect("connect to fuchsia.posix.socket.Provider");

    let create_socket_with_fwmark = |fwmark: Option<u32>| {
        let provider = &provider;
        async move {
            let response = provider
                .datagram_socket(
                    match I::VERSION {
                        IpVersion::V4 => fposix_socket::Domain::Ipv4,
                        IpVersion::V6 => fposix_socket::Domain::Ipv6,
                    },
                    fposix_socket::DatagramSocketProtocol::Udp,
                )
                .await
                .expect("should not have FIDL error")
                .expect("should not get error");
            let socket_proxy = match response {
                fposix_socket::ProviderDatagramSocketResponse::SynchronousDatagramSocket(
                    client_end,
                ) => client_end.into_proxy(),
                _ => unreachable!("netstack3 does not implement fast UDP yet"),
            };

            if let Some(fwmark) = fwmark {
                socket_proxy
                    .set_mark(
                        fnet::MarkDomain::Mark1,
                        &fposix_socket::OptionalUint32::Value(fwmark),
                    )
                    .await
                    .expect("should not get FIDL error")
                    .expect("should not get error");
            }

            let socket = socket2::Socket::from(
                fdio::create_fd(
                    socket_proxy
                        .into_client_end()
                        .expect("should successfully get back client end")
                        .into(),
                )
                .expect("create fd should succeed"),
            );
            fasync::net::UdpSocket::from_datagram(
                fasync::net::DatagramSocket::new_from_socket(socket)
                    .expect("should successfully create async socket from socket2 socket"),
            )
            .expect("should successfully create UDP socket")
        }
    };

    let peer_sockets = futures::stream::iter(test_peers.iter())
        .then(|&(test_subnet, ref peer)| async move {
            let fnet_ext::IpAddress(peer_addr) = test_subnet.peer_address::<I>().into();
            let socket_addr = std::net::SocketAddr::new(peer_addr, 1234);
            let socket = fasync::net::UdpSocket::bind_in_realm(&peer.peer_realm, socket_addr)
                .await
                .expect("should successfully bind UDP socket");
            (test_subnet, socket_addr, socket)
        })
        .collect::<Vec<_>>()
        .await;

    // A 0-marked socket should not be able to reach any of the peers.
    let socket = create_socket_with_fwmark(Some(0)).await;

    for &(_test_subnet, peer_addr, ref _peer_socket) in &peer_sockets {
        let message = format!("hello {peer_addr:?}");
        let result = socket.send_to(message.as_bytes(), peer_addr).await;
        let error = assert_matches!(result, Err(e) => e);
        assert_matches!(error.kind(), std::io::ErrorKind::NetworkUnreachable);
    }

    // A socket with the corresponding mark should be able to reach each of the peers, but not the
    // others.
    let mut buf = [0u8; 64];
    for &(test_subnet, peer_addr, ref peer_socket) in &peer_sockets {
        for mark in TEST_SUBNETS
            .into_iter()
            .map(|test_subnet| Some(test_subnet.mark()))
            .chain(std::iter::once(None))
        {
            let socket = create_socket_with_fwmark(mark).await;

            let expect_success = mark.map(|mark| mark == test_subnet.mark()).unwrap_or(false);

            let message = format!("hello {peer_addr:?} from mark {mark:?}");
            let result = socket.send_to(message.as_bytes(), peer_addr).await;
            if expect_success {
                let (n, _from_addr) =
                    peer_socket.recv_from(&mut buf).await.expect("should successfully receive");
                assert_eq!(&buf[..n], message.as_bytes());
            } else {
                let error = assert_matches!(result, Err(e) => e);
                assert_matches!(error.kind(), std::io::ErrorKind::NetworkUnreachable);
            }
        }
    }
}

#[netstack_test]
#[variant(I, Ip)]
async fn successfully_installs_rule_referencing_main_table<
    I: Ip + FidlRuleIpExt + FidlRouteIpExt + FidlRouteAdminIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let main_realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let (netlink, _join_handle) = start_test_netlink(&main_realm).await;

    let client = add_route_client(&netlink);

    let state = &main_realm.connect_to_protocol::<I::StateMarker>().expect("connect to protocol");
    let mut rules_event_stream = std::pin::pin!(
        fnet_routes_ext::rules::rule_event_stream_from_state::<I>(state).expect("get rule watcher")
    );
    let _preexisting_rules = fnet_routes_ext::rules::collect_rules_until_idle::<I, HashSet<_>>(
        rules_event_stream.by_ref(),
    )
    .await
    .expect("collect rules until idle");

    fn address_family<I: Ip>() -> AddressFamily {
        match I::VERSION {
            IpVersion::V4 => AddressFamily::Inet,
            IpVersion::V6 => AddressFamily::Inet6,
        }
    }

    const MARK: u32 = 1234;
    const PRIORITY: u32 = 5678;

    let create_rule_to_main_table = || {
        let mut rule_message = RuleMessage::default();
        rule_message.header = RuleHeader {
            family: address_family::<I>(),
            dst_len: 0,
            src_len: 0,
            tos: 0,
            table: rt_class_t_RT_TABLE_MAIN as u8,
            action: RuleAction::ToTable,
            flags: RuleFlags::empty(),
        };
        rule_message.attributes.extend([
            RuleAttribute::Priority(PRIORITY),
            RuleAttribute::FwMark(MARK),
            RuleAttribute::FwMask(u32::MAX),
        ]);
        rule_message
    };

    // Install one rule referencing the table.
    let mut new_rule_message: NetlinkMessage<_> =
        RouteNetlinkMessage::NewRule(create_rule_to_main_table()).into();
    new_rule_message.finalize();
    client
        .sender
        .0
        .unbounded_send(FakeCreds::attach(new_rule_message))
        .expect("should not be disconnected");

    // Await the rule's installation.
    let rule_event: fnet_routes_ext::rules::RuleEvent<I> = rules_event_stream
        .next()
        .await
        .expect("should not get FIDL error")
        .expect("should not get watcher error");

    let added_rule =
        assert_matches!(rule_event, fnet_routes_ext::rules::RuleEvent::Added(rule) => rule);
    // Assume that if the marks match, this is the corresponding rule.
    let (mark_range, table) = assert_matches!(added_rule,
        fnet_routes_ext::rules::InstalledRule {
            matcher: fnet_routes_ext::rules::RuleMatcher {
                mark_1: Some(
                    fnet_matchers_ext::Mark::Marked { mask: u32::MAX, between, invert: false}
                ),
                ..
            },
            action: fnet_routes_ext::rules::RuleAction::Lookup(table),
            ..
    } => (between, table));

    assert_eq!(mark_range, MARK..=MARK);

    // Check that the rule does actually target the main table.
    let main_table_proxy =
        main_realm.connect_to_protocol::<I::RouteTableMarker>().expect("connect to protocol");
    let main_table_id =
        fnet_routes_ext::admin::get_table_id::<I>(&main_table_proxy).await.expect("should succeed");
    assert_eq!(main_table_id, table);
}

async fn await_disappearance_of_table(
    routes_state: &fnet_routes::StateProxy,
    table: fnet_routes_ext::TableId,
    table_name: &str,
) {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

    loop {
        let get_name_result = routes_state
            .get_route_table_name(table.get())
            .await
            .expect("should not get FIDL error");

        match get_name_result {
            Ok(name) => {
                assert_eq!(&name, table_name);
                fasync::Timer::new(POLL_INTERVAL).await;
            }
            Err(fnet_routes::StateGetRouteTableNameError::NoTable) => break,
        }
    }
}

#[netstack_test]
#[variant(I, Ip)]
async fn route_table_kept_alive_by_rules<I: Ip + FidlRuleIpExt + FidlRouteIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let main_realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let (netlink, _join_handle) = start_test_netlink(&main_realm).await;

    let client = add_route_client(&netlink);

    let state = &main_realm.connect_to_protocol::<I::StateMarker>().expect("connect to protocol");
    let mut rules_event_stream = std::pin::pin!(
        fnet_routes_ext::rules::rule_event_stream_from_state::<I>(state).expect("get rule watcher")
    );
    let _preexisting_rules = fnet_routes_ext::rules::collect_rules_until_idle::<I, HashSet<_>>(
        rules_event_stream.by_ref(),
    )
    .await
    .expect("collect rules until idle");

    fn address_family<I: Ip>() -> AddressFamily {
        match I::VERSION {
            IpVersion::V4 => AddressFamily::Inet,
            IpVersion::V6 => AddressFamily::Inet6,
        }
    }

    const TABLE: u8 = 42;
    const MARK: u32 = 1234;
    const PRIORITIES: [u32; 2] = [11, 12];

    let create_rule_to_table = |priority| {
        let mut rule_message = RuleMessage::default();
        rule_message.header = RuleHeader {
            family: address_family::<I>(),
            dst_len: 0,
            src_len: 0,
            tos: 0,
            table: TABLE,
            action: RuleAction::ToTable,
            flags: RuleFlags::empty(),
        };
        rule_message.attributes.extend([
            RuleAttribute::Priority(priority),
            RuleAttribute::FwMark(MARK),
            RuleAttribute::FwMask(u32::MAX),
        ]);
        rule_message
    };

    // Install one rule referencing the table.
    let mut new_rule_message: NetlinkMessage<_> =
        RouteNetlinkMessage::NewRule(create_rule_to_table(PRIORITIES[0])).into();
    new_rule_message.finalize();
    client
        .sender
        .0
        .unbounded_send(FakeCreds::attach(new_rule_message))
        .expect("should not be disconnected");

    // Await the rule's installation.
    let rule_event: fnet_routes_ext::rules::RuleEvent<I> = rules_event_stream
        .next()
        .await
        .expect("should not get FIDL error")
        .expect("should not get watcher error");

    let added_rule =
        assert_matches!(rule_event, fnet_routes_ext::rules::RuleEvent::Added(rule) => rule);
    // Assume that if the marks match, this is the corresponding rule.
    let (mark_range, table) = assert_matches!(added_rule, fnet_routes_ext::rules::InstalledRule {
        matcher: fnet_routes_ext::rules::RuleMatcher {
            mark_1: Some(fnet_matchers_ext::Mark::Marked { mask: u32::MAX, between, invert: false}),
            ..
        },
        action: fnet_routes_ext::rules::RuleAction::Lookup(table),
        ..
    } => (between, table));

    assert_eq!(mark_range, MARK..=MARK);

    // Install another rule referencing the table.
    let mut new_rule_message: NetlinkMessage<_> =
        RouteNetlinkMessage::NewRule(create_rule_to_table(PRIORITIES[1])).into();
    new_rule_message.finalize();
    client
        .sender
        .0
        .unbounded_send(FakeCreds::attach(new_rule_message))
        .expect("should not be disconnected");

    // Await the rule's installation.
    let rule_event: fnet_routes_ext::rules::RuleEvent<I> = rules_event_stream
        .next()
        .await
        .expect("should not get FIDL error")
        .expect("should not get watcher error");

    let added_rule =
        assert_matches!(rule_event, fnet_routes_ext::rules::RuleEvent::Added(rule) => rule);

    let (mark_range, other_table) = assert_matches!(added_rule,
        fnet_routes_ext::rules::InstalledRule {
            matcher: fnet_routes_ext::rules::RuleMatcher {
                mark_1: Some(
                    fnet_matchers_ext::Mark::Marked { mask: u32::MAX, between, invert: false}
                ),
                ..
            },
            action: fnet_routes_ext::rules::RuleAction::Lookup(table),
            ..
        } => (between, table)
    );

    assert_eq!(mark_range, MARK..=MARK);
    assert_eq!(table, other_table);

    let routes_state = main_realm
        .connect_to_protocol::<fnet_routes::StateMarker>()
        .expect("connect to fuchsia.net.routes.State");
    let routes_state = &routes_state;

    let netlink_table_name = routes_state
        .get_route_table_name(table.get())
        .await
        .expect("should not get FIDL error")
        .expect("table should be present");
    let netlink_table_name = &netlink_table_name;

    // Check that removing one of the rules does not cause the table to be removed.
    let mut del_rule_message: NetlinkMessage<_> =
        RouteNetlinkMessage::DelRule(create_rule_to_table(PRIORITIES[0])).into();
    del_rule_message.finalize();
    client
        .sender
        .0
        .unbounded_send(FakeCreds::attach(del_rule_message))
        .expect("should not be disconnected");

    // Await the rule's removal.
    let rule_event: fnet_routes_ext::rules::RuleEvent<I> = rules_event_stream
        .next()
        .await
        .expect("should not get FIDL error")
        .expect("should not get watcher error");

    let removed_rule =
        assert_matches!(rule_event, fnet_routes_ext::rules::RuleEvent::Removed(rule) => rule);
    let (mark_range, removal_referenced_table) = assert_matches!(
        removed_rule,
        fnet_routes_ext::rules::InstalledRule {
            matcher: fnet_routes_ext::rules::RuleMatcher {
                mark_1: Some(
                    fnet_matchers_ext::Mark::Marked { mask: u32::MAX, between, invert: false}
                ),
                ..
            },
            action: fnet_routes_ext::rules::RuleAction::Lookup(table),
            ..
        } => (between, table)
    );

    assert_eq!(mark_range, MARK..=MARK);
    assert_eq!(removal_referenced_table, table);

    await_disappearance_of_table(routes_state, table, netlink_table_name)
        .map(|()| panic!("table should not be removed"))
        .on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, || ())
        .await;

    // Now if we remove the second rule, the table should be removed.
    let mut del_rule_message: NetlinkMessage<_> =
        RouteNetlinkMessage::DelRule(create_rule_to_table(PRIORITIES[1])).into();
    del_rule_message.finalize();
    client
        .sender
        .0
        .unbounded_send(FakeCreds::attach(del_rule_message))
        .expect("should not be disconnected");
    let rule_event: fnet_routes_ext::rules::RuleEvent<I> = rules_event_stream
        .next()
        .await
        .expect("should not get FIDL error")
        .expect("should not get watcher error");

    let removed_rule =
        assert_matches!(rule_event, fnet_routes_ext::rules::RuleEvent::Removed(rule) => rule);
    let (mark_range, removal_referenced_table) = assert_matches!(
        removed_rule,
        fnet_routes_ext::rules::InstalledRule {
            matcher: fnet_routes_ext::rules::RuleMatcher {
                mark_1: Some(
                    fnet_matchers_ext::Mark::Marked { mask: u32::MAX, between, invert: false}
                ),
                ..
            },
            action: fnet_routes_ext::rules::RuleAction::Lookup(table),
            ..
        } => (between, table)
    );

    assert_eq!(mark_range, MARK..=MARK);
    assert_eq!(removal_referenced_table, table);

    await_disappearance_of_table(routes_state, table, netlink_table_name)
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || panic!("table should be removed"))
        .await;
}

#[derive(Debug, Copy, Clone)]
enum Order {
    RuleThenRoute,
    RouteThenRule,
}

#[netstack_test]
#[variant(I, Ip)]
#[test_matrix(
    [Order::RuleThenRoute, Order::RouteThenRule],
    [Order::RuleThenRoute, Order::RouteThenRule]
)]
async fn route_table_is_cleaned_up_after_rules_and_routes_deleted<
    I: Ip + FidlRuleIpExt + FidlRouteIpExt,
>(
    name: &str,
    add_order: Order,
    remove_order: Order,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let main_realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let network = sandbox.create_network("network").await.expect("create network");
    let main_interface = main_realm.join_network(&network, "ep").await.expect("join network");

    let (netlink, _join_handle) = start_test_netlink(&main_realm).await;

    let mut client = add_route_client(&netlink);
    assert!(
        client
            .client
            .add_membership(route_group::<I>())
            .expect("should add membership successfully")
            .is_noop(),
        "should not produce blocking work"
    );

    fn destination<I: Ip>() -> std::net::IpAddr {
        match I::VERSION {
            IpVersion::V4 => net_declare::std_ip!("192.168.0.0"),
            IpVersion::V6 => net_declare::std_ip!("2000::"),
        }
    }

    let interface_id = u32::try_from(main_interface.id()).unwrap();

    const TABLE: u8 = 42;

    let create_route_in_table = move |table| {
        let mut route_message = RouteMessage::default();
        route_message.header = RouteHeader {
            address_family: address_family::<I>(),
            destination_prefix_length: TEST_SUBNET_LENGTH,
            source_prefix_length: 0,
            tos: 0,
            table,
            protocol: RouteProtocol::Kernel,
            scope: RouteScope::Universe,
            kind: RouteType::Unicast,
            flags: RouteFlags::empty(),
        };
        route_message.attributes.extend([
            RouteAttribute::Destination(destination::<I>().into()),
            RouteAttribute::Oif(interface_id),
            RouteAttribute::Priority(1),
        ]);
        route_message
    };
    let create_route_in_table = &create_route_in_table;

    const MARK: u32 = 1234;

    let create_rule_to_table = || {
        let mut rule_message = RuleMessage::default();
        rule_message.header = RuleHeader {
            family: address_family::<I>(),
            dst_len: 0,
            src_len: 0,
            tos: 0,
            table: TABLE,
            action: RuleAction::ToTable,
            flags: RuleFlags::empty(),
        };
        rule_message.attributes.extend([
            RuleAttribute::Priority(9),
            RuleAttribute::FwMark(MARK),
            RuleAttribute::FwMask(u32::MAX),
        ]);
        rule_message
    };
    let create_rule_to_table = &create_rule_to_table;

    let add_route_and_await_installed = |mut client: NetlinkClient| {
        let mut netlink_route_message: NetlinkMessage<RouteNetlinkMessage> =
            RouteNetlinkMessage::NewRoute(create_route_in_table(TABLE)).into();
        netlink_route_message.finalize();

        client
            .sender
            .0
            .unbounded_send(FakeCreds::attach(netlink_route_message))
            .expect("should not be disconnected");
        async move {
            // We then receive notification of the route being added to the table we actually
            // requested.
            let SentNetlinkMessage { message: received_msg, group } =
                client.receiver.next().await.expect("should not be disconnected");
            assert_eq!(group, Some(route_group::<I>()));
            let received_route_message = assert_matches!(received_msg.payload,
                NetlinkPayload::InnerMessage(message) => message
            );

            assert_eq!(
                received_route_message,
                RouteNetlinkMessage::NewRoute(create_route_in_table(TABLE))
            );

            client
        }
    };

    let remove_route_and_await_uninstalled = |mut client: NetlinkClient| {
        let mut netlink_route_message: NetlinkMessage<RouteNetlinkMessage> =
            RouteNetlinkMessage::DelRoute(create_route_in_table(TABLE)).into();
        netlink_route_message.finalize();
        let netlink_route_message = netlink_route_message;

        client
            .sender
            .0
            .unbounded_send(FakeCreds::attach(netlink_route_message))
            .expect("should not be disconnected");
        async move {
            // We then receive notification of the route being added to the table we actually
            // requested.
            let SentNetlinkMessage { message: received_msg, group } =
                client.receiver.next().await.expect("should not be disconnected");
            assert_eq!(group, Some(route_group::<I>()));
            let received_route_message = assert_matches!(received_msg.payload,
                NetlinkPayload::InnerMessage(message) => message
            );

            assert_eq!(
                received_route_message,
                RouteNetlinkMessage::DelRoute(create_route_in_table(TABLE))
            );

            client
        }
    };

    let state = &main_realm.connect_to_protocol::<I::StateMarker>().expect("connect to protocol");
    let mut rules_event_stream = std::pin::pin!(
        fnet_routes_ext::rules::rule_event_stream_from_state::<I>(state).expect("get rule watcher")
    );
    let _preexisting_rules = fnet_routes_ext::rules::collect_rules_until_idle::<I, HashSet<_>>(
        rules_event_stream.by_ref(),
    )
    .await
    .expect("collect rules until idle");

    let mut routes_event_stream = std::pin::pin!(
        fnet_routes_ext::event_stream_from_state_with_options::<I>(
            state,
            fnet_routes_ext::WatcherOptions {
                table_interest: Some(fnet_routes::TableInterest::All(fnet_routes::All)),
            },
        )
        .expect("get routes watcher")
    );
    let _preexisting_routes =
        fnet_routes_ext::collect_routes_until_idle::<I, HashSet<_>>(routes_event_stream.by_ref())
            .await
            .expect("collect routes until idle");

    match add_order {
        Order::RouteThenRule => {
            client = add_route_and_await_installed(client).await;
        }
        Order::RuleThenRoute => {}
    }

    // Install the rule.
    let mut new_rule_message: NetlinkMessage<_> =
        RouteNetlinkMessage::NewRule(create_rule_to_table()).into();
    new_rule_message.finalize();
    client
        .sender
        .0
        .unbounded_send(FakeCreds::attach(new_rule_message))
        .expect("should not be disconnected");

    // Await the rule's installation.
    let rule_event: fnet_routes_ext::rules::RuleEvent<I> = rules_event_stream
        .next()
        .await
        .expect("should not get FIDL error")
        .expect("should not get watcher error");

    let added_rule =
        assert_matches!(rule_event, fnet_routes_ext::rules::RuleEvent::Added(rule) => rule);
    // Assume that if the marks match, this is the corresponding rule.
    let (mark_range, table) = assert_matches!(added_rule, fnet_routes_ext::rules::InstalledRule {
        matcher: fnet_routes_ext::rules::RuleMatcher {
            mark_1: Some(fnet_matchers_ext::Mark::Marked { mask: u32::MAX, between, invert: false}),
            ..
        },
        action: fnet_routes_ext::rules::RuleAction::Lookup(table),
        ..
    } => (between, table));

    assert_eq!(mark_range, MARK..=MARK);

    match add_order {
        Order::RouteThenRule => {}
        Order::RuleThenRoute => {
            client = add_route_and_await_installed(client).await;
        }
    }

    // Check that this agrees with the routes-watchers view of which table things were installed in.

    // Now we can check for the insertion into the "real" table.
    let route_event: fnet_routes_ext::Event<I> = routes_event_stream
        .next()
        .await
        .expect("should not get FIDL error")
        .expect("should not get watcher error");
    let added_route = assert_matches!(route_event, fnet_routes_ext::Event::Added(route) => route);
    let route_destination = net_types::ip::SubnetEither::from(added_route.route.destination);
    assert_eq!(
        route_destination,
        net_types::ip::SubnetEither::new(destination::<I>().into(), TEST_SUBNET_LENGTH).unwrap()
    );
    assert_eq!(added_route.table_id, table);

    let routes_state = main_realm
        .connect_to_protocol::<fnet_routes::StateMarker>()
        .expect("connect to fuchsia.net.routes.State");
    let routes_state = &routes_state;

    let netlink_table_name = routes_state
        .get_route_table_name(added_route.table_id.get())
        .await
        .expect("should not get FIDL error")
        .expect("table should be present");

    match remove_order {
        Order::RouteThenRule => {
            client = remove_route_and_await_uninstalled(client).await;

            // After only removing the route, the table should still be kept alive because there
            // is a rule referencing it.
            let got_name = routes_state
                .get_route_table_name(added_route.table_id.get())
                .await
                .expect("should not get FIDL error")
                .expect("table should still be alive");
            assert_eq!(got_name, netlink_table_name);
        }
        Order::RuleThenRoute => {}
    }

    // Remove the rule.
    let mut del_rule_message: NetlinkMessage<_> =
        RouteNetlinkMessage::DelRule(create_rule_to_table()).into();
    del_rule_message.finalize();
    client
        .sender
        .0
        .unbounded_send(FakeCreds::attach(del_rule_message))
        .expect("should not be disconnected");

    // Await the rule's removal.
    let rule_event: fnet_routes_ext::rules::RuleEvent<I> = rules_event_stream
        .next()
        .await
        .expect("should not get FIDL error")
        .expect("should not get watcher error");

    let removed_rule =
        assert_matches!(rule_event, fnet_routes_ext::rules::RuleEvent::Removed(rule) => rule);
    // Assume that if the marks match, this is the corresponding rule.
    let (mark_range, removal_referenced_table) = assert_matches!(
        removed_rule,
        fnet_routes_ext::rules::InstalledRule {
            matcher: fnet_routes_ext::rules::RuleMatcher {
                mark_1: Some(fnet_matchers_ext::Mark::Marked {
                    mask: u32::MAX,
                    between,
                    invert: false,
                }),
                ..
            },
            action: fnet_routes_ext::rules::RuleAction::Lookup(table),
            ..
        } => (between, table)
    );

    assert_eq!(mark_range, MARK..=MARK);
    assert_eq!(removal_referenced_table, table);

    match remove_order {
        Order::RouteThenRule => {}
        Order::RuleThenRoute => {
            // After only removing the rule, the table should still be kept alive because there
            // is a route referencing it.
            let got_name = routes_state
                .get_route_table_name(added_route.table_id.get())
                .await
                .expect("should not get FIDL error")
                .expect("table should still be alive");
            assert_eq!(got_name, netlink_table_name);

            client = remove_route_and_await_uninstalled(client).await;
        }
    }

    // Check for the route removal(s).
    let route_event: fnet_routes_ext::Event<I> = routes_event_stream
        .next()
        .await
        .expect("should not get FIDL error")
        .expect("should not get watcher error");
    let removed_route =
        assert_matches!(route_event, fnet_routes_ext::Event::Removed(route) => route);
    let route_destination = net_types::ip::SubnetEither::from(removed_route.route.destination);
    assert_eq!(
        route_destination,
        net_types::ip::SubnetEither::new(destination::<I>().into(), TEST_SUBNET_LENGTH).unwrap()
    );
    assert_eq!(removed_route.table_id, table);

    await_disappearance_of_table(routes_state, added_route.table_id, netlink_table_name.as_str())
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
            panic!("timed out waiting for route table to be deleted")
        })
        .await;
    let _ = client;
}

const MAC: fidl_fuchsia_net::MacAddress = fidl_mac!("02:00:01:02:03:04");
const SOURCE_ADDR: net_types::ip::Ipv6Addr = net_ip_v6!("fe80::1");

fn rdnss_option_builder() -> packet_formats_ndp::options::NdpOptionBuilder<'static> {
    const ADDRESSES: [net_types::ip::Ipv6Addr; 2] =
        [net_ip_v6!("2001:db8::1"), net_ip_v6!("2001:db8::2")];
    let option = packet_formats::icmp::ndp::options::RecursiveDnsServer::new(u32::MAX, &ADDRESSES);
    let builder = packet_formats::icmp::ndp::options::NdpOptionBuilder::RecursiveDnsServer(option);
    builder
}

#[netstack_test]
async fn join_leave_nduseropt_multicast_group(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let main_realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let (netlink, _join_handle) = start_test_netlink(&main_realm).await;

    let mut client = add_route_client(&netlink);
    let waiter = client
        .client
        .add_membership(ModernGroup(rtnetlink_groups_RTNLGRP_ND_USEROPT))
        .expect("should succeed");
    fasync::unblock(|| waiter.assert_blocking_and_wait_until_complete()).await;

    let network = sandbox.create_network("network").await.expect("failed to create network");

    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");
    let _iface = main_realm
        .join_network_with(
            &network,
            "iface",
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(MAC)),
            Default::default(),
        )
        .await
        .expect("failed to join network");

    // Wait to see an RS from the interface to confirm IPv6 is enabled.
    //
    // TODO(https://fxbug.dev/516793493): remove this after updating
    // `join_network_with` to wait until both IPv4 and IPv6 are enabled.
    netstack_testing_common::ndp::wait_for_router_solicitation(&fake_ep).await;

    let builder = rdnss_option_builder();
    let len = packet::records::RecordBuilder::serialized_len(&builder);
    let mut want_body = vec![0u8; len];
    packet::records::RecordBuilder::serialize_into(&builder, &mut want_body);

    netstack_testing_common::ndp::send_ra_with_router_lifetime(
        &fake_ep,
        u16::MAX,
        &[builder],
        SOURCE_ADDR,
    )
    .await
    .expect("should succeed");

    let message = client
        .receiver
        .next()
        .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
            panic!("client timeout while waiting to receive")
        })
        .await
        .expect("should receive message");
    let SentNetlinkMessage { message, group } = message;
    assert_eq!(
        group.expect("should be specified"),
        ModernGroup(rtnetlink_groups_RTNLGRP_ND_USEROPT)
    );
    let message =
        assert_matches!(message.payload, NetlinkPayload::InnerMessage(message) => message);
    let message = assert_matches!(
        message,
        RouteNetlinkMessage::NewNeighbourDiscoveryUserOption(message) => message
    );

    assert_eq!(message.option_body, want_body);
    let attr = assert_matches!(&message.attributes[..], [attr] => attr.clone());
    let addr_bytes = assert_matches!(attr,
        netlink_packet_route::neighbour_discovery_user_option::Nla::SourceLinkLocalAddress(
            addr_bytes,
        ) => addr_bytes);
    let addr = net_types::ip::Ipv6Addr::from_bytes(
        addr_bytes.try_into().expect("should be valid IPv6 bytes"),
    );
    assert_eq!(addr, SOURCE_ADDR);

    client
        .client
        .del_membership(ModernGroup(rtnetlink_groups_RTNLGRP_ND_USEROPT))
        .expect("del group should succeed");
}

#[netstack_test]
async fn netlink_uses_local_route_table(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let (netlink, join_handle) = start_test_netlink(&realm).await;

    const ACCEPT_RA_RT_TABLE: i32 = -1000;
    // `write_accept_ra_rt_table` is sync blocking, so we have to do it in
    // another thread. The below code emulates this function being called from
    // a starnix kernel thread.
    let netlink = fasync::unblock(move || {
        netlink
            .write_accept_ra_rt_table(netlink::SysctlInterfaceSelector::Default, ACCEPT_RA_RT_TABLE)
            .expect("failed to update accept_ra_rt_table");
        netlink
    })
    .await;

    let mut client = add_route_client(&netlink);

    assert!(
        client
            .client
            .add_membership(ModernGroup(rtnetlink_groups_RTNLGRP_IPV4_ROUTE))
            .expect("should join V4 group")
            .is_noop()
    );

    assert!(
        client
            .client
            .add_membership(ModernGroup(rtnetlink_groups_RTNLGRP_IPV6_ROUTE))
            .expect("should join V6 group")
            .is_noop()
    );

    let network = sandbox.create_network("network").await.expect("create network");
    let ep = realm
        .join_network_with_if_config(
            &network,
            "ep",
            netemul::InterfaceConfig {
                netstack_managed_routes_designation: Some(
                    fnet_interfaces_admin::NetstackManagedRoutesDesignation::InterfaceLocal(
                        fnet_interfaces_admin::Empty,
                    ),
                ),
                ..Default::default()
            },
        )
        .await
        .expect("failed to create the interface");
    let interface_id = ep.id();

    let expected_table_id =
        u32::try_from(ep.id()).unwrap() + u32::try_from(-ACCEPT_RA_RT_TABLE).unwrap();

    let expected_route = move |af, dest: std::net::IpAddr, prefix_len: u8| {
        let mut route_message = RouteMessage::default();
        route_message.header = RouteHeader {
            address_family: af,
            destination_prefix_length: prefix_len,
            source_prefix_length: 0,
            tos: 0,
            table: rt_class_t_RT_TABLE_COMPAT as u8,
            protocol: RouteProtocol::Kernel,
            scope: RouteScope::Universe,
            kind: RouteType::Unicast,
            flags: RouteFlags::empty(),
        };

        route_message.attributes.extend([
            RouteAttribute::Destination(dest.into()),
            RouteAttribute::Oif(u32::try_from(interface_id).unwrap()),
            RouteAttribute::Priority(100),
            RouteAttribute::Table(expected_table_id),
        ]);

        RouteNetlinkMessage::NewRoute(route_message)
    };
    // The initial device routes that we expect to be in the local table:
    let mut expected_routes = Vec::from_iter([
        expected_route(AddressFamily::Inet, std_ip!("224.0.0.0"), 4),
        expected_route(AddressFamily::Inet6, std_ip!("fe80::"), 64),
        expected_route(AddressFamily::Inet6, std_ip!("ff00::"), 8),
    ]);

    while !expected_routes.is_empty() {
        let next_msg = client
            .receiver
            .next()
            .on_timeout(ASYNC_EVENT_CHECK_INTERVAL, || {
                panic!(
                    "timed out waiting for route messages, still unseen routes: {:?}",
                    expected_routes
                );
            })
            .await
            .expect("should not be disconnected");
        let route_msg = match next_msg.message.payload {
            NetlinkPayload::InnerMessage(message) => message,
            _ => continue,
        };
        expected_routes.retain(|r| r != &route_msg);
    }

    // Hold the receiver open until the worker is finished.
    assert_eq!(join_handle.abort().await, None);
}

struct WaitForSpecificInterfaceHandler {
    expected_name: String,
    sender: Option<oneshot::Sender<()>>,
}

impl netlink::interfaces::InterfacesHandler for WaitForSpecificInterfaceHandler {
    fn handle_new_link(&mut self, name: &str, _: NonZeroU64) {
        if name == &self.expected_name {
            self.sender
                .take()
                .expect("should only see the interface online once")
                .send(())
                .unwrap();
        }
    }

    fn handle_deleted_link(&mut self, _: &str) {}
}

#[netstack_test]
#[variant(I, Ip)]
async fn netlink_add_routes_in_local_table<I: FidlRouteIpExt + FidlRouteAdminIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let name = "ep";
    let (interface_in_netlink_sender, interface_in_netlink_receiver) = oneshot::channel();
    let (netlink, join_handle) = start_test_netlink_with_interfaces_handler(
        &realm,
        WaitForSpecificInterfaceHandler {
            expected_name: name.to_string(),
            sender: Some(interface_in_netlink_sender),
        },
    )
    .await;

    let network = sandbox.create_network("network").await.expect("create network");
    let ep = realm
        .join_network_with_if_config(
            &network,
            name,
            netemul::InterfaceConfig {
                netstack_managed_routes_designation: Some(
                    fnet_interfaces_admin::NetstackManagedRoutesDesignation::InterfaceLocal(
                        fnet_interfaces_admin::Empty,
                    ),
                ),
                name: Some(Cow::Borrowed(name)),
                ..Default::default()
            },
        )
        .await
        .expect("failed to create the interface");
    const ACCEPT_RA_RT_TABLE: i32 = -1000;
    let interface_id = ep.id();

    interface_in_netlink_receiver.await.expect("should see the interface online");

    // `write_accept_ra_rt_table` is sync blocking, so we have to do it in
    // another thread. The below code emulates this function being called from
    // a starnix kernel thread.
    let netlink = fasync::unblock(move || {
        netlink
            .write_accept_ra_rt_table(
                netlink::SysctlInterfaceSelector::Id(NonZeroU64::new(interface_id).unwrap()),
                ACCEPT_RA_RT_TABLE,
            )
            .expect("failed to update accept_ra_rt_table");
        netlink
    })
    .await;
    let expected_table_id =
        u32::try_from(ep.id()).unwrap() + u32::try_from(-ACCEPT_RA_RT_TABLE).unwrap();

    let mut client = add_route_client(&netlink);
    assert!(
        client
            .client
            .add_membership(route_group::<I>())
            .expect("should add membership successfully")
            .is_noop(),
        "should not produce blocking work"
    );
    const SUBNET_TO_ADD: TestSubnet = TestSubnet::B;
    const ROUTE_PRIORITY: u32 = 1;
    add_route_in_table_and_await_installed::<I>(
        &mut client,
        SUBNET_TO_ADD,
        ep.id().try_into().unwrap(),
        expected_table_id.try_into().unwrap(),
        ROUTE_PRIORITY,
    )
    .await;

    let grant = ep.control().get_authorization_for_interface().await.expect("failed to get grant");
    let proof = fnet_interfaces_ext::admin::proof_from_grant(&grant);
    let provider =
        realm.connect_to_protocol::<I::RouteTableProviderMarker>().expect("connect to protocol");
    let local_table = fnet_routes_ext::admin::get_interface_local_table::<I>(&provider, proof)
        .await
        .expect("fidl")
        .expect("failed to get interface local table");
    let table_id = fnet_routes_ext::admin::get_table_id::<I>(&local_table).await.expect("fidl");
    let state = realm.connect_to_protocol::<I::StateMarker>().expect("connect to protocol");
    let mut stream = std::pin::pin!(
        fnet_routes_ext::event_stream_from_state_with_options(
            &state,
            fnet_routes_ext::WatcherOptions {
                table_interest: Some(fnet_routes::TableInterest::Only(table_id.get())),
            },
        )
        .expect("convert to stream")
    );
    let routes = fnet_routes_ext::collect_routes_until_idle::<I, HashSet<_>>(stream.by_ref())
        .await
        .expect("collect routes until idle");
    let network = I::map_ip_out(
        SUBNET_TO_ADD.subnet::<I>(),
        |subnet| assert_matches!(subnet, fnet::IpAddress::Ipv4(fidl) => Ipv4Addr::from_ext(fidl)),
        |subnet| assert_matches!(subnet, fnet::IpAddress::Ipv6(fidl) => Ipv6Addr::from_ext(fidl)),
    );
    let expected_route = fnet_routes_ext::InstalledRoute {
        route: fnet_routes_ext::Route {
            destination: net_types::ip::Subnet::new(network, TEST_SUBNET_LENGTH).unwrap(),
            action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                outbound_interface: ep.id(),
                next_hop: None,
            }),
            properties: fnet_routes_ext::RouteProperties {
                specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                    metric: fnet_routes::SpecifiedMetric::ExplicitMetric(ROUTE_PRIORITY),
                },
            },
        },
        effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: ROUTE_PRIORITY },
        table_id,
    };
    assert!(routes.contains(&expected_route));
    assert_eq!(join_handle.abort().await, None);
}

#[netstack_test]
#[variant(I, Ip)]
async fn add_remove_routes<I: Ip + FidlRouteIpExt>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let (netlink, _join_handle) = start_test_netlink(&realm).await;

    let network = sandbox.create_network("network").await.expect("create network");

    const SUBNET: TestSubnet = TestSubnet::A;

    let ep = realm
        .join_network_with_if_config(&network, "ep", Default::default())
        .await
        .expect("failed to create the interface");

    let mut client = add_route_client(&netlink);
    assert!(
        client
            .client
            .add_membership(route_group::<I>())
            .expect("should add membership successfully")
            .is_noop(),
        "should not produce blocking work"
    );

    let table_id = 1000;
    for priority in [DEFAULT_ROUTE_PRIORITY, DEFAULT_ROUTE_PRIORITY + 1] {
        add_route_in_table_and_await_installed::<I>(
            &mut client,
            SUBNET,
            ep.id().try_into().unwrap(),
            table_id,
            priority,
        )
        .await;
        remove_route_in_table_and_await_uninstalled::<I>(
            &mut client,
            SUBNET,
            ep.id().try_into().unwrap(),
            table_id,
            priority,
        )
        .await;
    }
}

struct NetlinkSockDiagClient {
    _client: netlink::protocol_family::sock_diag::NetlinkSockDiagClient,
    sender: Sender<NetlinkMessageWithCreds<NetlinkMessage<SockDiagRequest>, FakeCreds>>,
    receiver: Receiver<SentNetlinkMessage<SockDiagResponse>>,
}

fn add_sock_diag_client(netlink: &netlink::Netlink<NetlinkContext>) -> NetlinkSockDiagClient {
    let (server_sender, client_receiver) = Sender::new_pair();
    let (client_sender, server_receiver) = Sender::new_pair();
    let client = netlink
        .new_sock_diag_client(server_sender, server_receiver)
        .expect("should create new client successfully");
    NetlinkSockDiagClient { _client: client, sender: client_sender, receiver: client_receiver }
}

async fn bind_socket<I: Ip>(
    realm: &TestRealm<'_>,
    proto: i32,
    addr: I::Addr,
) -> (std::net::IpAddr, u16, Box<dyn std::any::Any>) {
    let addr = std::net::SocketAddr::new(addr.to_ip_addr().into(), 0);
    match proto {
        libc::IPPROTO_UDP => {
            let s =
                fasync::net::UdpSocket::bind_in_realm(realm, addr).await.expect("bind UDP socket");
            let local = s.local_addr().expect("get local addr");
            (local.ip(), local.port(), Box::new(s))
        }
        libc::IPPROTO_TCP => {
            let s = fasync::net::TcpListener::listen_in_realm(realm, addr)
                .await
                .expect("listen TCP socket");
            let local = s.local_addr().expect("get local addr");
            (local.ip(), local.port(), Box::new(s))
        }
        p => panic!("unsupported protocol {}", p),
    }
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(libc::IPPROTO_UDP; "udp")]
#[test_case(libc::IPPROTO_TCP; "tcp")]
async fn sock_diag_dump<I: Ip>(name: &str, proto: i32) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let (netlink, _join_handle) = start_test_netlink(&realm).await;
    let mut client = add_sock_diag_client(&netlink);

    let (_ip1, port1, _socket1) = bind_socket::<I>(&realm, proto, I::LOOPBACK_ADDRESS.get()).await;
    let (_ip2, port2, _socket2) = bind_socket::<I>(&realm, proto, I::LOOPBACK_ADDRESS.get()).await;

    let req = InetRequest {
        family: match I::VERSION {
            IpVersion::V4 => libc::AF_INET as u8,
            IpVersion::V6 => libc::AF_INET6 as u8,
        },
        protocol: proto as u8,
        extensions: ExtensionFlags::empty(),
        states: StateFlags::all(),
        socket_id: SocketId {
            source_port: 0,
            destination_port: 0,
            source_address: I::UNSPECIFIED_ADDRESS.to_ip_addr().into(),
            destination_address: I::UNSPECIFIED_ADDRESS.to_ip_addr().into(),
            interface_id: 0,
            cookie: [0xFF; 8],
        },
        nlas: smallvec![],
    };

    let mut message: NetlinkMessage<SockDiagRequest> = SockDiagRequest::InetRequest(req).into();
    message.header.flags |= netlink_packet_core::NLM_F_DUMP;
    message.finalize();
    client.sender.0.unbounded_send(FakeCreds::attach(message)).expect("send request");

    let mut found_ports = Vec::new();
    while let Some(response) = client.receiver.next().await {
        match response.message.payload {
            NetlinkPayload::InnerMessage(SockDiagResponse::InetResponse(msg)) => {
                let _ = found_ports.push(msg.header.socket_id.source_port);
            }
            NetlinkPayload::Done(_) => break,
            NetlinkPayload::Error(e) => panic!("received error: {:?}", e),
            p => panic!("unexpected payload: {:?}", p),
        }
    }

    found_ports.sort();
    let mut expected_ports = [port1, port2];
    expected_ports.sort();

    assert_eq!(found_ports, expected_ports);
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(libc::IPPROTO_UDP; "udp")]
#[test_case(libc::IPPROTO_TCP; "tcp")]
async fn sock_diag_get_one<I: Ip>(name: &str, proto: i32) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let (netlink, _join_handle) = start_test_netlink(&realm).await;
    let mut client = add_sock_diag_client(&netlink);

    let (ip1, port1, _socket1) = bind_socket::<I>(&realm, proto, I::UNSPECIFIED_ADDRESS).await;
    let (_ip2, _port2, _socket2) = bind_socket::<I>(&realm, proto, I::UNSPECIFIED_ADDRESS).await;

    // Linux has a bug where the UDP tuple is reversed for single-socket requests.
    let (source_port, destination_port, source_address, destination_address) =
        if proto == libc::IPPROTO_UDP {
            (0, port1, I::UNSPECIFIED_ADDRESS.to_ip_addr().into(), ip1)
        } else {
            (port1, 0, ip1, I::UNSPECIFIED_ADDRESS.to_ip_addr().into())
        };

    let req = InetRequest {
        family: match I::VERSION {
            IpVersion::V4 => libc::AF_INET as u8,
            IpVersion::V6 => libc::AF_INET6 as u8,
        },
        protocol: proto as u8,
        extensions: ExtensionFlags::empty(),
        states: StateFlags::all(),
        socket_id: SocketId {
            source_port,
            destination_port,
            source_address,
            destination_address,
            interface_id: 0,
            cookie: [0xFF; 8],
        },
        nlas: smallvec![],
    };

    let mut message: NetlinkMessage<SockDiagRequest> = SockDiagRequest::InetRequest(req).into();
    message.finalize();
    client.sender.0.unbounded_send(FakeCreds::attach(message)).expect("send request");

    let expected_state = match proto.into() {
        libc::IPPROTO_UDP => netlink_packet_sock_diag::TCP_CLOSE,
        libc::IPPROTO_TCP => netlink_packet_sock_diag::TCP_LISTEN,
        _ => panic!("unsupported protocol"),
    };

    let response = client.receiver.next().await.expect("should receive one response");

    match response.message.payload {
        NetlinkPayload::InnerMessage(SockDiagResponse::InetResponse(msg)) => {
            assert_eq!(
                net_types::ip::IpAddr::from(msg.header.socket_id.source_address),
                net_types::ip::IpAddr::from(ip1),
            );
            assert_eq!(msg.header.socket_id.source_port, port1);
            assert_eq!(msg.header.socket_id.destination_port, 0);
            assert_eq!(
                net_types::ip::IpAddr::from(msg.header.socket_id.destination_address),
                I::UNSPECIFIED_ADDRESS.to_ip_addr()
            );
            assert_eq!(msg.header.socket_id.interface_id, u32::MAX);
            assert_ne!(msg.header.socket_id.cookie, [0xFF; 8]);

            assert_eq!(
                msg.header.family,
                match I::VERSION {
                    IpVersion::V4 => libc::AF_INET as u8,
                    IpVersion::V6 => libc::AF_INET6 as u8,
                }
            );

            assert_eq!(msg.header.state, expected_state);
            assert_eq!(msg.header.timer, None);
            assert_eq!(msg.header.recv_queue, u32::MAX);
            assert_eq!(msg.header.send_queue, u32::MAX);
            assert_eq!(msg.header.uid, u32::MAX);
            assert_eq!(msg.header.inode, u32::MAX);
        }
        p => panic!("unexpected payload: {:?}", p),
    }

    match client.receiver.try_next() {
        Ok(Some(payload)) => panic!("received unexpected extra message: {:?}", payload),
        _ => {}
    }
}

#[netstack_test]
#[variant(I, Ip)]
async fn sock_destroy_tcp<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let (netlink, _join_handle) = start_test_netlink(&realm).await;
    let mut client = add_sock_diag_client(&netlink);

    let listener = fasync::net::TcpListener::listen_in_realm(
        &realm,
        std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.get().to_ip_addr().into(), 0),
    )
    .await
    .expect("listen_in_realm");
    let listener_addr = listener.local_addr().expect("get local addr");

    let mut client_socket = fasync::net::TcpStream::connect_in_realm(&realm, listener_addr)
        .await
        .expect("connect_in_realm");

    let (_listener, _server_socket, _peer) = listener.accept().await.expect("accept");

    let local_addr = client_socket.std().local_addr().expect("get local addr");
    let peer_addr = client_socket.std().peer_addr().expect("get peer addr");

    let req = InetRequest {
        family: match I::VERSION {
            IpVersion::V4 => linux_uapi::AF_INET as u8,
            IpVersion::V6 => linux_uapi::AF_INET6 as u8,
        },
        protocol: linux_uapi::IPPROTO_TCP as u8,
        extensions: ExtensionFlags::empty(),
        states: StateFlags::all(),
        socket_id: SocketId {
            source_port: local_addr.port(),
            destination_port: peer_addr.port(),
            source_address: local_addr.ip(),
            destination_address: peer_addr.ip(),
            interface_id: 0,
            // The match-all cookie.
            cookie: [0xFF; 8],
        },
        nlas: smallvec![],
    };

    let msg = SockDiagRequest::InetSockDestroy(req);
    let mut netlink_msg: NetlinkMessage<SockDiagRequest> = msg.into();
    netlink_msg.header.flags = NLM_F_ACK | NLM_F_REQUEST;
    netlink_msg.finalize();

    client.sender.0.unbounded_send(FakeCreds::attach(netlink_msg)).expect("send request");

    let msg = client.receiver.next().await.expect("receive ack").message;
    assert_matches::assert_matches!(
        msg.payload,
        NetlinkPayload::Error(ErrorMessage { code: None, .. })
    );

    let mut buf = [0u8; 1];
    assert_matches!(
        client_socket.read(&mut buf).await,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionAborted
    );
}

#[netstack_test]
#[variant(I, Ip)]
async fn sock_destroy_udp<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let (netlink, _join_handle) = start_test_netlink(&realm).await;
    let mut client = add_sock_diag_client(&netlink);

    let addr = std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.get().to_ip_addr().into(), 0);

    let socket1 = fasync::net::UdpSocket::bind_in_realm(&realm, addr).await.expect("bind socket1");
    let local_addr1 = socket1.local_addr().expect("get local addr 1");

    let socket2 = fasync::net::UdpSocket::bind_in_realm(&realm, addr).await.expect("bind socket2");
    let local_addr2 = socket2.local_addr().expect("get local addr 2");

    socket1.connect(&local_addr2).expect("connect socket1");
    socket2.connect(&local_addr1).expect("connect socket2");

    let req = InetRequest {
        family: match I::VERSION {
            IpVersion::V4 => linux_uapi::AF_INET as u8,
            IpVersion::V6 => linux_uapi::AF_INET6 as u8,
        },
        protocol: linux_uapi::IPPROTO_UDP as u8,
        extensions: ExtensionFlags::empty(),
        states: StateFlags::all(),
        socket_id: SocketId {
            source_port: local_addr1.port(),
            destination_port: local_addr2.port(),
            source_address: local_addr1.ip(),
            destination_address: local_addr2.ip(),
            interface_id: 0,
            // The match-all cookie.
            cookie: [0xFF; 8],
        },
        nlas: smallvec![],
    };

    let msg = SockDiagRequest::InetSockDestroy(req);
    let mut netlink_msg: NetlinkMessage<SockDiagRequest> = msg.into();
    netlink_msg.header.flags = NLM_F_ACK | NLM_F_REQUEST;
    netlink_msg.finalize();

    client.sender.0.unbounded_send(FakeCreds::attach(netlink_msg)).expect("send request");

    let msg = client.receiver.next().await.expect("receive ack").message;
    assert_matches!(msg.payload, NetlinkPayload::Error(ErrorMessage { code: None, .. }));

    let mut buf = [0u8; 1];
    assert_matches!(
        socket1.recv_from(&mut buf).await,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionAborted
    );

    assert_matches!(socket2.recv_from(&mut buf).now_or_never(), None);
}

#[netstack_test]
#[variant(I, Ip)]
async fn sock_destroy_id_mismatch<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let (netlink, _join_handle) = start_test_netlink(&realm).await;
    let mut client = add_sock_diag_client(&netlink);

    let listener = fasync::net::TcpListener::listen_in_realm(
        &realm,
        std::net::SocketAddr::new(I::LOOPBACK_ADDRESS.get().to_ip_addr().into(), 0),
    )
    .await
    .expect("listen_in_realm");
    let listener_addr = listener.local_addr().expect("get local addr");

    let mut client_socket = fasync::net::TcpStream::connect_in_realm(&realm, listener_addr)
        .await
        .expect("connect_in_realm");

    let (_listener, _server_socket, _peer) = listener.accept().await.expect("accept");

    let local_addr = client_socket.std().local_addr().expect("get local addr");
    let peer_addr = client_socket.std().peer_addr().expect("get peer addr");

    let req = InetRequest {
        family: match I::VERSION {
            IpVersion::V4 => linux_uapi::AF_INET as u8,
            IpVersion::V6 => linux_uapi::AF_INET6 as u8,
        },
        protocol: linux_uapi::IPPROTO_TCP as u8,
        extensions: ExtensionFlags::empty(),
        states: StateFlags::all(),
        socket_id: SocketId {
            // Wrong port, wrapping to a nonzero value.
            source_port: local_addr.port().checked_add(1).unwrap_or(1),
            destination_port: peer_addr.port(),
            source_address: local_addr.ip(),
            destination_address: peer_addr.ip(),
            interface_id: 0,
            cookie: [0xFF; 8],
        },
        nlas: smallvec![],
    };

    let msg = SockDiagRequest::InetSockDestroy(req);
    let mut netlink_msg: NetlinkMessage<SockDiagRequest> = msg.into();
    netlink_msg.header.flags = NLM_F_ACK | NLM_F_REQUEST;
    netlink_msg.finalize();

    client.sender.0.unbounded_send(FakeCreds::attach(netlink_msg)).expect("send request");

    let msg = client.receiver.next().await.expect("receive ack").message;
    let err = assert_matches::assert_matches!(
        msg.payload,
        NetlinkPayload::Error(err) => err
    );
    let expected = std::num::NonZeroI32::new(-(libc::ENOENT as i32)).unwrap();
    assert_eq!(err.code, Some(expected));

    let res = client_socket.write_all(&[0u8]).await;
    assert!(res.is_ok(), "Socket should still work");
}
