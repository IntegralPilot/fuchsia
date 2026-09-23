// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extension crate for `fuchsia.posix.socket` and `fuchsia.posix.socket.packet`.
#![deny(missing_docs)]

use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_packet as fpacket;
use zx::StatusExt;

/// Creates a datagram socket using the given provider.
pub async fn datagram_socket(
    provider: &fposix_socket::ProviderProxy,
    domain: fposix_socket::Domain,
    protocol: fposix_socket::DatagramSocketProtocol,
) -> Result<Result<socket2::Socket, std::io::Error>, fidl::Error> {
    let result = provider.datagram_socket(domain, protocol).await?;
    Ok(async move {
        let response =
            result.map_err(|errno| std::io::Error::from_raw_os_error(errno.into_primitive()))?;
        let fd = match response {
            fposix_socket::ProviderDatagramSocketResponse::DatagramSocket(client_end) => {
                fdio::create_fd(client_end.into()).map_err(|status| status.into_io_error())
            }
            fposix_socket::ProviderDatagramSocketResponse::SynchronousDatagramSocket(
                client_end,
            ) => fdio::create_fd(client_end.into()).map_err(|status| status.into_io_error()),
        }?;
        Ok(fd.into())
    }
    .await)
}

/// Creates a datagram socket using the given provider and configured with the
/// specified options.
pub async fn datagram_socket_with_options(
    provider: &fposix_socket::ProviderProxy,
    domain: fposix_socket::Domain,
    protocol: fposix_socket::DatagramSocketProtocol,
    options: fposix_socket::SocketCreationOptions,
) -> Result<Result<socket2::Socket, std::io::Error>, fidl::Error> {
    let result = provider.datagram_socket_with_options(domain, protocol, options).await?;
    Ok(async move {
        let response =
            result.map_err(|errno| std::io::Error::from_raw_os_error(errno.into_primitive()))?;
        let fd = match response {
            fposix_socket::ProviderDatagramSocketWithOptionsResponse::DatagramSocket(
                client_end,
            ) => fdio::create_fd(client_end.into()).map_err(|status| status.into_io_error()),
            fposix_socket::ProviderDatagramSocketWithOptionsResponse::SynchronousDatagramSocket(
                client_end,
            ) => fdio::create_fd(client_end.into()).map_err(|status| status.into_io_error()),
        }?;
        Ok(fd.into())
    }
    .await)
}

/// Creates a packet socket using the given provider.
pub async fn packet_socket(
    provider: &fpacket::ProviderProxy,
    kind: fpacket::Kind,
) -> Result<Result<socket2::Socket, std::io::Error>, fidl::Error> {
    let result = provider.socket(kind).await?;
    Ok(async move {
        let client_end =
            result.map_err(|errno| std::io::Error::from_raw_os_error(errno.into_primitive()))?;
        Ok(fdio::create_fd(client_end.into()).map_err(|status| status.into_io_error())?.into())
    }
    .await)
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl_fuchsia_net_ext as fnet_ext;
    use fidl_fuchsia_netemul_network as fnetemul_network;
    use fidl_fuchsia_posix_socket as fposix_socket;
    use net_declare::std_socket_addr;
    use netstack_testing_common::realms::{Netstack, TestSandboxExt as _};
    use netstack_testing_macros::netstack_test;
    use sockaddr::{IntoSockAddr as _, TryToSockaddrLl as _};

    #[netstack_test]
    #[variant(N, Netstack)]
    async fn datagram_socket_send_receive<N: Netstack>(name: &str) {
        let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();

        let network =
            sandbox.create_network(format!("{name}-test-network")).await.expect("create network");
        let realm_a: netemul::TestRealm<'_> = sandbox
            .create_netstack_realm::<N, _>(format!("{name}-test-realm-a"))
            .expect("create realm");
        let realm_b: netemul::TestRealm<'_> = sandbox
            .create_netstack_realm::<N, _>(format!("{name}-test-realm-b"))
            .expect("create realm");

        const MAC_A: net_types::ethernet::Mac = net_declare::net_mac!("00:00:00:00:00:01");
        const MAC_B: net_types::ethernet::Mac = net_declare::net_mac!("00:00:00:00:00:02");
        const FIDL_SUBNET_A: fidl_fuchsia_net::Subnet = net_declare::fidl_subnet!("192.0.2.1/24");
        const SOCKET_ADDR_A: std::net::SocketAddr = std_socket_addr!("192.0.2.1:1111");
        const FIDL_SUBNET_B: fidl_fuchsia_net::Subnet = net_declare::fidl_subnet!("192.0.2.2/24");
        const SOCKET_ADDR_B: std::net::SocketAddr = std_socket_addr!("192.0.2.2:2222");

        let iface_a = realm_a
            .join_network_with(
                &network,
                "iface_a",
                fnetemul_network::EndpointConfig {
                    mtu: netemul::DEFAULT_MTU,
                    mac: Some(Box::new(fnet_ext::MacAddress { octets: MAC_A.bytes() }.into())),
                    port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                    checksum_offload: false,
                },
                netemul::InterfaceConfig { name: Some("iface_a".into()), ..Default::default() },
            )
            .await
            .expect("join network with realm_a");
        let iface_b = realm_b
            .join_network_with(
                &network,
                "iface_b",
                fnetemul_network::EndpointConfig {
                    mtu: netemul::DEFAULT_MTU,
                    mac: Some(Box::new(fnet_ext::MacAddress { octets: MAC_B.bytes() }.into())),
                    port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                    checksum_offload: false,
                },
                netemul::InterfaceConfig { name: Some("iface_b".into()), ..Default::default() },
            )
            .await
            .expect("join network with realm_b");

        iface_a
            .add_address_and_subnet_route(FIDL_SUBNET_A)
            .await
            .expect("add address should succeed");
        iface_b
            .add_address_and_subnet_route(FIDL_SUBNET_B)
            .await
            .expect("add address should succeed");

        let socket_a = datagram_socket(
            &realm_a
                .connect_to_protocol::<fposix_socket::ProviderMarker>()
                .expect("connect should succeed"),
            fposix_socket::Domain::Ipv4,
            fposix_socket::DatagramSocketProtocol::Udp,
        )
        .await
        .expect("should not have FIDL error")
        .expect("should not have io Error");

        socket_a.bind(&SOCKET_ADDR_A.into()).expect("should succeed");

        let socket_b = datagram_socket(
            &realm_b
                .connect_to_protocol::<fposix_socket::ProviderMarker>()
                .expect("connect should succeed"),
            fposix_socket::Domain::Ipv4,
            fposix_socket::DatagramSocketProtocol::Udp,
        )
        .await
        .expect("should not have FIDL error")
        .expect("should not have io Error");

        socket_b.bind(&SOCKET_ADDR_B.into()).expect("should succeed");

        let mut buf = [std::mem::MaybeUninit::new(0u8); netemul::DEFAULT_MTU as usize];

        let payload = b"hello world!";

        let n = socket_a
            .send_to(payload.as_ref(), &SOCKET_ADDR_B.into())
            .expect("send_to should succeed");
        assert_eq!(n, payload.len());

        let (n, address) = socket_b.recv_from(&mut buf[..]).expect("recv_from should succeed");
        let buf = buf[..n].iter().map(|byte| unsafe { byte.assume_init() }).collect::<Vec<_>>();

        assert_eq!(&buf[..], payload.as_ref());
        assert_eq!(address.as_socket().expect("should be SocketAddr"), SOCKET_ADDR_A);
    }

    #[netstack_test]
    #[variant(N, Netstack)]
    async fn packet_socket_send_receive<N: Netstack>(name: &str) {
        let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();

        let network =
            sandbox.create_network(format!("{name}-test-network")).await.expect("create network");
        let realm_a: netemul::TestRealm<'_> = sandbox
            .create_netstack_realm::<N, _>(format!("{name}-test-realm-a"))
            .expect("create realm");
        let realm_b: netemul::TestRealm<'_> = sandbox
            .create_netstack_realm::<N, _>(format!("{name}-test-realm-b"))
            .expect("create realm");

        const MAC_A: net_types::ethernet::Mac = net_declare::net_mac!("00:00:00:00:00:01");
        const MAC_B: net_types::ethernet::Mac = net_declare::net_mac!("00:00:00:00:00:02");

        let iface_a = realm_a
            .join_network_with(
                &network,
                "iface_a",
                fnetemul_network::EndpointConfig {
                    mtu: netemul::DEFAULT_MTU,
                    mac: Some(Box::new(fnet_ext::MacAddress { octets: MAC_A.bytes() }.into())),
                    port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                    checksum_offload: false,
                },
                netemul::InterfaceConfig { name: Some("iface_a".into()), ..Default::default() },
            )
            .await
            .expect("join network with realm_a");
        let iface_b = realm_b
            .join_network_with(
                &network,
                "iface_b",
                fnetemul_network::EndpointConfig {
                    mtu: netemul::DEFAULT_MTU,
                    mac: Some(Box::new(fnet_ext::MacAddress { octets: MAC_B.bytes() }.into())),
                    port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                    checksum_offload: false,
                },
                netemul::InterfaceConfig { name: Some("iface_b".into()), ..Default::default() },
            )
            .await
            .expect("join network with realm_b");

        let socket_a = packet_socket(
            &realm_a
                .connect_to_protocol::<fpacket::ProviderMarker>()
                .expect("connect should succeed"),
            fpacket::Kind::Network,
        )
        .await
        .expect("should not have FIDL error")
        .expect("should not have io Error");

        let socket_b = packet_socket(
            &realm_b
                .connect_to_protocol::<fpacket::ProviderMarker>()
                .expect("connect should succeed"),
            fpacket::Kind::Network,
        )
        .await
        .expect("should not have FIDL error")
        .expect("should not have io Error");

        let sockaddr_a = libc::sockaddr_ll::from(sockaddr::EthernetSockaddr {
            interface_id: Some(iface_a.id().try_into().expect("nonzero")),
            addr: MAC_A,
            protocol: packet_formats::ethernet::EtherType::Ipv4,
        });

        let sockaddr_b = libc::sockaddr_ll::from(sockaddr::EthernetSockaddr {
            interface_id: Some(iface_b.id().try_into().expect("nonzero")),
            addr: MAC_B,
            protocol: packet_formats::ethernet::EtherType::Ipv4,
        });

        socket_a.bind(&sockaddr_a.into_sockaddr()).expect("should succeed");
        socket_b.bind(&sockaddr_b.into_sockaddr()).expect("should succeed");

        let mut buf = [std::mem::MaybeUninit::new(0u8); netemul::DEFAULT_MTU as usize];

        let payload = b"hello world!";

        let n = socket_a
            .send_to(
                payload.as_ref(),
                &libc::sockaddr_ll::from(sockaddr::EthernetSockaddr {
                    interface_id: Some(iface_a.id().try_into().expect("nonzero")),
                    addr: MAC_B,
                    protocol: packet_formats::ethernet::EtherType::Ipv4,
                })
                .into_sockaddr(),
            )
            .expect("send_to should succeed");
        assert_eq!(n, payload.len());

        // We make multiple attempts because there's no guarantee that we're the
        // exclusive traffic over this interface. In particular, this is being
        // introduced because we're seeing IGMP reports over the interface
        // (https://g-issues.fuchsia.dev/issues/324591565#comment13), but even
        // without that this is a source of flakiness.
        const NUM_ATTEMPTS: i32 = 5;

        for attempt in 1..=NUM_ATTEMPTS {
            let (n, address) = socket_b.recv_from(&mut buf[..]).expect("recv_from should succeed");
            let buf = buf[..n].iter().map(|byte| unsafe { byte.assume_init() }).collect::<Vec<_>>();

            if &buf[..] != payload.as_ref() {
                println!("got buf={buf:?} didn't match wanted={payload:?} in attempt {attempt}");
                continue;
            }

            let got_address = match address.try_to_sockaddr_ll() {
                Some(addr) => addr,
                None => {
                    println!("could not convert {address:?} to sockaddr_ll in attempt {attempt}");
                    continue;
                }
            };

            let want_address = {
                let mut addr = libc::sockaddr_ll::from(sockaddr::EthernetSockaddr {
                    interface_id: Some(iface_b.id().try_into().expect("nonzero")),
                    addr: MAC_A,
                    protocol: packet_formats::ethernet::EtherType::Ipv4,
                });
                const ARPHRD_ETHER: libc::c_ushort = 1;
                addr.sll_hatype = ARPHRD_ETHER;
                addr
            };

            if got_address != want_address {
                println!(
                    "got_address {got_address:?} didn't match \
                want_address {want_address:?} in attempt {attempt}"
                );
                continue;
            }

            println!("succeeded on attempt {attempt}");
            return;
        }

        panic!("failed to receive expected frame in all {NUM_ATTEMPTS} attempts");
    }
}
