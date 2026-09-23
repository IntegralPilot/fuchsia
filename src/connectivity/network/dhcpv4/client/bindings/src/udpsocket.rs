// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZeroU64;
use std::os::fd::AsRawFd;

use dhcp_client_core::deps::UdpSocketProvider;
use fidl_fuchsia_posix as fposix;
use fuchsia_async as fasync;

pub(crate) struct UdpSocket {
    inner: fasync::net::UdpSocket,
}

fn translate_io_error(e: std::io::Error) -> dhcp_client_core::deps::SocketError {
    use fposix::Errno as E;
    match e.raw_os_error().and_then(fposix::Errno::from_primitive) {
        None => dhcp_client_core::deps::SocketError::Other(e),
        Some(errno) => match errno {
            // ============
            // These errors are documented in `man 7 udp`.
            E::Econnrefused => dhcp_client_core::deps::SocketError::Other(e),

            // ============
            // These errors are documented in `man 7 ip`.
            E::Eagain | E::Ealready => panic!(
                "unexpected error {e:?}; nonblocking logic should be
                 handled by fuchsia_async UDP socket wrapper"
            ),
            E::Ehostunreach => dhcp_client_core::deps::SocketError::HostUnreachable,
            E::Enetunreach => dhcp_client_core::deps::SocketError::NetworkUnreachable,
            E::Enobufs | E::Enomem => panic!("out of memory: {e:?}"),
            E::Epipe => dhcp_client_core::deps::SocketError::BrokenPipe,
            E::Eacces
            | E::Eaddrinuse
            | E::Eaddrnotavail
            | E::Einval
            | E::Eisconn
            | E::Emsgsize
            | E::Enoent
            | E::Enopkg
            | E::Enoprotoopt
            | E::Eopnotsupp
            | E::Enotconn
            | E::Eperm
            | E::Esocktnosupport => dhcp_client_core::deps::SocketError::Other(e),

            // TODO(https://fxbug.dev/42077996): Revisit whether we should
            // actually panic on this error once we establish whether this error
            // is set for UDP sockets.
            E::Ehostdown => dhcp_client_core::deps::SocketError::Other(e),

            // ============
            // These errors are documented in `man bind`.
            E::Enotsock => {
                panic!("tried to perform socket operation with non-socket file descriptor: {:?}", e)
            }
            E::Ebadf => {
                panic!("tried to perform socket operation with bad file descriptor: {:?}", e)
            }

            // ============
            // Enodev can be returned when setting SO_BINDTODEVICE.
            E::Enodev => dhcp_client_core::deps::SocketError::NoInterface,

            // ============
            // These errors can be returned from `sendto()` at the socket layer.
            E::Eintr => panic!("got EINTR, should be handled by lower-level library: {:?}", e),
            E::Econnreset => panic!("got ECONNRESET, but we aren't using TCP: {:?}", e),

            // ============
            // UDP sockets can be disconnected via fuchsia.net.sockets/Control.DisconnectIp.
            E::Econnaborted => dhcp_client_core::deps::SocketError::ConnectionAborted,

            // ============
            // The following errors aren't expected to be returned by any of the
            // socket operations we use.
            E::Esrch
            | E::Eio
            | E::E2Big
            | E::Enoexec
            | E::Echild
            | E::Enotblk
            | E::Ebusy
            | E::Eexist
            | E::Exdev
            | E::Enotdir
            | E::Eisdir
            | E::Enfile
            | E::Emfile
            | E::Enotty
            | E::Etxtbsy
            | E::Efbig
            | E::Enospc
            | E::Espipe
            | E::Erofs
            | E::Emlink
            | E::Edom
            | E::Erange
            | E::Edeadlk
            | E::Enametoolong
            | E::Enolck
            | E::Enosys
            | E::Enotempty
            | E::Eloop
            | E::Enomsg
            | E::Eidrm
            | E::Echrng
            | E::El2Nsync
            | E::El3Hlt
            | E::El3Rst
            | E::Elnrng
            | E::Eunatch
            | E::Enocsi
            | E::El2Hlt
            | E::Ebade
            | E::Ebadr
            | E::Exfull
            | E::Enoano
            | E::Ebadrqc
            | E::Ebadslt
            | E::Ebfont
            | E::Enostr
            | E::Enodata
            | E::Etime
            | E::Enosr
            | E::Enonet
            | E::Eremote
            | E::Enolink
            | E::Eadv
            | E::Esrmnt
            | E::Ecomm
            | E::Eproto
            | E::Emultihop
            | E::Edotdot
            | E::Ebadmsg
            | E::Eoverflow
            | E::Enotuniq
            | E::Ebadfd
            | E::Eremchg
            | E::Elibacc
            | E::Elibbad
            | E::Elibscn
            | E::Elibmax
            | E::Elibexec
            | E::Eilseq
            | E::Erestart
            | E::Estrpipe
            | E::Eusers
            | E::Eprototype
            | E::Eprotonosupport
            | E::Epfnosupport
            | E::Eafnosupport
            | E::Enetreset
            | E::Eshutdown
            | E::Etoomanyrefs
            | E::Etimedout
            | E::Einprogress
            | E::Estale
            | E::Euclean
            | E::Enotnam
            | E::Enavail
            | E::Eisnam
            | E::Eremoteio
            | E::Edquot
            | E::Enomedium
            | E::Emediumtype
            | E::Ecanceled
            | E::Enokey
            | E::Ekeyexpired
            | E::Ekeyrevoked
            | E::Ekeyrejected
            | E::Eownerdead
            | E::Enotrecoverable
            | E::Erfkill
            | E::Enxio
            | E::Efault
            | E::Edestaddrreq
            | E::Enetdown
            | E::Ehwpoison => panic!("unexpected error from socket: {:?}", e),
        },
    }
}

impl dhcp_client_core::deps::Socket<std::net::SocketAddr> for UdpSocket {
    async fn send_to(
        &self,
        buf: &[u8],
        addr: std::net::SocketAddr,
    ) -> Result<(), dhcp_client_core::deps::SocketError> {
        let Self { inner } = self;

        let n = inner.send_to(buf, addr).await.map_err(translate_io_error)?;
        // UDP sockets never have short sends.
        assert_eq!(n, buf.len());
        Ok(())
    }

    async fn recv_from(
        &self,
        buf: &mut [u8],
    ) -> Result<
        dhcp_client_core::deps::DatagramInfo<std::net::SocketAddr>,
        dhcp_client_core::deps::SocketError,
    > {
        let Self { inner } = self;
        let (length, address) = inner.recv_from(buf).await.map_err(translate_io_error)?;
        Ok(dhcp_client_core::deps::DatagramInfo { length, address })
    }
}

fn set_bindtoifindex(
    socket: &impl AsRawFd,
    interface_id: NonZeroU64,
) -> Result<(), dhcp_client_core::deps::SocketError> {
    let interface_id =
        libc::c_int::try_from(interface_id.get()).expect("interface_id should fit in c_int");

    // SAFETY: `setsockopt` does not take ownership of anything passed to it.
    if unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTOIFINDEX,
            &interface_id as *const _ as *const libc::c_void,
            std::mem::size_of_val(&interface_id) as libc::socklen_t,
        )
    } != 0
    {
        return Err(translate_io_error(std::io::Error::last_os_error()));
    }

    Ok(())
}

pub(crate) struct LibcUdpSocketProvider {
    pub(crate) interface_id: NonZeroU64,
}

impl UdpSocketProvider for LibcUdpSocketProvider {
    type Sock = UdpSocket;

    async fn bind_new_udp_socket(
        &self,
        bound_addr: std::net::SocketAddr,
    ) -> Result<Self::Sock, dhcp_client_core::deps::SocketError> {
        let socket = std::net::UdpSocket::bind(&bound_addr).map_err(translate_io_error)?;
        set_bindtoifindex(&socket, self.interface_id)?;

        let socket = fasync::net::UdpSocket::from_socket(socket).map_err(translate_io_error)?;
        socket.set_broadcast(true).map_err(translate_io_error)?;
        Ok(UdpSocket { inner: socket })
    }
}

#[cfg(test)]
mod testutil {
    use super::*;
    use fidl_fuchsia_posix_socket as fposix_socket;
    use fidl_fuchsia_posix_socket_ext as fposix_socket_ext;

    pub(crate) struct TestUdpSocketProvider {
        provider: fposix_socket::ProviderProxy,
        interface_id: NonZeroU64,
    }

    impl TestUdpSocketProvider {
        pub(crate) fn new(
            provider: fposix_socket::ProviderProxy,
            interface_id: NonZeroU64,
        ) -> Self {
            Self { provider, interface_id }
        }
    }

    impl UdpSocketProvider for TestUdpSocketProvider {
        type Sock = UdpSocket;

        async fn bind_new_udp_socket(
            &self,
            bound_addr: std::net::SocketAddr,
        ) -> Result<Self::Sock, dhcp_client_core::deps::SocketError> {
            let Self { provider, interface_id } = self;

            let socket = fposix_socket_ext::datagram_socket(
                provider,
                fposix_socket::Domain::Ipv4,
                fposix_socket::DatagramSocketProtocol::Udp,
            )
            .await
            .map_err(|e: fidl::Error| dhcp_client_core::deps::SocketError::FailedToOpen(e.into()))?
            .map_err(translate_io_error)?;

            socket.bind(&bound_addr.into()).map_err(translate_io_error)?;
            socket.set_broadcast(true).map_err(translate_io_error)?;

            set_bindtoifindex(&socket, *interface_id)?;

            let socket =
                fasync::net::UdpSocket::from_socket(socket.into()).map_err(translate_io_error)?;

            Ok(UdpSocket { inner: socket })
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::udpsocket::testutil::TestUdpSocketProvider;
    use dhcp_client_core::deps::{DatagramInfo, Socket as _};
    use fidl_fuchsia_net_ext as fnet_ext;
    use fidl_fuchsia_netemul_network as fnetemul_network;
    use fidl_fuchsia_posix_socket as fposix_socket;
    use futures::{FutureExt as _, join};
    use net_declare::std_socket_addr;
    use netstack_testing_common::realms::TestSandboxExt as _;

    #[fuchsia::test]
    async fn udp_socket_provider_impl_send_receive() {
        let sandbox: netemul::TestSandbox = netemul::TestSandbox::new().unwrap();

        let network = sandbox.create_network("dhcp-test-network").await.expect("create network");
        let realm_a: netemul::TestRealm<'_> = sandbox
            .create_netstack_realm::<netstack_testing_common::realms::Netstack2, _>(
                "dhcp-test-realm-a",
            )
            .expect("create realm");
        let realm_b: netemul::TestRealm<'_> = sandbox
            .create_netstack_realm::<netstack_testing_common::realms::Netstack2, _>(
                "dhcp-test-realm-b",
            )
            .expect("create realm");

        const MAC_A: net_types::ethernet::Mac = net_declare::net_mac!("00:00:00:00:00:01");
        const MAC_B: net_types::ethernet::Mac = net_declare::net_mac!("00:00:00:00:00:02");
        const FIDL_SUBNET_A: fidl_fuchsia_net::Subnet = net_declare::fidl_subnet!("1.1.1.1/24");
        const SOCKET_ADDR_A: std::net::SocketAddr = std_socket_addr!("1.1.1.1:1111");
        const FIDL_SUBNET_B: fidl_fuchsia_net::Subnet = net_declare::fidl_subnet!("1.1.1.2/24");
        const SOCKET_ADDR_B: std::net::SocketAddr = std_socket_addr!("1.1.1.2:2222");

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

        let socket_a = TestUdpSocketProvider::new(
            realm_a.connect_to_protocol::<fposix_socket::ProviderMarker>().unwrap(),
            NonZeroU64::new(iface_a.id()).unwrap(),
        )
        .bind_new_udp_socket(SOCKET_ADDR_A)
        .await
        .expect("get udp socket");

        let socket_b = TestUdpSocketProvider::new(
            realm_b.connect_to_protocol::<fposix_socket::ProviderMarker>().unwrap(),
            NonZeroU64::new(iface_b.id()).unwrap(),
        )
        .bind_new_udp_socket(SOCKET_ADDR_B)
        .await
        .expect("get udp socket");

        let mut buf = [0u8; netemul::DEFAULT_MTU as usize];

        let payload = b"hello world!";

        let DatagramInfo { length, address } = {
            let send_fut = async {
                socket_a.send_to(payload.as_ref(), SOCKET_ADDR_B).await.expect("send_to");
            }
            .fuse();

            let receive_fut =
                async { socket_b.recv_from(&mut buf).await.expect("recv_from") }.fuse();

            let ((), datagram_info) = join!(send_fut, receive_fut);
            datagram_info
        };

        assert_eq!(&buf[..length], payload.as_ref());
        assert_eq!(address, SOCKET_ADDR_A);
    }
}
