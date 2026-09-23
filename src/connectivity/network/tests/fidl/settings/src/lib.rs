// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_settings as fnet_settings;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_packet as fposix_socket_packet;
use fidl_fuchsia_posix_socket_raw as fposix_socket_raw;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;

#[netstack_test]
async fn startup_interface_defaults(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let state = realm.connect_to_protocol::<fnet_settings::StateMarker>().expect("connect state");
    let defaults = state.get_interface_defaults().await.expect("get defaults");
    let interface = realm.join_network(&net, "ep").await.expect("install interface");
    let config = interface
        .control()
        .get_configuration()
        .await
        .expect("get configuration")
        .expect("get config error");
    assert_eq!(config, defaults);
}

#[netstack_test]
async fn interface_defaults(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let net = sandbox.create_network("net").await.expect("failed to create network");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let state = realm.connect_to_protocol::<fnet_settings::StateMarker>().expect("connect state");
    let defaults = state.get_interface_defaults().await.expect("get defaults");

    let modify_nud = |nud: fnet_interfaces_admin::NudConfiguration| {
        let fnet_interfaces_admin::NudConfiguration {
            max_multicast_solicitations,
            max_unicast_solicitations,
            base_reachable_time,
            retrans_timer,
            __source_breaking,
        } = nud;
        fnet_interfaces_admin::NudConfiguration {
            max_multicast_solicitations: Some(
                max_multicast_solicitations.expect("missing max mcast solicit") + 1,
            ),
            max_unicast_solicitations: Some(
                max_unicast_solicitations.expect("missing max unicast solicit") + 1,
            ),
            base_reachable_time: Some(
                base_reachable_time.expect("missing base reachable time") + 1000,
            ),
            retrans_timer: Some(retrans_timer.expect("missing retrans timer") + 1000),
            __source_breaking,
        }
    };

    let modify_dad = |dad: fnet_interfaces_admin::DadConfiguration| {
        let fnet_interfaces_admin::DadConfiguration { transmits, __source_breaking } = dad;
        fnet_interfaces_admin::DadConfiguration {
            transmits: Some(transmits.expect("missing transmits") + 1),
            __source_breaking,
        }
    };

    let modify_router_solicitations =
        |rs: fnet_interfaces_admin::RouterSolicitationConfiguration| {
            let fnet_interfaces_admin::RouterSolicitationConfiguration { max, __source_breaking } =
                rs;
            fnet_interfaces_admin::RouterSolicitationConfiguration {
                max: Some(max.expect("missing max router solicitations") + 1),
                __source_breaking,
            }
        };

    // Modify defaults.
    let fnet_interfaces_admin::Configuration { ipv4, ipv6, __source_breaking } = defaults.clone();
    let fnet_interfaces_admin::Ipv4Configuration {
        unicast_forwarding,
        multicast_forwarding,
        igmp,
        arp,
        enabled: ipv4_enabled,
        __source_breaking,
    } = ipv4.expect("missing ipv4");

    let fnet_interfaces_admin::IgmpConfiguration { version, __source_breaking } =
        igmp.expect("missing igmp");
    let igmp = fnet_interfaces_admin::IgmpConfiguration {
        version: Some(match version.expect("missing IGMP version") {
            fnet_interfaces_admin::IgmpVersion::V3 => fnet_interfaces_admin::IgmpVersion::V1,
            _ => fnet_interfaces_admin::IgmpVersion::V3,
        }),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    let fnet_interfaces_admin::ArpConfiguration { nud, dad, __source_breaking } =
        arp.expect("missing arp");
    let arp = fnet_interfaces_admin::ArpConfiguration {
        nud: Some(modify_nud(nud.expect("missing nud"))),
        dad: Some(modify_dad(dad.expect("missing dad"))),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    let ipv4 = fnet_interfaces_admin::Ipv4Configuration {
        unicast_forwarding: Some(!unicast_forwarding.expect("missing unicast fwd")),
        multicast_forwarding: Some(!multicast_forwarding.expect("missing mcast fwd")),
        igmp: Some(igmp),
        arp: Some(arp),
        enabled: Some(ipv4_enabled.expect("missing ipv4 enabled")),
        __source_breaking: fidl::marker::SourceBreaking,
    };

    let fnet_interfaces_admin::Ipv6Configuration {
        unicast_forwarding,
        multicast_forwarding,
        mld,
        ndp,
        enabled: ipv6_enabled,
        __source_breaking,
    } = ipv6.expect("missing ipv6");
    let fnet_interfaces_admin::MldConfiguration { version, __source_breaking } =
        mld.expect("missing mld");
    let mld = fnet_interfaces_admin::MldConfiguration {
        version: Some(match version.expect("missing mld") {
            fnet_interfaces_admin::MldVersion::V2 => fnet_interfaces_admin::MldVersion::V1,
            _ => fnet_interfaces_admin::MldVersion::V2,
        }),
        __source_breaking,
    };
    let fnet_interfaces_admin::NdpConfiguration {
        nud,
        dad,
        slaac,
        route_discovery,
        router_solicitations,
        __source_breaking,
    } = ndp.expect("missing ndp");
    let fnet_interfaces_admin::SlaacConfiguration { temporary_address, __source_breaking } =
        slaac.expect("missing slaac");
    let slaac = fnet_interfaces_admin::SlaacConfiguration {
        temporary_address: Some(!temporary_address.expect("missing temp addresses")),
        __source_breaking,
    };
    let fnet_interfaces_admin::RouteDiscoveryConfiguration {
        allow_default_route,
        __source_breaking,
    } = route_discovery.expect("missing route discovery");
    let route_discovery = fnet_interfaces_admin::RouteDiscoveryConfiguration {
        allow_default_route: Some(!allow_default_route.expect("missing allow default route")),
        __source_breaking,
    };
    let ndp = fnet_interfaces_admin::NdpConfiguration {
        nud: Some(modify_nud(nud.expect("missing nud"))),
        dad: Some(modify_dad(dad.expect("missing dad"))),
        slaac: Some(slaac),
        route_discovery: Some(route_discovery),
        router_solicitations: Some(modify_router_solicitations(
            router_solicitations.expect("missing router solicitations"),
        )),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    let ipv6 = fnet_interfaces_admin::Ipv6Configuration {
        unicast_forwarding: Some(!unicast_forwarding.expect("missing unicast fwd")),
        multicast_forwarding: Some(!multicast_forwarding.expect("missing mcast fwd")),
        mld: Some(mld),
        ndp: Some(ndp),
        enabled: Some(!ipv6_enabled.expect("missing ipv6 enabled")),
        __source_breaking: fidl::marker::SourceBreaking,
    };

    let new_defaults = fnet_interfaces_admin::Configuration {
        ipv4: Some(ipv4),
        ipv6: Some(ipv6),
        __source_breaking: fidl::marker::SourceBreaking,
    };

    let control =
        realm.connect_to_protocol::<fnet_settings::ControlMarker>().expect("connect control");
    let prev = control
        .update_interface_defaults(&new_defaults)
        .await
        .expect("calling update interface defaults")
        .expect("update defaults");
    assert_eq!(prev, defaults);

    let interface = realm.join_network(&net, "ep").await.expect("install interface");
    let config = interface
        .control()
        .get_configuration()
        .await
        .expect("get configuration")
        .expect("get config error");
    assert_eq!(config, new_defaults);

    let defaults = state.get_interface_defaults().await.expect("get defaults");
    assert_eq!(defaults, new_defaults);
}

#[netstack_test]
async fn partial_update_interface(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let state = realm.connect_to_protocol::<fnet_settings::StateMarker>().expect("connect state");
    let defaults = state.get_interface_defaults().await.expect("get defaults");

    let fnet_interfaces_admin::Ipv4Configuration {
        unicast_forwarding, multicast_forwarding, ..
    } = defaults.ipv4.as_ref().unwrap();
    let default_unicast_fwd = unicast_forwarding.unwrap();
    let default_multicast_fwd = multicast_forwarding.unwrap();

    let control =
        realm.connect_to_protocol::<fnet_settings::ControlMarker>().expect("connect control");
    let update = fnet_interfaces_admin::Configuration {
        ipv4: Some(fnet_interfaces_admin::Ipv4Configuration {
            // Apply the same value to unicast, and a different value to
            // multicast to prove we get the previous value returns either way.
            unicast_forwarding: Some(default_unicast_fwd),
            multicast_forwarding: Some(!default_multicast_fwd),
            ..Default::default()
        }),
        ..Default::default()
    };
    let prev = control
        .update_interface_defaults(&update)
        .await
        .expect("calling update interface defaults")
        .expect("update defaults");

    // The touched values return their previous configuration.
    let expect = fnet_interfaces_admin::Configuration {
        ipv4: Some(fnet_interfaces_admin::Ipv4Configuration {
            unicast_forwarding: Some(default_unicast_fwd),
            multicast_forwarding: Some(default_multicast_fwd),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(prev, expect);
}

#[netstack_test]
async fn tcp_buffer_sizes(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let state = realm.connect_to_protocol::<fnet_settings::StateMarker>().expect("connect state");

    let defaults = state.get_tcp().await.expect("get defaults");
    let default_buffer_sizes = defaults.buffer_sizes.expect("missing buffer sizes");
    let buffer_sizes = SocketBufferSizes::from(default_buffer_sizes.clone());
    let socket = realm
        .stream_socket(fposix_socket::Domain::Ipv4, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("open socket");
    check_socket_buffer_sizes(&socket, buffer_sizes.clone());

    let buffer_sizes = buffer_sizes.double();
    let control =
        realm.connect_to_protocol::<fnet_settings::ControlMarker>().expect("connect control");
    let updated = control
        .update_tcp(&fnet_settings::Tcp {
            buffer_sizes: Some(buffer_sizes.clone().into()),
            ..Default::default()
        })
        .await
        .expect("calling update tcp")
        .expect("update tcp");

    assert_eq!(
        updated,
        fnet_settings::Tcp { buffer_sizes: Some(default_buffer_sizes), ..Default::default() }
    );
    let new_defaults = state.get_tcp().await.expect("get defaults");
    assert_eq!(
        new_defaults,
        fnet_settings::Tcp { buffer_sizes: Some(buffer_sizes.clone().into()), ..defaults }
    );

    let socket = realm
        .stream_socket(fposix_socket::Domain::Ipv4, fposix_socket::StreamSocketProtocol::Tcp)
        .await
        .expect("open socket");
    check_socket_buffer_sizes(&socket, buffer_sizes);
}

#[netstack_test]
async fn udp_buffer_sizes(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let state = realm.connect_to_protocol::<fnet_settings::StateMarker>().expect("connect state");

    let defaults = state.get_udp().await.expect("get defaults");
    let default_buffer_sizes = defaults.buffer_sizes.expect("missing buffer sizes");
    let buffer_sizes = SocketBufferSizes::from(default_buffer_sizes.clone());
    let socket = realm
        .datagram_socket(fposix_socket::Domain::Ipv4, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("open socket");
    check_socket_buffer_sizes(&socket, buffer_sizes.clone());

    let buffer_sizes = buffer_sizes.double();
    let control =
        realm.connect_to_protocol::<fnet_settings::ControlMarker>().expect("connect control");
    let updated = control
        .update_udp(&fnet_settings::Udp {
            buffer_sizes: Some(buffer_sizes.clone().into()),
            ..Default::default()
        })
        .await
        .expect("calling update udp")
        .expect("update udp");

    assert_eq!(
        updated,
        fnet_settings::Udp { buffer_sizes: Some(default_buffer_sizes), ..Default::default() }
    );
    let new_defaults = state.get_udp().await.expect("get defaults");
    assert_eq!(
        new_defaults,
        fnet_settings::Udp { buffer_sizes: Some(buffer_sizes.clone().into()), ..defaults }
    );

    let socket = realm
        .datagram_socket(fposix_socket::Domain::Ipv4, fposix_socket::DatagramSocketProtocol::Udp)
        .await
        .expect("open socket");
    check_socket_buffer_sizes(&socket, buffer_sizes);
}

#[netstack_test]
async fn icmp_buffer_sizes(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let state = realm.connect_to_protocol::<fnet_settings::StateMarker>().expect("connect state");

    let defaults = state.get_icmp().await.expect("get defaults");
    let default_buffer_sizes = defaults.echo_buffer_sizes.expect("missing buffer sizes");

    let buffer_sizes = SocketBufferSizes::from(default_buffer_sizes.clone());
    let socket = realm
        .datagram_socket(
            fposix_socket::Domain::Ipv4,
            fposix_socket::DatagramSocketProtocol::IcmpEcho,
        )
        .await
        .expect("open socket");
    check_socket_buffer_sizes(&socket, buffer_sizes.clone());

    let buffer_sizes = buffer_sizes.double();
    let control =
        realm.connect_to_protocol::<fnet_settings::ControlMarker>().expect("connect control");
    let updated = control
        .update_icmp(&fnet_settings::Icmp {
            echo_buffer_sizes: Some(buffer_sizes.clone().into()),
            ..Default::default()
        })
        .await
        .expect("calling update icmp")
        .expect("update icmp");

    assert_eq!(
        updated,
        fnet_settings::Icmp { echo_buffer_sizes: Some(default_buffer_sizes), ..Default::default() }
    );
    let new_defaults = state.get_icmp().await.expect("get defaults");
    assert_eq!(
        new_defaults,
        fnet_settings::Icmp { echo_buffer_sizes: Some(buffer_sizes.clone().into()), ..defaults }
    );

    let socket = realm
        .datagram_socket(
            fposix_socket::Domain::Ipv4,
            fposix_socket::DatagramSocketProtocol::IcmpEcho,
        )
        .await
        .expect("open socket");
    check_socket_buffer_sizes(&socket, buffer_sizes);
}

#[netstack_test]
async fn raw_ip_buffer_sizes(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let state = realm.connect_to_protocol::<fnet_settings::StateMarker>().expect("connect state");

    let defaults = state.get_ip().await.expect("get defaults");
    let default_buffer_sizes = defaults.raw_buffer_sizes.expect("missing buffer sizes");

    let fnet_settings::SocketBufferSizes { send, receive, __source_breaking } =
        default_buffer_sizes;
    // TODO(https://fxbug.dev/392111277): Support SNDBUF on raw sockets.
    assert_eq!(send, None);
    let default_receive = receive.expect("missing receive");

    let receive = SocketBufferSizeRange::from(default_receive.clone());
    let socket = realm
        .raw_socket(
            fposix_socket::Domain::Ipv4,
            fposix_socket_raw::ProtocolAssociation::Associated(1),
        )
        .await
        .expect("open socket");
    check_socket_receive_buffer_sizes(&socket, receive.clone());

    let receive = receive.double();
    let control =
        realm.connect_to_protocol::<fnet_settings::ControlMarker>().expect("connect control");
    let updated = control
        .update_ip(&fnet_settings::Ip {
            raw_buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                // TODO(https://fxbug.dev/392111277): Support SNDBUF on raw
                // sockets.
                send: None,
                receive: Some(receive.clone().into()),
                __source_breaking,
            }),
            ..Default::default()
        })
        .await
        .expect("calling update ip")
        .expect("update ip");

    assert_eq!(
        updated,
        fnet_settings::Ip {
            raw_buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                receive: Some(default_receive),
                ..Default::default()
            }),
            ..Default::default()
        }
    );
    let new_defaults = state.get_ip().await.expect("get defaults");
    assert_eq!(
        new_defaults,
        fnet_settings::Ip {
            raw_buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                send: None,
                receive: Some(receive.clone().into()),
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            ..defaults
        }
    );

    let socket = realm
        .raw_socket(
            fposix_socket::Domain::Ipv4,
            fposix_socket_raw::ProtocolAssociation::Associated(1),
        )
        .await
        .expect("open socket");
    check_socket_receive_buffer_sizes(&socket, receive);
}

#[netstack_test]
async fn packet_buffer_sizes(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let state = realm.connect_to_protocol::<fnet_settings::StateMarker>().expect("connect state");

    let defaults = state.get_device().await.expect("get defaults");
    let default_buffer_sizes = defaults.packet_buffer_sizes.expect("missing buffer sizes");

    let fnet_settings::SocketBufferSizes { send, receive, __source_breaking } =
        default_buffer_sizes;
    // TODO(https://fxbug.dev/391946195): Support SNDBUF on packet sockets.
    assert_eq!(send, None);
    let default_receive = receive.expect("missing receive");

    let receive = SocketBufferSizeRange::from(default_receive.clone());
    let socket = realm.packet_socket(fposix_socket_packet::Kind::Link).await.expect("open socket");
    check_socket_receive_buffer_sizes(&socket, receive.clone());

    let receive = receive.double();
    let control =
        realm.connect_to_protocol::<fnet_settings::ControlMarker>().expect("connect control");
    let updated = control
        .update_device(&fnet_settings::Device {
            packet_buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                // TODO(https://fxbug.dev/391946195): Support SNDBUF on packet
                // sockets.
                send: None,
                receive: Some(receive.clone().into()),
                __source_breaking,
            }),
            ..Default::default()
        })
        .await
        .expect("calling update device")
        .expect("update device");

    assert_eq!(
        updated,
        fnet_settings::Device {
            packet_buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                receive: Some(default_receive),
                ..Default::default()
            }),
            ..Default::default()
        }
    );
    let new_defaults = state.get_device().await.expect("get defaults");
    assert_eq!(
        new_defaults,
        fnet_settings::Device {
            packet_buffer_sizes: Some(fnet_settings::SocketBufferSizes {
                send: None,
                receive: Some(receive.clone().into()),
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            ..defaults
        }
    );

    let socket = realm.packet_socket(fposix_socket_packet::Kind::Link).await.expect("open socket");
    check_socket_receive_buffer_sizes(&socket, receive);
}

fn check_socket_buffer_sizes(socket: &socket2::Socket, sizes: SocketBufferSizes) {
    let SocketBufferSizes { send, receive } = sizes;
    check_socket_send_buffer_sizes(socket, send);
    check_socket_receive_buffer_sizes(socket, receive);
}

fn check_socket_send_buffer_sizes(socket: &socket2::Socket, sizes: SocketBufferSizeRange) {
    let SocketBufferSizeRange { min, default, max } = sizes;
    let rcv = socket.send_buffer_size().expect("get sndbuf");
    assert_eq!(rcv, default);

    socket.set_send_buffer_size(min - 1).expect("set min");
    let rcv = socket.send_buffer_size().expect("get sndbuf");
    assert_eq!(rcv, min);

    socket.set_send_buffer_size(max + 1).expect("set max");
    let rcv = socket.send_buffer_size().expect("get sndbuf");
    assert_eq!(rcv, max);
}

fn check_socket_receive_buffer_sizes(socket: &socket2::Socket, sizes: SocketBufferSizeRange) {
    let SocketBufferSizeRange { min, default, max } = sizes;
    let rcv = socket.recv_buffer_size().expect("get rcvbuf");
    assert_eq!(rcv, default);

    socket.set_recv_buffer_size(min - 1).expect("set min");
    let rcv = socket.recv_buffer_size().expect("get rcvbuf");
    assert_eq!(rcv, min);

    socket.set_recv_buffer_size(max + 1).expect("set max");
    let rcv = socket.recv_buffer_size().expect("get rcvbuf");
    assert_eq!(rcv, max);
}

#[derive(Copy, Clone)]
struct SocketBufferSizeRange {
    min: usize,
    default: usize,
    max: usize,
}

impl SocketBufferSizeRange {
    fn double(self) -> Self {
        let Self { min, default, max } = self;
        Self { min: min * 2, default: default * 2, max: max * 2 }
    }
}

impl From<fnet_settings::SocketBufferSizeRange> for SocketBufferSizeRange {
    fn from(value: fnet_settings::SocketBufferSizeRange) -> Self {
        let fnet_settings::SocketBufferSizeRange { max, default, min, __source_breaking } = value;
        Self {
            min: min.expect("missing min").try_into().unwrap(),
            default: default.expect("missing default").try_into().unwrap(),
            max: max.expect("missing max").try_into().unwrap(),
        }
    }
}

impl From<SocketBufferSizeRange> for fnet_settings::SocketBufferSizeRange {
    fn from(value: SocketBufferSizeRange) -> Self {
        let SocketBufferSizeRange { min, default, max } = value;
        fnet_settings::SocketBufferSizeRange {
            max: Some(max.try_into().unwrap()),
            default: Some(default.try_into().unwrap()),
            min: Some(min.try_into().unwrap()),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

#[derive(Copy, Clone)]
struct SocketBufferSizes {
    send: SocketBufferSizeRange,
    receive: SocketBufferSizeRange,
}

impl SocketBufferSizes {
    fn double(self) -> Self {
        let Self { send, receive } = self;
        Self { send: send.double(), receive: receive.double() }
    }
}

impl From<fnet_settings::SocketBufferSizes> for SocketBufferSizes {
    fn from(value: fnet_settings::SocketBufferSizes) -> Self {
        let fnet_settings::SocketBufferSizes { send, receive, __source_breaking } = value;
        Self {
            send: send.expect("missing send").into(),
            receive: receive.expect("missing receive").into(),
        }
    }
}

impl From<SocketBufferSizes> for fnet_settings::SocketBufferSizes {
    fn from(value: SocketBufferSizes) -> Self {
        let SocketBufferSizes { send, receive } = value;
        Self {
            send: Some(send.into()),
            receive: Some(receive.into()),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}
