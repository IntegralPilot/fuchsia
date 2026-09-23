// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides implementation for handling Netlink requests and transforming them
//! into requests for [`eventloop::SockDiagEventLoop`].
//!
//! Instead of using a tangle of if statements, this opts instead for a sea of
//! trait soup. What follows is a high-level primer to your main course.
//!
//! There are ultimately three `NETLINK_SOCK_DIAG` operations: Dump a single
//! socket, dump multiple sockets, or destroy a single socket. These three
//! operations are each represented by a struct: [`GetOne`], [`Dump`], and
//! [`Destroy`]. These implement the [`RequestType`] trait, which pulls together
//! all the behavior into a single place.
//!
//! These three operations have two axes of semantics: How each field is
//! converted into a matcher (e.g. is 0 a wildcard?), and how to ultimately
//! convert those matchers into a request for [`eventloop::SockDiagEventLoop`].
//! The first axis is represented by the [`MatcherPolicy`] trait, and the second
//! axis is represented inside the top-level [`RequestType`] trait.
//!
//! One level down from this is the [`TransportProtocolConverter`] trait, which
//! abstracts over how to convert from the generic transport protocol
//! information in the request (e.g. a generic port) to protocol-specific
//! matchers (e.g. a TCP port).

use std::marker::PhantomData;
use std::num::{NonZeroU16, NonZeroU64};

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_matchers as fnet_matchers;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_sockets as fnet_sockets;
use fidl_fuchsia_net_sockets_ext as fnet_sockets_ext;
use futures::SinkExt;
use futures::channel::{mpsc, oneshot};
use net_types::SpecifiedAddress as _;
use net_types::ip::{Ip, IpInvariant, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, SubnetEither};
use netlink_packet_core::{NLM_F_ACK, NLM_F_DUMP, NetlinkMessage, NetlinkPayload};
use netlink_packet_sock_diag::inet::bytecode::{self, Bytecode};
use netlink_packet_sock_diag::inet::{ExtensionFlags, InetRequest, SocketId, StateFlags};
use netlink_packet_sock_diag::{SockDiagRequest, SockDiagResponse, TCP_CLOSE, TCP_ESTABLISHED};

use crate::client::InternalClient;
use crate::logging::{log_error, log_warn};
use crate::messaging::Sender;
use crate::netlink_packet;
use crate::netlink_packet::errno::Errno;
use crate::protocol_family::NetlinkFamilyRequestHandler;
use crate::protocol_family::sock_diag::{NetlinkSockDiag, eventloop};

#[derive(Clone)]
pub(crate) struct NetlinkSockDiagRequestHandler<S: Sender<SockDiagResponse>> {
    pub(crate) sock_diag_request_sink: mpsc::Sender<eventloop::Request<S>>,
}

impl<S: Sender<SockDiagResponse>> NetlinkFamilyRequestHandler<NetlinkSockDiag, S>
    for NetlinkSockDiagRequestHandler<S>
{
    async fn handle_request(
        &mut self,
        req: NetlinkMessage<SockDiagRequest>,
        client: &mut InternalClient<NetlinkSockDiag, S>,
    ) {
        let Self { sock_diag_request_sink } = self;

        let (req_header, payload) = req.into_parts();
        let req = match payload {
            NetlinkPayload::InnerMessage(p) => p,
            p => {
                log_warn!(
                    "Ignoring request from client {} with unexpected payload: {:?}",
                    client,
                    p
                );
                return;
            }
        };

        let is_dump = req_header.flags & NLM_F_DUMP == NLM_F_DUMP;
        let expects_ack = req_header.flags & NLM_F_ACK == NLM_F_ACK;

        let args = match req {
            SockDiagRequest::InetRequest(inet_request) => {
                let ret = if is_dump {
                    construct_request::<Dump>(inet_request)
                } else {
                    // Linux gets these backwards, but only for single-socket
                    // UDP get requests.
                    //
                    // Yes, this breaks the whole policy abstraction, but it's
                    // WAY clearer than trying to integrate it into the trait
                    // structure.
                    let inet_request = if inet_request.protocol as u32 == linux_uapi::IPPROTO_UDP {
                        let SocketId {
                            source_address,
                            source_port,
                            destination_address,
                            destination_port,
                            interface_id,
                            cookie,
                        } = inet_request.socket_id;

                        InetRequest {
                            socket_id: SocketId {
                                source_address: destination_address,
                                source_port: destination_port,
                                destination_address: source_address,
                                destination_port: source_port,
                                interface_id,
                                cookie,
                            },
                            ..inet_request
                        }
                    } else {
                        inet_request
                    };

                    construct_request::<GetOne>(inet_request)
                };

                match ret {
                    Ok(args) => args,
                    Err(e) => {
                        client.send_unicast(netlink_packet::new_error(Err(e), req_header));
                        return;
                    }
                }
            }
            SockDiagRequest::InetSockDestroy(inet_request) => {
                if is_dump {
                    client.send_unicast(netlink_packet::new_error(Err(Errno::EINVAL), req_header));
                    return;
                }

                match construct_request::<Destroy>(inet_request) {
                    Ok(args) => args,
                    Err(e) => {
                        client.send_unicast(netlink_packet::new_error(Err(e), req_header));
                        return;
                    }
                }
            }
            SockDiagRequest::UnixRequest(_) => {
                log_warn!(
                    "Received unsupported UNIX NETLINK_SOCK_DIAG request: \
                    {:?} is_dump={}, expects_ack={}",
                    req,
                    is_dump,
                    expects_ack,
                );
                client.send_unicast(netlink_packet::new_error(
                    Err(crate::netlink_packet::errno::Errno::ENOTSUP),
                    req_header,
                ));

                return;
            }
        };

        let (completer, waiter) = oneshot::channel::<Result<(), eventloop::RequestError>>();
        sock_diag_request_sink
            .send(eventloop::Request {
                args,
                sequence_number: req_header.sequence_number,
                client: client.clone(),
                completer,
            })
            .await
            .expect("sock_diag event loop should never terminate");

        match waiter.await.expect("sock_diag loop should have handled the request") {
            Ok(()) => {
                if is_dump {
                    client.send_unicast(netlink_packet::new_done(req_header))
                } else if expects_ack {
                    client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                }
            }
            Err(e) => {
                client.send_unicast(netlink_packet::new_error(Err(e.into_errno()), req_header))
            }
        }
    }
}

trait RequestType {
    type MatcherPolicy<I, T>: MatcherPolicy
    where
        T: TransportConverter,
        I: Ip;

    fn into_request<I, T>(
        matchers: Self::MatcherPolicy<I, T>,
        extensions: ExtensionFlags,
    ) -> eventloop::RequestArgs
    where
        I: Ip,
        T: TransportConverter;
}

struct GetOne;

impl RequestType for GetOne {
    type MatcherPolicy<I, T>
        = SingleSocketMatcherPolicy<I, T>
    where
        I: Ip,
        T: TransportConverter;

    fn into_request<I, T>(
        matchers: Self::MatcherPolicy<I, T>,
        extensions: ExtensionFlags,
    ) -> eventloop::RequestArgs
    where
        I: Ip,
        T: TransportConverter,
    {
        let Self::MatcherPolicy { matchers, transport: _, ip: _ } = matchers;

        eventloop::RequestArgs::Get(matchers, T::extensions(extensions), false)
    }
}

struct Dump;

impl RequestType for Dump {
    type MatcherPolicy<I, T>
        = MultiSocketMatcherPolicy<I, T>
    where
        I: Ip,
        T: TransportConverter;

    fn into_request<I, T>(
        matchers: Self::MatcherPolicy<I, T>,
        extensions: ExtensionFlags,
    ) -> eventloop::RequestArgs
    where
        I: Ip,
        T: TransportConverter,
    {
        let Self::MatcherPolicy { matchers, transport: _, ip: _ } = matchers;

        eventloop::RequestArgs::Get(matchers, T::extensions(extensions), true)
    }
}

struct Destroy;

impl RequestType for Destroy {
    type MatcherPolicy<I, T>
        = SingleSocketMatcherPolicy<I, T>
    where
        I: Ip,
        T: TransportConverter;

    fn into_request<I, T>(
        matchers: Self::MatcherPolicy<I, T>,
        _extensions: ExtensionFlags,
    ) -> eventloop::RequestArgs
    where
        I: Ip,
        T: TransportConverter,
    {
        let Self::MatcherPolicy { matchers, transport: _, ip: _ } = matchers;

        eventloop::RequestArgs::Destroy(matchers)
    }
}

trait MatcherPolicy: Default {
    fn push_family(&mut self);

    fn push_states(&mut self, states: StateFlags);

    fn push_src_port(&mut self, port: Option<NonZeroU16>);

    fn push_dst_port(&mut self, port: Option<NonZeroU16>);

    fn push_src_addr(&mut self, addr: std::net::IpAddr) -> Result<(), Errno>;

    fn push_dst_addr(&mut self, addr: std::net::IpAddr) -> Result<(), Errno>;

    fn push_cookie(&mut self, cookie: u64);

    fn push_interface(&mut self, interface: u32);

    fn apply_nlas(
        &mut self,
        nlas: &[netlink_packet_sock_diag::inet::RequestNla],
    ) -> Result<(), Errno>;
}

struct SingleSocketMatcherPolicy<I, T> {
    matchers: Vec<fnet_sockets_ext::IpSocketMatcher>,
    transport: PhantomData<T>,
    ip: PhantomData<I>,
}

impl<I, T> Default for SingleSocketMatcherPolicy<I, T> {
    fn default() -> Self {
        Self { matchers: Default::default(), transport: PhantomData, ip: PhantomData }
    }
}

impl<I, T> MatcherPolicy for SingleSocketMatcherPolicy<I, T>
where
    I: Ip,
    T: TransportConverter,
{
    fn push_family(&mut self) {
        self.matchers.push(fnet_sockets_ext::IpSocketMatcher::Family(I::VERSION))
    }

    fn push_states(&mut self, _states: StateFlags) {
        // Linux only uses this for filtering multi-socket requests.
    }

    fn push_src_port(&mut self, port: Option<NonZeroU16>) {
        self.matchers.push(T::convert_src_port(port))
    }

    fn push_dst_port(&mut self, port: Option<NonZeroU16>) {
        self.matchers.push(T::convert_dst_port(port))
    }

    fn push_src_addr(&mut self, addr: std::net::IpAddr) -> Result<(), Errno> {
        self.matchers.push(fnet_sockets_ext::IpSocketMatcher::SrcAddr(convert_address::<I>(addr)?));
        Ok(())
    }

    fn push_dst_addr(&mut self, addr: std::net::IpAddr) -> Result<(), Errno> {
        self.matchers.push(fnet_sockets_ext::IpSocketMatcher::DstAddr(convert_address::<I>(addr)?));
        Ok(())
    }

    fn push_cookie(&mut self, cookie: u64) {
        // Linux treats the all 1s cookie as a wildcard.
        if cookie != u64::MAX {
            self.matchers.push(fnet_sockets_ext::IpSocketMatcher::Cookie(
                fnet_matchers::SocketCookie { cookie, invert: false },
            ))
        }
    }

    fn push_interface(&mut self, interface: u32) {
        if let Some(interface) = NonZeroU64::new(interface as u64) {
            self.matchers.push(fnet_sockets_ext::IpSocketMatcher::BoundInterface(
                fnet_matchers_ext::BoundInterface::Bound(fnet_matchers_ext::Interface::Id(
                    interface,
                )),
            ));
        }
    }

    fn apply_nlas(
        &mut self,
        _nlas: &[netlink_packet_sock_diag::inet::RequestNla],
    ) -> Result<(), Errno> {
        // Bytecode programs are the only NLA we care about, and they're only
        // supported for dump requests.
        Ok(())
    }
}

/// A [`MatcherPolicy`] used for NLM_F_DUMP requests, where unset fields are
/// (generally) treated as wildcards.
struct MultiSocketMatcherPolicy<I, T> {
    matchers: Vec<fnet_sockets_ext::IpSocketMatcher>,
    transport: PhantomData<T>,
    ip: PhantomData<I>,
}

impl<I, T> Default for MultiSocketMatcherPolicy<I, T> {
    fn default() -> Self {
        Self { matchers: Default::default(), transport: PhantomData, ip: PhantomData }
    }
}

impl<I, T> MatcherPolicy for MultiSocketMatcherPolicy<I, T>
where
    I: Ip,
    T: TransportConverter,
{
    fn push_family(&mut self) {
        self.matchers.push(fnet_sockets_ext::IpSocketMatcher::Family(I::VERSION))
    }

    fn push_states(&mut self, states: StateFlags) {
        self.matchers.push(T::convert_states(states))
    }

    fn push_src_port(&mut self, port: Option<NonZeroU16>) {
        // Treat an unset port as a wildcard.
        if let Some(port) = port {
            self.matchers.push(T::convert_src_port(Some(port)))
        }
    }

    fn push_dst_port(&mut self, port: Option<NonZeroU16>) {
        // Treat an unset port as a wildcard.
        if let Some(port) = port {
            self.matchers.push(T::convert_dst_port(Some(port)))
        }
    }

    fn push_src_addr(&mut self, _addr: std::net::IpAddr) -> Result<(), Errno> {
        // Linux doesn't look at the address fields of dump requests. A
        // bytecode program must be used for address-based filtering.
        Ok(())
    }

    fn push_dst_addr(&mut self, _addr: std::net::IpAddr) -> Result<(), Errno> {
        // See above.
        Ok(())
    }

    fn push_cookie(&mut self, _cookie: u64) {
        // Cookie-based filtering can only happen in bytecode.
    }

    fn push_interface(&mut self, _interface: u32) {
        // Interface-based filtering can only happen in bytecode.
    }

    fn apply_nlas(
        &mut self,
        nlas: &[netlink_packet_sock_diag::inet::RequestNla],
    ) -> Result<(), Errno> {
        for nla in nlas {
            match nla {
                netlink_packet_sock_diag::inet::RequestNla::Bytecode(bc) => {
                    let parsed =
                        match netlink_packet_sock_diag::inet::bytecode::Bytecode::parse(&bc[..]) {
                            Ok(parsed) => parsed,
                            Err(e) => {
                                log_warn!("Failed to parse bytecode program: {e:?}");
                                return Err(Errno::EINVAL);
                            }
                        };

                    matchers_from_bytecode::<I, _>(&mut self.matchers, &parsed)?;
                }
                _ => {
                    log_warn!("Received unsupported NLA: {nla:?}");
                    return Err(Errno::ENOTSUP);
                }
            }
        }
        Ok(())
    }
}

fn construct_request<R: RequestType>(
    InetRequest { family, protocol, extensions, states, socket_id, nlas }: InetRequest,
) -> Result<eventloop::RequestArgs, Errno> {
    match family as u32 {
        linux_uapi::AF_INET => construct_request_with_ip_version::<R, Ipv4>(
            protocol, extensions, states, socket_id, &nlas,
        ),
        linux_uapi::AF_INET6 => construct_request_with_ip_version::<R, Ipv6>(
            protocol, extensions, states, socket_id, &nlas,
        ),
        _ => {
            log_warn!(
                "Received NETLINK_SOCK_DIAG request for \
                unsupported address family: {family}"
            );
            Err(Errno::ENOTSUP)
        }
    }
}

fn construct_request_with_ip_version<R: RequestType, I: Ip>(
    protocol: u8,
    extensions: ExtensionFlags,
    states: StateFlags,
    socket_id: SocketId,
    nlas: &[netlink_packet_sock_diag::inet::RequestNla],
) -> Result<eventloop::RequestArgs, Errno> {
    match protocol as u32 {
        linux_uapi::IPPROTO_TCP => {
            construct_request_inner::<R, I, Tcp>(extensions, states, socket_id, nlas)
        }
        linux_uapi::IPPROTO_UDP => {
            construct_request_inner::<R, I, Udp>(extensions, states, socket_id, nlas)
        }
        _ => {
            log_warn!(
                "Received NETLINK_SOCK_DIAG request for \
                unsupported protocol: {protocol}"
            );
            Err(Errno::ENOTSUP)
        }
    }
}

fn construct_request_inner<R: RequestType, I: Ip, T: TransportConverter>(
    extensions: ExtensionFlags,
    states: StateFlags,
    SocketId {
        source_address,
        source_port,
        destination_address,
        destination_port,
        interface_id,
        cookie,
    }: SocketId,
    nlas: &[netlink_packet_sock_diag::inet::RequestNla],
) -> Result<eventloop::RequestArgs, Errno> {
    let mut matchers = R::MatcherPolicy::<I, T>::default();

    matchers.push_family();
    matchers.push_states(states);
    matchers.push_src_port(NonZeroU16::new(source_port));
    matchers.push_dst_port(NonZeroU16::new(destination_port));
    matchers.push_src_addr(source_address)?;
    matchers.push_dst_addr(destination_address)?;
    matchers.push_interface(interface_id);
    matchers.push_cookie(u64::from_ne_bytes(cookie));

    matchers.apply_nlas(nlas)?;

    Ok(R::into_request::<I, T>(matchers, extensions))
}

trait TransportConverter {
    fn convert_states(states: StateFlags) -> fnet_sockets_ext::IpSocketMatcher;
    fn convert_src_port(port: Option<NonZeroU16>) -> fnet_sockets_ext::IpSocketMatcher;
    fn convert_dst_port(port: Option<NonZeroU16>) -> fnet_sockets_ext::IpSocketMatcher;
    fn extensions(flags: ExtensionFlags) -> fnet_sockets::Extensions;
}

struct Tcp;

impl TransportConverter for Tcp {
    fn convert_states(states: StateFlags) -> fnet_sockets_ext::IpSocketMatcher {
        // Linux states are 1-based, FIDL states are 0-based.
        fnet_sockets_ext::IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Tcp(
            fnet_matchers_ext::TcpSocket::States(fnet_matchers::TcpState::from_bits_truncate(
                (states.bits() >> 1) as u32,
            )),
        ))
    }

    fn convert_src_port(port: Option<NonZeroU16>) -> fnet_sockets_ext::IpSocketMatcher {
        fnet_sockets_ext::IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Tcp(
            fnet_matchers_ext::TcpSocket::SrcPort(convert_port(port)),
        ))
    }

    fn convert_dst_port(port: Option<NonZeroU16>) -> fnet_sockets_ext::IpSocketMatcher {
        fnet_sockets_ext::IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Tcp(
            fnet_matchers_ext::TcpSocket::DstPort(convert_port(port)),
        ))
    }

    fn extensions(flags: ExtensionFlags) -> fnet_sockets::Extensions {
        if flags.contains(ExtensionFlags::INFO) {
            fnet_sockets::Extensions::TCP_INFO
        } else {
            fnet_sockets::Extensions::empty()
        }
    }
}

struct Udp;

impl TransportConverter for Udp {
    fn convert_states(states: StateFlags) -> fnet_sockets_ext::IpSocketMatcher {
        let mut s = fnet_matchers::UdpState::empty();
        let bits = states.bits();
        // Linux uses the TCP state constants for non-TCP sockets.
        if bits & (1 << TCP_ESTABLISHED) != 0 {
            s |= fnet_matchers::UdpState::CONNECTED;
        }
        if bits & (1 << TCP_CLOSE) != 0 {
            s |= fnet_matchers::UdpState::BOUND;
        }
        fnet_sockets_ext::IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Udp(
            fidl_fuchsia_net_matchers_ext::UdpSocket::States(s),
        ))
    }

    fn convert_src_port(port: Option<NonZeroU16>) -> fnet_sockets_ext::IpSocketMatcher {
        fnet_sockets_ext::IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Udp(
            fnet_matchers_ext::UdpSocket::SrcPort(convert_port(port)),
        ))
    }

    fn convert_dst_port(port: Option<NonZeroU16>) -> fnet_sockets_ext::IpSocketMatcher {
        fnet_sockets_ext::IpSocketMatcher::Proto(fnet_matchers_ext::SocketTransportProtocol::Udp(
            fnet_matchers_ext::UdpSocket::DstPort(convert_port(port)),
        ))
    }

    fn extensions(_flags: ExtensionFlags) -> fnet_sockets::Extensions {
        fnet_sockets::Extensions::empty()
    }
}

fn convert_port(port: Option<NonZeroU16>) -> fnet_matchers_ext::BoundPort {
    match port {
        Some(port) => fnet_matchers_ext::BoundPort::Bound(fnet_matchers_ext::Port::new_single(
            port.get(),
            false,
        )),
        None => fnet_matchers_ext::BoundPort::Unbound,
    }
}

fn convert_address<I: Ip>(
    addr: std::net::IpAddr,
) -> Result<fnet_matchers_ext::BoundAddress, Errno> {
    let addr = I::map_ip::<_, Option<I::Addr>>(
        IpInvariant(addr),
        |IpInvariant(addr)| match addr {
            std::net::IpAddr::V4(addr) => Some(addr.into()),
            _ => None,
        },
        |IpInvariant(addr)| match addr {
            std::net::IpAddr::V6(addr) => Some(addr.into()),
            _ => None,
        },
    )
    .ok_or(Errno::EINVAL)?;

    if addr.is_specified() {
        Ok(fnet_matchers_ext::BoundAddress::Bound(fnet_matchers_ext::Address {
            matcher: fnet_matchers_ext::AddressMatcherType::Range(
                fnet_matchers_ext::AddressRange::new_single::<I>(addr),
            ),
            invert: false,
        }))
    } else {
        Ok(fnet_matchers_ext::BoundAddress::Unbound)
    }
}

/// Parses the bytecode program against a small number of known forms in order
/// to extract matchers from it.
pub(crate) fn matchers_from_bytecode<I, M>(
    matchers: &mut M,
    bytecode: &Bytecode,
) -> Result<(), Errno>
where
    I: Ip,
    M: Extend<fnet_sockets_ext::IpSocketMatcher>,
{
    match bytecode.0.as_slice() {
        [
            bytecode::Instruction::Condition {
                yes: bytecode::Action::Accept,
                no: bytecode::Action::Reject,
                condition:
                    bytecode::Condition::SrcTuple(bytecode::TupleCondition {
                        prefix_len,
                        // This means "match any port", which translated to FIDL
                        // is no matcher at all.
                        port: None,
                        addr,
                    }),
            },
        ] => {
            let subnet = match I::map_ip_in::<_, Option<SubnetEither>>(
                IpInvariant(addr),
                |IpInvariant(addr)| match addr {
                    Some(std::net::IpAddr::V4(addr)) => {
                        net_types::ip::Subnet::new((*addr).into(), *prefix_len)
                            .ok()
                            .map(|s: net_types::ip::Subnet<Ipv4Addr>| s.into())
                    }
                    _ => None,
                },
                |IpInvariant(addr)| match addr {
                    Some(std::net::IpAddr::V6(addr)) => {
                        net_types::ip::Subnet::new((*addr).into(), *prefix_len)
                            .ok()
                            .map(|s: net_types::ip::Subnet<Ipv6Addr>| s.into())
                    }
                    _ => None,
                },
            ) {
                Some(subnet) => subnet,
                None => {
                    log_warn!("Failed to parse subnet from bytecode: {addr:?}/{prefix_len}");
                    return Err(Errno::EINVAL);
                }
            };

            matchers.extend(std::iter::once(fnet_sockets_ext::IpSocketMatcher::SrcAddr(
                fnet_matchers_ext::BoundAddress::Bound(fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Subnet(subnet.into()),
                    invert: false,
                }),
            )));
            Ok(())
        }
        [
            bytecode::Instruction::Condition {
                yes: bytecode::Action::AdvanceBy(forward_1),
                no: bytecode::Action::Reject,
                condition: bytecode::Condition::Mark { mark: mark1, mask: mask1 },
            },
            bytecode::Instruction::Condition {
                yes: bytecode::Action::AdvanceBy(forward_2),
                no: bytecode::Action::Accept,
                condition: bytecode::Condition::Mark { mark: mark2, mask: mask2 },
            },
            bytecode::Instruction::Jmp(bytecode::Action::Reject),
        ] if forward_1.get() == 1 && forward_2.get() == 1 => {
            matchers.extend(std::iter::once(fnet_sockets_ext::IpSocketMatcher::Mark(
                fnet_matchers_ext::MarkInDomain {
                    domain: fnet::MarkDomain::Mark1,
                    mark: fnet_matchers_ext::Mark::Marked {
                        mask: *mask1,
                        between: *mark1..=*mark1,
                        invert: false,
                    },
                },
            )));
            matchers.extend(std::iter::once(fnet_sockets_ext::IpSocketMatcher::Mark(
                fnet_matchers_ext::MarkInDomain {
                    domain: fnet::MarkDomain::Mark1,
                    mark: fnet_matchers_ext::Mark::Marked {
                        mask: *mask2,
                        between: *mark2..=*mark2,
                        invert: true,
                    },
                },
            )));
            Ok(())
        }
        [] => Ok(()),
        _ => {
            log_error!("Received unexpected bytecode program: {bytecode:?}");
            Err(Errno::ENOTSUP)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use net_types::ip::IpAddress;
    use smallvec::smallvec;

    use crate::protocol_family::sock_diag::testutil::TestIpExt;

    #[ip_test(I)]
    fn construct_request_dump<I: TestIpExt>() {
        let socket_id = SocketId {
            source_address: I::SRC_ADDR.to_ip_addr().into(),
            source_port: 1234,
            destination_address: I::DST_ADDR.to_ip_addr().into(),
            destination_port: 8080,
            interface_id: 12,
            cookie: [0xA1; 8],
        };

        let req = InetRequest {
            family: I::LINUX_FAMILY,
            protocol: linux_uapi::IPPROTO_UDP as u8,
            extensions: ExtensionFlags::empty(),
            states: StateFlags::ESTABLISHED,
            socket_id,
            nlas: smallvec![],
        };

        let args = construct_request::<Dump>(req).expect("valid request");
        let matchers =
            assert_matches!(args, eventloop::RequestArgs::Get(matchers, _, true) => matchers);
        assert_eq!(
            matchers,
            [
                fnet_sockets_ext::IpSocketMatcher::Family(I::VERSION),
                fnet_sockets_ext::IpSocketMatcher::Proto(
                    fnet_matchers_ext::SocketTransportProtocol::Udp(
                        fnet_matchers_ext::UdpSocket::States(fnet_matchers::UdpState::CONNECTED),
                    ),
                ),
                fnet_sockets_ext::IpSocketMatcher::Proto(
                    fnet_matchers_ext::SocketTransportProtocol::Udp(
                        fnet_matchers_ext::UdpSocket::SrcPort(fnet_matchers_ext::BoundPort::Bound(
                            fnet_matchers_ext::Port::new_single(1234, false),
                        )),
                    ),
                ),
                fnet_sockets_ext::IpSocketMatcher::Proto(
                    fnet_matchers_ext::SocketTransportProtocol::Udp(
                        fnet_matchers_ext::UdpSocket::DstPort(fnet_matchers_ext::BoundPort::Bound(
                            fnet_matchers_ext::Port::new_single(8080, false),
                        )),
                    ),
                ),
            ]
        );
    }

    #[ip_test(I)]
    fn construct_request_get_one<I: TestIpExt>() {
        let socket_id = SocketId {
            source_address: I::SRC_ADDR.to_ip_addr().into(),
            source_port: 1234,
            destination_address: I::DST_ADDR.to_ip_addr().into(),
            destination_port: 8080,
            interface_id: 12,
            cookie: [0xA1; 8],
        };

        let req = InetRequest {
            family: I::LINUX_FAMILY,
            protocol: linux_uapi::IPPROTO_UDP as u8,
            extensions: ExtensionFlags::empty(),
            states: StateFlags::empty(),
            socket_id,
            nlas: smallvec![],
        };

        let args = construct_request::<GetOne>(req).expect("valid request");
        let matchers =
            assert_matches!(args, eventloop::RequestArgs::Get(matchers, _, false) => matchers);
        assert_eq!(
            matchers,
            [
                fnet_sockets_ext::IpSocketMatcher::Family(I::VERSION),
                fnet_sockets_ext::IpSocketMatcher::Proto(
                    fnet_matchers_ext::SocketTransportProtocol::Udp(
                        fnet_matchers_ext::UdpSocket::SrcPort(fnet_matchers_ext::BoundPort::Bound(
                            fnet_matchers_ext::Port::new_single(1234, false),
                        )),
                    ),
                ),
                fnet_sockets_ext::IpSocketMatcher::Proto(
                    fnet_matchers_ext::SocketTransportProtocol::Udp(
                        fnet_matchers_ext::UdpSocket::DstPort(fnet_matchers_ext::BoundPort::Bound(
                            fnet_matchers_ext::Port::new_single(8080, false),
                        )),
                    ),
                ),
                fnet_sockets_ext::IpSocketMatcher::SrcAddr(fnet_matchers_ext::BoundAddress::Bound(
                    fnet_matchers_ext::Address {
                        matcher: fnet_matchers_ext::AddressMatcherType::Range(
                            fnet_matchers_ext::AddressRange::new_single::<I>(I::SRC_ADDR,),
                        ),
                        invert: false,
                    },
                )),
                fnet_sockets_ext::IpSocketMatcher::DstAddr(fnet_matchers_ext::BoundAddress::Bound(
                    fnet_matchers_ext::Address {
                        matcher: fnet_matchers_ext::AddressMatcherType::Range(
                            fnet_matchers_ext::AddressRange::new_single::<I>(I::DST_ADDR,),
                        ),
                        invert: false,
                    },
                )),
                fnet_sockets_ext::IpSocketMatcher::BoundInterface(
                    fnet_matchers_ext::BoundInterface::Bound(fnet_matchers_ext::Interface::Id(
                        NonZeroU64::new(12).unwrap(),
                    )),
                ),
                fnet_sockets_ext::IpSocketMatcher::Cookie(fnet_matchers::SocketCookie {
                    cookie: 0xA1A1A1A1A1A1A1A1,
                    invert: false,
                }),
            ]
        );
    }

    #[ip_test(I)]
    fn construct_request_destroy<I: TestIpExt>() {
        let socket_id = SocketId {
            source_address: I::SRC_ADDR.to_ip_addr().into(),
            source_port: 1234,
            destination_address: I::DST_ADDR.to_ip_addr().into(),
            destination_port: 8080,
            interface_id: 12,
            cookie: [0xA1; 8],
        };

        let req = InetRequest {
            family: I::LINUX_FAMILY,
            protocol: linux_uapi::IPPROTO_UDP as u8,
            extensions: ExtensionFlags::empty(),
            states: StateFlags::empty(),
            socket_id,
            nlas: smallvec![],
        };
        let args = construct_request::<Destroy>(req).expect("valid request");
        let matchers = assert_matches!(args, eventloop::RequestArgs::Destroy(matchers) => matchers);

        assert_eq!(
            matchers,
            [
                fnet_sockets_ext::IpSocketMatcher::Family(I::VERSION),
                fnet_sockets_ext::IpSocketMatcher::Proto(
                    fnet_matchers_ext::SocketTransportProtocol::Udp(
                        fnet_matchers_ext::UdpSocket::SrcPort(fnet_matchers_ext::BoundPort::Bound(
                            fnet_matchers_ext::Port::new_single(1234, false),
                        )),
                    ),
                ),
                fnet_sockets_ext::IpSocketMatcher::Proto(
                    fnet_matchers_ext::SocketTransportProtocol::Udp(
                        fnet_matchers_ext::UdpSocket::DstPort(fnet_matchers_ext::BoundPort::Bound(
                            fnet_matchers_ext::Port::new_single(8080, false),
                        )),
                    ),
                ),
                fnet_sockets_ext::IpSocketMatcher::SrcAddr(fnet_matchers_ext::BoundAddress::Bound(
                    fnet_matchers_ext::Address {
                        matcher: fnet_matchers_ext::AddressMatcherType::Range(
                            fnet_matchers_ext::AddressRange::new_single::<I>(I::SRC_ADDR,),
                        ),
                        invert: false,
                    },
                )),
                fnet_sockets_ext::IpSocketMatcher::DstAddr(fnet_matchers_ext::BoundAddress::Bound(
                    fnet_matchers_ext::Address {
                        matcher: fnet_matchers_ext::AddressMatcherType::Range(
                            fnet_matchers_ext::AddressRange::new_single::<I>(I::DST_ADDR,),
                        ),
                        invert: false,
                    },
                )),
                fnet_sockets_ext::IpSocketMatcher::BoundInterface(
                    fnet_matchers_ext::BoundInterface::Bound(fnet_matchers_ext::Interface::Id(
                        NonZeroU64::new(12).unwrap(),
                    )),
                ),
                fnet_sockets_ext::IpSocketMatcher::Cookie(fnet_matchers::SocketCookie {
                    cookie: 0xA1A1A1A1A1A1A1A1,
                    invert: false,
                }),
            ]
        );
    }

    #[ip_test(I)]
    fn construct_request_errors<I: TestIpExt>() {
        let socket_id = match I::VERSION {
            net_types::ip::IpVersion::V4 => SocketId::new_v4(),
            net_types::ip::IpVersion::V6 => SocketId::new_v6(),
        };

        // Invalid Family (AF_PACKET = 17)
        let req = InetRequest {
            family: 17,
            protocol: linux_uapi::IPPROTO_TCP as u8,
            extensions: ExtensionFlags::empty(),
            states: StateFlags::empty(),
            socket_id: socket_id.clone(),
            nlas: smallvec![],
        };
        assert_eq!(construct_request::<Dump>(req), Err(Errno::ENOTSUP));

        // Invalid Protocol
        let req = InetRequest {
            family: I::LINUX_FAMILY,
            protocol: linux_uapi::IPPROTO_ICMP as u8,
            extensions: ExtensionFlags::empty(),
            states: StateFlags::empty(),
            socket_id,
            nlas: smallvec![],
        };
        assert_eq!(construct_request::<Dump>(req), Err(Errno::ENOTSUP));

        // SocketId is the wrong IP version is invalid only for the
        // single-socket matchers. Dump doesn't actually look at the fields.
        let req = InetRequest {
            family: I::LINUX_FAMILY,
            protocol: linux_uapi::IPPROTO_TCP as u8,
            extensions: ExtensionFlags::empty(),
            states: StateFlags::empty(),
            socket_id: match I::VERSION {
                net_types::ip::IpVersion::V4 => SocketId::new_v6(),
                net_types::ip::IpVersion::V6 => SocketId::new_v4(),
            },
            nlas: smallvec![],
        };
        assert_eq!(construct_request::<GetOne>(req.clone()), Err(Errno::EINVAL));
        assert_eq!(construct_request::<Destroy>(req.clone()), Err(Errno::EINVAL));
        assert_matches!(construct_request::<Dump>(req), Ok(_));
    }

    #[ip_test(I)]
    fn matchers_from_bytecode_one<I: TestIpExt>() {
        let (addr, prefix_len) = I::map_ip::<_, (I::Addr, _)>(
            (),
            |()| (net_types::ip::Ipv4Addr::new([192, 0, 2, 0]), 24),
            |()| (net_types::ip::Ipv6Addr::new([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0]), 64),
        );

        let bytecode = Bytecode(vec![bytecode::Instruction::Condition {
            yes: bytecode::Action::Accept,
            no: bytecode::Action::Reject,
            condition: bytecode::Condition::SrcTuple(bytecode::TupleCondition {
                prefix_len,
                port: None,
                addr: Some(addr.to_ip_addr().into()),
            }),
        }]);
        let mut matchers = Vec::new();
        assert_matches!(matchers_from_bytecode::<I, _>(&mut matchers, &bytecode), Ok(()));
        assert_eq!(
            matchers,
            vec![fnet_sockets_ext::IpSocketMatcher::SrcAddr(
                fnet_matchers_ext::BoundAddress::Bound(fnet_matchers_ext::Address {
                    matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                        SubnetEither::from(
                            net_types::ip::Subnet::<I::Addr>::new(addr, prefix_len).unwrap()
                        )
                        .into(),
                    ),
                    invert: false,
                }),
            )]
        );
    }

    #[ip_test(I)]
    fn matchers_from_bytecode_two<I: TestIpExt>() {
        let bytecode = Bytecode(vec![
            bytecode::Instruction::Condition {
                yes: bytecode::Action::AdvanceBy(NonZeroUsize::new(1).unwrap()),
                no: bytecode::Action::Reject,
                condition: bytecode::Condition::Mark { mark: 42, mask: 0xffff },
            },
            bytecode::Instruction::Condition {
                yes: bytecode::Action::AdvanceBy(NonZeroUsize::new(1).unwrap()),
                no: bytecode::Action::Accept,
                condition: bytecode::Condition::Mark { mark: 327680, mask: 327680 },
            },
            bytecode::Instruction::Jmp(bytecode::Action::Reject),
        ]);
        let mut matchers = Vec::new();
        assert_matches!(matchers_from_bytecode::<I, _>(&mut matchers, &bytecode), Ok(()));
        assert_eq!(
            matchers,
            vec![
                fnet_sockets_ext::IpSocketMatcher::Mark(fnet_matchers_ext::MarkInDomain {
                    domain: fnet::MarkDomain::Mark1,
                    mark: fnet_matchers_ext::Mark::Marked {
                        mask: 0xffff,
                        between: 42..=42,
                        invert: false,
                    },
                }),
                fnet_sockets_ext::IpSocketMatcher::Mark(fnet_matchers_ext::MarkInDomain {
                    domain: fnet::MarkDomain::Mark1,
                    mark: fnet_matchers_ext::Mark::Marked {
                        mask: 327680,
                        between: 327680..=327680,
                        invert: true,
                    },
                })
            ]
        );
    }

    #[ip_test(I)]
    fn matchers_from_bytecode_invalid<I: TestIpExt>() {
        let program = bytecode::Bytecode(vec![bytecode::Instruction::Condition {
            yes: bytecode::Action::Accept,
            no: bytecode::Action::Reject,
            condition: bytecode::Condition::AutoPort,
        }]);

        assert_matches!(
            matchers_from_bytecode::<I, _>(&mut Vec::new(), &program),
            Err(Errno::ENOTSUP)
        );
    }
}
