// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{NetlinkSockDiag, NetlinkSockDiagNotifiedGroup};

use std::convert::Infallible as Never;

use derivative::Derivative;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_sockets as fnet_sockets;
use fidl_fuchsia_net_sockets_ext as fnet_sockets_ext;
use fidl_fuchsia_net_tcp as fnet_tcp;
use fidl_fuchsia_net_udp as fnet_udp;
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt as _, StreamExt as _, pin_mut};
use linux_uapi::{AF_INET, AF_INET6};
use net_types::ip::{Ip, IpAddress as _};
use netlink_packet_core::{NLM_F_MULTIPART, NetlinkMessage};
use netlink_packet_sock_diag::inet::nlas::{Nla, TcpInfo};
use netlink_packet_sock_diag::inet::{InetResponse, InetResponseHeader};
use netlink_packet_sock_diag::{
    SockDiagResponse, TCP_CA_CWR, TCP_CA_DISORDER, TCP_CA_LOSS, TCP_CA_OPEN, TCP_CA_RECOVERY,
    TCP_CLOSE, TCP_CLOSE_WAIT, TCP_CLOSING, TCP_ESTABLISHED, TCP_FIN_WAIT1, TCP_FIN_WAIT2,
    TCP_LAST_ACK, TCP_LISTEN, TCP_SYN_RECV, TCP_SYN_SENT, TCP_TIME_WAIT,
};

use crate::client::{AsyncWorkItem, ClientTable, InternalClient};
use crate::logging::{log_debug, log_error, log_warn};
use crate::messaging::Sender;
use crate::netlink_packet::errno::Errno;
use crate::protocol_family::ProtocolFamily;

/// The argument(s) for a [`Request`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RequestArgs {
    Get(Vec<fnet_sockets_ext::IpSocketMatcher>, fnet_sockets::Extensions, bool),
    Destroy(Vec<fnet_sockets_ext::IpSocketMatcher>),
}

/// An error encountered while handling a [`Request`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RequestError {
    NotFound,
    Internal,
}

impl RequestError {
    pub(crate) fn into_errno(self) -> Errno {
        match self {
            RequestError::NotFound => Errno::ENOENT,
            RequestError::Internal => Errno::EINVAL,
        }
    }
}

/// A `NETLINK_SOCK_DIAG` request.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub(crate) struct Request<S: Sender<<NetlinkSockDiag as ProtocolFamily>::Response>> {
    /// The operation-specific arguments for this request.
    pub args: RequestArgs,
    /// The request's sequence number.
    ///
    /// This value will be copied verbatim into any message sent as a result of
    /// this request.
    pub sequence_number: u32,
    /// The client that made the request.
    pub client: InternalClient<NetlinkSockDiag, S>,
    /// A completer that will have the result of the request sent over.
    pub completer: oneshot::Sender<Result<(), RequestError>>,
}

pub(crate) struct SockDiagEventLoop<
    S: crate::messaging::Sender<<NetlinkSockDiag as ProtocolFamily>::Response>,
> {
    socket_diagnostics: fnet_sockets::DiagnosticsProxy,
    socket_control: fnet_sockets::ControlProxy,
    request_stream: mpsc::Receiver<Request<S>>,
    async_work_receiver: mpsc::UnboundedReceiver<AsyncWorkItem<NetlinkSockDiag>>,
    client_table: ClientTable<NetlinkSockDiag, S>,

    tcp_v4_clients: u64,
    tcp_v6_clients: u64,
    udp_v4_clients: u64,
    udp_v6_clients: u64,
    // This watcher is Some whenever there are any clients and None otherwise.
    destruction_watcher_stream: Option<
        futures::stream::BoxStream<
            'static,
            Result<fnet_sockets_ext::IpSocketState, fnet_sockets_ext::DestructionWatcherError>,
        >,
    >,
}

impl<S: crate::messaging::Sender<<NetlinkSockDiag as ProtocolFamily>::Response>>
    SockDiagEventLoop<S>
{
    pub(crate) fn new(
        socket_diagnostics: fnet_sockets::DiagnosticsProxy,
        socket_control: fnet_sockets::ControlProxy,
        request_stream: mpsc::Receiver<Request<S>>,
        async_work_receiver: mpsc::UnboundedReceiver<AsyncWorkItem<NetlinkSockDiag>>,
        client_table: ClientTable<NetlinkSockDiag, S>,
    ) -> Self {
        Self {
            socket_diagnostics,
            socket_control,
            request_stream,
            async_work_receiver,
            client_table,
            tcp_v4_clients: 0,
            tcp_v6_clients: 0,
            udp_v4_clients: 0,
            udp_v6_clients: 0,
            destruction_watcher_stream: None,
        }
    }

    fn clients_count(&self, group: NetlinkSockDiagNotifiedGroup) -> u64 {
        match group {
            NetlinkSockDiagNotifiedGroup::TcpV4Destroy => self.tcp_v4_clients,
            NetlinkSockDiagNotifiedGroup::TcpV6Destroy => self.tcp_v6_clients,
            NetlinkSockDiagNotifiedGroup::UdpV4Destroy => self.udp_v4_clients,
            NetlinkSockDiagNotifiedGroup::UdpV6Destroy => self.udp_v6_clients,
        }
    }

    fn clients_count_mut(&mut self, group: NetlinkSockDiagNotifiedGroup) -> &mut u64 {
        match group {
            NetlinkSockDiagNotifiedGroup::TcpV4Destroy => &mut self.tcp_v4_clients,
            NetlinkSockDiagNotifiedGroup::TcpV6Destroy => &mut self.tcp_v6_clients,
            NetlinkSockDiagNotifiedGroup::UdpV4Destroy => &mut self.udp_v4_clients,
            NetlinkSockDiagNotifiedGroup::UdpV6Destroy => &mut self.udp_v6_clients,
        }
    }

    fn has_destruction_clients(&self) -> bool {
        self.tcp_v4_clients > 0
            || self.tcp_v6_clients > 0
            || self.udp_v4_clients > 0
            || self.udp_v6_clients > 0
    }

    async fn handle_join(&mut self, group: NetlinkSockDiagNotifiedGroup) {
        let clients = self.clients_count_mut(group);
        *clients = clients.checked_add(1).expect("number of clients can't overflow a u64");

        if self.destruction_watcher_stream.is_none() {
            match fnet_sockets_ext::watch_destruction(&self.socket_diagnostics).await {
                Ok(stream) => {
                    self.destruction_watcher_stream = Some(stream.boxed());
                }
                Err(e) => {
                    panic!("failed to get destruction watcher: {:?}", e);
                }
            }
        }
    }

    fn handle_leave(&mut self, group: NetlinkSockDiagNotifiedGroup) {
        let clients = self.clients_count_mut(group);
        *clients = clients.checked_sub(1).expect("each added client is removed once");

        if !self.has_destruction_clients() {
            self.destruction_watcher_stream = None;
        }
    }

    async fn handle_async_work(&mut self, work: AsyncWorkItem<NetlinkSockDiag>) {
        match work {
            AsyncWorkItem::OnJoinMulticastGroup(group, sender) => {
                self.handle_join(group).await;
                let _ = sender.send(());
            }
            AsyncWorkItem::OnLeaveMulticastGroup(groups) => {
                for group in groups.into_iter() {
                    self.handle_leave(group);
                }
            }
            AsyncWorkItem::OnSetMulticastGroups { joined, left, complete } => {
                // Process joined groups before left groups to avoid possibly
                // passing through 0, which would drop the destruction watcher.
                if let Some(groups) = joined {
                    for group in groups.into_iter() {
                        self.handle_join(group).await;
                    }
                }
                if let Some(groups) = left {
                    for group in groups.into_iter() {
                        self.handle_leave(group);
                    }
                }
                if let Some(complete) = complete {
                    let _ = complete.send(());
                }
            }
            AsyncWorkItem::Inner(never) => match never {},
        }
    }

    fn handle_destruction_event(
        &mut self,
        event: Option<
            Result<fnet_sockets_ext::IpSocketState, fnet_sockets_ext::DestructionWatcherError>,
        >,
    ) {
        match event {
            Some(Ok(socket)) => {
                let group = NetlinkSockDiagNotifiedGroup::from_socket_state(&socket);
                if self.clients_count(group) > 0 {
                    let mut msg: NetlinkMessage<SockDiagResponse> =
                        ip_socket_to_netlink_response(socket).into();
                    msg.finalize();
                    self.client_table.send_message_to_group(msg, group.into());
                }
            }
            Some(Err(fnet_sockets_ext::DestructionWatcherError::DroppedEvents(num_dropped))) => {
                log_warn!("Missing {num_dropped} socket destruction events from the netstack.");
            }
            Some(Err(e)) => {
                log_error!("unexpected socket destruction watcher error: {e:?}");
            }
            None => {
                // This is only likely to happen if the netstack has
                // crashed. The whole system is about to go down anyway, so it's
                // not worth trying to recover.
                log_error!("destruction watcher stream ended unexpectedly");
            }
        }
    }

    pub(crate) async fn run(mut self) -> Never {
        loop {
            self.run_one_step().await;
        }
    }

    async fn run_one_step(&mut self) {
        let destruction_stream_next = async {
            if let Some(stream) = &mut self.destruction_watcher_stream {
                stream.next().await
            } else {
                futures::future::pending().await
            }
        };
        pin_mut!(destruction_stream_next);

        futures::select! {
            async_work = self.async_work_receiver.select_next_some() => {
                self.handle_async_work(async_work).await;
            }
            event = destruction_stream_next.fuse() => {
                self.handle_destruction_event(event);
            }
            request = self.request_stream.select_next_some() => {
                handle_request(&mut self.socket_diagnostics, &mut self.socket_control, request)
                    .await;
            }
        };
    }
}

async fn handle_request<S>(
    socket_diagnostics: &mut fidl_fuchsia_net_sockets::DiagnosticsProxy,
    socket_control: &mut fidl_fuchsia_net_sockets::ControlProxy,
    mut request: Request<S>,
) where
    S: crate::messaging::Sender<<NetlinkSockDiag as ProtocolFamily>::Response>,
{
    match request.args {
        RequestArgs::Get(matchers, extensions, is_dump) => {
            log_debug!(
                "Calling iterate_ip with matchers: {:?}, extensions: {:?}, is_dump: {}",
                matchers,
                extensions,
                is_dump
            );

            let stream = match fnet_sockets_ext::iterate_ip(
                socket_diagnostics,
                extensions,
                matchers,
            )
            .await
            {
                Ok(stream) => stream,
                Err(e) => {
                    log_error!("iterate_ip error: {e:?}");
                    request
                        .completer
                        .send(Err(RequestError::Internal))
                        .expect("receiving end of completer should not be dropped");
                    return;
                }
            };

            pin_mut!(stream);

            let mut found = false;
            while let Some(socket) = stream.next().await {
                match socket {
                    Ok(socket) => {
                        found = true;

                        let mut msg: NetlinkMessage<SockDiagResponse> =
                            ip_socket_to_netlink_response(socket).into();
                        msg.header.sequence_number = request.sequence_number;
                        if is_dump {
                            msg.header.flags |= NLM_F_MULTIPART;
                        }
                        msg.finalize();
                        request.client.send_unicast(msg);

                        // Non-dump requests on Linux return only the
                        // first socket, even if more would match (e.g.
                        // SO_REUSEPORT with a wildcard cookie).
                        if !is_dump {
                            break;
                        }
                    }

                    Err(e) => {
                        log_error!("socket stream error: {e:?}");
                        request
                            .completer
                            .send(Err(RequestError::Internal))
                            .expect("receiving end of completer should not be dropped");
                        return;
                    }
                }
            }

            let result = if !is_dump && !found { Err(RequestError::NotFound) } else { Ok(()) };

            request
                .completer
                .send(result)
                .expect("receiving end of completer should not be dropped");
        }
        RequestArgs::Destroy(matchers) => {
            log_debug!("Calling disconnect_ip with matchers: {:?}", matchers);
            let result = match fnet_sockets_ext::disconnect_ip(socket_control, matchers).await {
                Ok(disconnected) => {
                    if disconnected > 0 {
                        Ok(())
                    } else {
                        Err(RequestError::NotFound)
                    }
                }
                Err(e) => {
                    log_error!("disconnect_ip error: {e:?}");
                    Err(RequestError::Internal)
                }
            };
            request
                .completer
                .send(result)
                .expect("receiving end of completer should not be dropped");
        }
    }
}

/// Convert the FIDL socket into a netlink response.
///
/// Returns `None` if any of the required fields are not set. Fills any
/// unsupported fields with the maximum supported value to hopefully make it
/// more obvious if something in userspace is depending on it.
fn ip_socket_to_netlink_response(socket: fnet_sockets_ext::IpSocketState) -> SockDiagResponse {
    match socket {
        fnet_sockets_ext::IpSocketState::V4(state) => ip_socket_to_netlink_response_inner(state),
        fnet_sockets_ext::IpSocketState::V6(state) => ip_socket_to_netlink_response_inner(state),
    }
}

fn ip_socket_to_netlink_response_inner<I: Ip>(
    socket: fnet_sockets_ext::IpSocketStateSpecific<I>,
) -> SockDiagResponse {
    let fnet_sockets_ext::IpSocketStateSpecific { src_addr, dst_addr, cookie, marks, transport } =
        socket;

    let mut nlas = Vec::new();
    let (socket_id, state) = match transport {
        fnet_sockets_ext::IpSocketTransportState::Tcp(fnet_sockets_ext::IpSocketTcpState {
            src_port,
            dst_port,
            state,
            tcp_info,
        }) => {
            if let Some(info) = tcp_info {
                nlas.push(Nla::TcpInfo(convert_tcp_info(info)));
            }
            (
                make_socket_id::<I>(src_port, dst_port, src_addr, dst_addr, cookie),
                tcp_state_fidl_to_linux(state),
            )
        }
        fnet_sockets_ext::IpSocketTransportState::Udp(fnet_sockets_ext::IpSocketUdpState {
            src_port,
            dst_port,
            state,
        }) => (
            make_socket_id::<I>(src_port, dst_port, src_addr, dst_addr, cookie),
            match state {
                fnet_udp::State::Bound => TCP_CLOSE,
                fnet_udp::State::Connected => TCP_ESTABLISHED,
            },
        ),
    };

    if let Some(mark) = marks.get(fnet::MARK_DOMAIN_SO_MARK) {
        nlas.push(Nla::Mark(mark));
    }
    let uid = marks.get(fnet::MARK_DOMAIN_SOCKET_UID).unwrap_or(u32::MAX);

    let resp = InetResponse {
        header: InetResponseHeader {
            family: I::map_ip((), |()| AF_INET as u8, |()| AF_INET6 as u8),
            state,
            timer: None,
            socket_id,
            recv_queue: u32::MAX,
            send_queue: u32::MAX,
            uid,
            inode: u32::MAX,
        },
        nlas: nlas.into(),
    };

    SockDiagResponse::InetResponse(Box::new(resp))
}

fn make_socket_id<I: Ip>(
    src_port: Option<u16>,
    dst_port: Option<u16>,
    src_addr: Option<I::Addr>,
    dst_addr: Option<I::Addr>,
    cookie: u64,
) -> netlink_packet_sock_diag::inet::SocketId {
    netlink_packet_sock_diag::inet::SocketId {
        // Ports and address are allowed to be unset.
        source_port: src_port.unwrap_or(0),
        destination_port: dst_port.unwrap_or(0),
        source_address: src_addr.unwrap_or(I::UNSPECIFIED_ADDRESS).to_ip_addr().into(),
        destination_address: dst_addr.unwrap_or(I::UNSPECIFIED_ADDRESS).to_ip_addr().into(),
        interface_id: u32::MAX,
        cookie: cookie.to_ne_bytes(),
    }
}

fn tcp_state_fidl_to_linux(state: fnet_tcp::State) -> u8 {
    match state {
        fnet_tcp::State::Established => TCP_ESTABLISHED,
        fnet_tcp::State::SynSent => TCP_SYN_SENT,
        fnet_tcp::State::SynRecv => TCP_SYN_RECV,
        fnet_tcp::State::FinWait1 => TCP_FIN_WAIT1,
        fnet_tcp::State::FinWait2 => TCP_FIN_WAIT2,
        fnet_tcp::State::TimeWait => TCP_TIME_WAIT,
        fnet_tcp::State::Close => TCP_CLOSE,
        fnet_tcp::State::CloseWait => TCP_CLOSE_WAIT,
        fnet_tcp::State::LastAck => TCP_LAST_ACK,
        fnet_tcp::State::Listen => TCP_LISTEN,
        fnet_tcp::State::Closing => TCP_CLOSING,
    }
}

fn ca_state_fidl_to_linux(ca_state: fnet_tcp::CongestionControlState) -> u8 {
    match ca_state {
        fnet_tcp::CongestionControlState::Open => TCP_CA_OPEN,
        fnet_tcp::CongestionControlState::Disorder => TCP_CA_DISORDER,
        fnet_tcp::CongestionControlState::CongestionWindowReduced => TCP_CA_CWR,
        fnet_tcp::CongestionControlState::Recovery => TCP_CA_RECOVERY,
        fnet_tcp::CongestionControlState::Loss => TCP_CA_LOSS,
    }
}

fn convert_tcp_info(info: fnet_sockets_ext::TcpInfo) -> TcpInfo {
    let fnet_sockets_ext::TcpInfo {
        state,
        ca_state,
        rto_usec,
        tcpi_last_data_sent_msec,
        tcpi_last_ack_recv_msec,
        rtt_usec,
        rtt_var_usec,
        snd_ssthresh,
        snd_cwnd,
        tcpi_total_retrans,
        tcpi_segs_out,
        tcpi_segs_in,
        reorder_seen,
        tcpi_snd_mss,
        tcpi_rcv_mss,
    } = info;

    TcpInfo {
        state: tcp_state_fidl_to_linux(state),
        ca_state: ca_state_fidl_to_linux(ca_state),
        rto: rto_usec.unwrap_or(0),
        last_data_sent: tcpi_last_data_sent_msec.unwrap_or(u32::MAX),
        last_ack_recv: tcpi_last_ack_recv_msec.unwrap_or(u32::MAX),
        rtt: rtt_usec.unwrap_or(0),
        rttvar: rtt_var_usec.unwrap_or(0),
        snd_ssthresh,
        snd_cwnd,
        total_retrans: tcpi_total_retrans,
        segs_out: tcpi_segs_out.try_into().unwrap_or(u32::MAX),
        segs_in: tcpi_segs_in.try_into().unwrap_or(u32::MAX),
        // TODO(https://fxrev.dev/434682660): reorder_seen should be a u32.
        // TODO(https://fxbug.dev/404910001): Netstack2 only reports reordering
        // when using RACK, which Netstack3 doesn't support.
        reord_seen: if reorder_seen { 1 } else { 0 },
        snd_mss: tcpi_snd_mss.unwrap_or(0),
        rcv_mss: tcpi_rcv_mss.unwrap_or(0),

        // Unsupported fields are set to MAX values.
        retransmits: u8::MAX,
        probes: u8::MAX,
        backoff: u8::MAX,
        options: u8::MAX,
        wscale: u8::MAX,
        delivery_rate_app_limited: u8::MAX,
        ato: u32::MAX,
        unacked: u32::MAX,
        sacked: u32::MAX,
        lost: u32::MAX,
        retrans: u32::MAX,
        fackets: u32::MAX,
        last_ack_sent: u32::MAX,
        last_data_recv: u32::MAX,
        pmtu: u32::MAX,
        rcv_ssthresh: u32::MAX,
        advmss: u32::MAX,
        reordering: u32::MAX,
        rcv_rtt: u32::MAX,
        rcv_space: u32::MAX,
        pacing_rate: u64::MAX,
        max_pacing_rate: u64::MAX,
        bytes_acked: u64::MAX,
        bytes_received: u64::MAX,
        notsent_bytes: u32::MAX,
        min_rtt: u32::MAX,
        data_segs_in: u32::MAX,
        data_segs_out: u32::MAX,
        delivery_rate: u64::MAX,
        busy_time: u64::MAX,
        rwnd_limited: u64::MAX,
        sndbuf_limited: u64::MAX,
        delivered: u32::MAX,
        delivered_ce: u32::MAX,
        bytes_sent: u64::MAX,
        bytes_retrans: u64::MAX,
        dsack_dups: u32::MAX,
        rcv_ooopack: u32::MAX,
        snd_wnd: u32::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::pin::pin;

    use fidl_fuchsia_net_ext::IntoExt as _;
    use fuchsia_async as fasync;
    use futures::{SinkExt as _, future};
    use ip_test_macro::ip_test;
    use net_declare::fidl_ip;

    use crate::client::ClientId;
    use crate::logging::testutils::set_logger_for_test;
    use crate::messaging::testutil::SentMessage;
    use crate::protocol_family::sock_diag::testutil::TestIpExt;

    const TEST_SEQUENCE_NUMBER: u32 = 1234;

    async fn fake_iterate_ip(
        stream: fnet_sockets::DiagnosticsRequestStream,
        sockets: Vec<fnet_sockets::IpSocketState>,
    ) {
        let mut stream = stream;
        let request = stream.next().await.expect("request should succeed").unwrap();
        let (iterator, responder) = match request {
            fnet_sockets::DiagnosticsRequest::IterateIp {
                s,
                extensions: _,
                matchers: _,
                responder,
            } => (s, responder),
            fnet_sockets::DiagnosticsRequest::GetDestructionWatcher { .. } => unreachable!(),
        };

        let mut stream = iterator.into_stream();
        responder
            .send(&fnet_sockets::IterateIpResult::Ok(fnet_sockets::Empty))
            .expect("send result");

        let request = stream.next().await.expect("request should succeed").unwrap();
        let responder = match request {
            fnet_sockets::IpIteratorRequest::Next { responder } => responder,
            _ => panic!("unexpected request"),
        };
        responder.send(&sockets, false).unwrap();
    }

    async fn run_request_test(
        args: RequestArgs,
        sockets: Vec<fnet_sockets::IpSocketState>,
        cb: impl FnOnce(Result<Vec<SentMessage<SockDiagResponse>>, RequestError>),
    ) {
        let scope = fasync::Scope::new();

        let (diagnostics_proxy, diagnostics_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_sockets::DiagnosticsMarker>();
        let (control_proxy, _control_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_sockets::ControlMarker>();
        let (mut request_sink, request_stream) = mpsc::channel(1);
        let (_async_work_sink, async_work_receiver) = mpsc::unbounded();

        let event_loop = SockDiagEventLoop::new(
            diagnostics_proxy,
            control_proxy,
            request_stream,
            async_work_receiver,
            ClientTable::default(),
        );

        let (mut client_sink, client, async_work_drain_task) =
            crate::client::testutil::new_fake_client::<NetlinkSockDiag>(
                crate::client::testutil::CLIENT_ID_1,
                [],
            );
        let _async_work_drain_task = scope.spawn(async_work_drain_task);

        let (completer, completer_receiver) = oneshot::channel();
        let request = Request { args, sequence_number: TEST_SEQUENCE_NUMBER, client, completer };

        let work_fut = {
            let sockets = sockets.clone();
            async {
                let request_fut = async { request_sink.send(request).await.unwrap() };
                let iterate_ip_fut = fake_iterate_ip(diagnostics_request_stream, sockets);
                let completer_fut = async { completer_receiver.await.unwrap() };

                let ((), (), completer_result) =
                    future::join3(request_fut, iterate_ip_fut, completer_fut).await;
                completer_result.map(|()| client_sink.take_messages())
            }
        };

        match future::select(pin!(work_fut), pin!(event_loop.run())).await {
            future::Either::Left((res, _)) => cb(res),
            future::Either::Right(_) => unreachable!("eventloop does not complete"),
        }

        scope.join().await;
    }

    /// Tests the success case of getting a single socket as well as
    /// "netstack returned two sockets but we only return one".
    #[fuchsia::test(logging = false)]
    async fn get_one_success() {
        set_logger_for_test();

        let socket_1 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let socket_2 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(4321),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        run_request_test(
            RequestArgs::Get(vec![], fnet_sockets::Extensions::empty(), false),
            vec![socket_1.clone(), socket_2.clone()],
            |res| {
                let messages = res.unwrap();
                assert_eq!(messages.len(), 1);
                let msg = &messages[0].message;
                assert_eq!(msg.header.sequence_number, TEST_SEQUENCE_NUMBER);

                let expected_payload = ip_socket_to_netlink_response(socket_1.try_into().unwrap());
                assert_eq!(msg.payload, NetlinkMessage::from(expected_payload).payload);
            },
        )
        .await;
    }

    #[fuchsia::test(logging = false)]
    async fn get_bad_socket() {
        set_logger_for_test();

        let socket = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            // Cookie should always be set, so this will cause the socket to be skipped.
            cookie: None,
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        run_request_test(
            RequestArgs::Get(vec![], fnet_sockets::Extensions::empty(), false),
            vec![socket.clone()],
            |res| assert_eq!(res, Err(RequestError::Internal)),
        )
        .await;
    }

    #[fuchsia::test(logging = false)]
    async fn dump_no_sockets_success() {
        set_logger_for_test();

        run_request_test(
            RequestArgs::Get(vec![], fnet_sockets::Extensions::empty(), true),
            vec![],
            |res| {
                assert_eq!(res, Ok(vec![]));
            },
        )
        .await;
    }

    #[fuchsia::test(logging = false)]
    async fn dump_success() {
        set_logger_for_test();

        let socket_1 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1234),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let socket_2 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(4321),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        run_request_test(
            RequestArgs::Get(vec![], fnet_sockets::Extensions::empty(), true),
            vec![socket_1.clone(), socket_2.clone()],
            |res| {
                let messages = res.unwrap();
                assert_eq!(messages.len(), 2);

                let msg = &messages[0].message;
                assert_eq!(msg.header.sequence_number, TEST_SEQUENCE_NUMBER);
                let expected_payload = ip_socket_to_netlink_response(socket_1.try_into().unwrap());
                assert_eq!(msg.payload, NetlinkMessage::from(expected_payload).payload);

                let msg = &messages[1].message;
                assert_eq!(msg.header.sequence_number, TEST_SEQUENCE_NUMBER);
                let expected_payload = ip_socket_to_netlink_response(socket_2.try_into().unwrap());
                assert_eq!(msg.payload, NetlinkMessage::from(expected_payload).payload);
            },
        )
        .await;
    }

    #[ip_test(I)]
    fn ip_socket_to_netlink_response_tcp<I: TestIpExt>() {
        let state = fnet_sockets::IpSocketState {
            family: Some(I::VERSION.into_ext()),
            src_addr: Some(I::SRC_ADDR.to_ip_addr().into_ext()),
            dst_addr: Some(I::DST_ADDR.to_ip_addr().into_ext()),
            cookie: Some(123),
            marks: Some(fnet::Marks {
                mark_1: Some(0x11111111),
                mark_2: Some(0x22222222),
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1234),
                    dst_port: Some(5678),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let mut response: NetlinkMessage<SockDiagResponse> =
            ip_socket_to_netlink_response(state.try_into().unwrap()).into();
        response.finalize();

        let payload = InetResponse {
            header: InetResponseHeader {
                family: match I::VERSION {
                    net_types::ip::IpVersion::V4 => AF_INET,
                    net_types::ip::IpVersion::V6 => AF_INET6,
                } as u8,
                state: TCP_ESTABLISHED,
                timer: None,
                socket_id: netlink_packet_sock_diag::inet::SocketId {
                    source_port: 1234,
                    destination_port: 5678,
                    source_address: I::SRC_ADDR.to_ip_addr().into(),
                    destination_address: I::DST_ADDR.to_ip_addr().into(),
                    interface_id: u32::MAX,
                    cookie: 123u64.to_ne_bytes(),
                },
                recv_queue: u32::MAX,
                send_queue: u32::MAX,
                uid: 0x22222222,
                inode: u32::MAX,
            },
            nlas: vec![Nla::Mark(0x11111111)].into(),
        };
        let mut expected: NetlinkMessage<SockDiagResponse> =
            SockDiagResponse::InetResponse(Box::new(payload)).into();
        expected.finalize();

        assert_eq!(response, expected);
    }

    #[ip_test(I)]
    fn ip_socket_to_netlink_response_tcp_with_info<I: TestIpExt>() {
        let state = fnet_sockets::IpSocketState {
            family: Some(I::VERSION.into_ext()),
            src_addr: Some(I::SRC_ADDR.to_ip_addr().into_ext()),
            dst_addr: Some(I::DST_ADDR.to_ip_addr().into_ext()),
            cookie: Some(123),
            marks: Some(fnet::Marks {
                mark_1: Some(0x11111111),
                mark_2: Some(0x22222222),
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1234),
                    dst_port: Some(5678),
                    state: Some(fnet_tcp::State::Established),
                    tcp_info: Some(fnet_tcp::Info {
                        state: Some(fnet_tcp::State::Established),
                        ca_state: Some(fnet_tcp::CongestionControlState::Open),
                        rto_usec: Some(100),
                        tcpi_last_data_sent_msec: Some(200),
                        tcpi_last_ack_recv_msec: Some(300),
                        rtt_usec: Some(400),
                        rtt_var_usec: Some(500),
                        snd_ssthresh: Some(600),
                        snd_cwnd: Some(700),
                        tcpi_total_retrans: Some(800),
                        tcpi_segs_out: Some(900),
                        tcpi_segs_in: Some(1000),
                        reorder_seen: Some(true),
                        tcpi_rcv_mss: Some(128),
                        tcpi_snd_mss: Some(256),
                        __source_breaking: fidl::marker::SourceBreaking,
                    }),
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let mut response: NetlinkMessage<SockDiagResponse> =
            ip_socket_to_netlink_response(state.try_into().unwrap()).into();
        response.finalize();

        let payload = InetResponse {
            header: InetResponseHeader {
                family: match I::VERSION {
                    net_types::ip::IpVersion::V4 => AF_INET,
                    net_types::ip::IpVersion::V6 => AF_INET6,
                } as u8,
                state: TCP_ESTABLISHED,
                timer: None,
                socket_id: netlink_packet_sock_diag::inet::SocketId {
                    source_port: 1234,
                    destination_port: 5678,
                    source_address: I::SRC_ADDR.to_ip_addr().into(),
                    destination_address: I::DST_ADDR.to_ip_addr().into(),
                    interface_id: u32::MAX,
                    cookie: 123u64.to_ne_bytes(),
                },
                recv_queue: u32::MAX,
                send_queue: u32::MAX,
                uid: 0x22222222,
                inode: u32::MAX,
            },
            nlas: vec![
                Nla::TcpInfo(TcpInfo {
                    state: TCP_ESTABLISHED,
                    ca_state: TCP_CA_OPEN,
                    retransmits: u8::MAX,
                    probes: u8::MAX,
                    backoff: u8::MAX,
                    options: u8::MAX,
                    wscale: u8::MAX,
                    delivery_rate_app_limited: u8::MAX,
                    rto: 100,
                    ato: u32::MAX,
                    rcv_mss: 128,
                    snd_mss: 256,
                    unacked: u32::MAX,
                    sacked: u32::MAX,
                    lost: u32::MAX,
                    retrans: u32::MAX,
                    fackets: u32::MAX,
                    last_data_sent: 200,
                    last_ack_sent: u32::MAX,
                    last_data_recv: u32::MAX,
                    last_ack_recv: 300,
                    pmtu: u32::MAX,
                    rcv_ssthresh: u32::MAX,
                    rtt: 400,
                    rttvar: 500,
                    snd_ssthresh: 600,
                    snd_cwnd: 700,
                    advmss: u32::MAX,
                    reordering: u32::MAX,
                    rcv_rtt: u32::MAX,
                    rcv_space: u32::MAX,
                    total_retrans: 800,
                    pacing_rate: u64::MAX,
                    max_pacing_rate: u64::MAX,
                    bytes_acked: u64::MAX,
                    bytes_received: u64::MAX,
                    segs_out: 900,
                    segs_in: 1000,
                    notsent_bytes: u32::MAX,
                    min_rtt: u32::MAX,
                    data_segs_in: u32::MAX,
                    data_segs_out: u32::MAX,
                    delivery_rate: u64::MAX,
                    busy_time: u64::MAX,
                    rwnd_limited: u64::MAX,
                    sndbuf_limited: u64::MAX,
                    delivered: u32::MAX,
                    delivered_ce: u32::MAX,
                    bytes_sent: u64::MAX,
                    bytes_retrans: u64::MAX,
                    dsack_dups: u32::MAX,
                    reord_seen: 1,
                    rcv_ooopack: u32::MAX,
                    snd_wnd: u32::MAX,
                }),
                Nla::Mark(0x11111111),
            ]
            .into(),
        };
        let mut expected: NetlinkMessage<SockDiagResponse> =
            SockDiagResponse::InetResponse(Box::new(payload)).into();
        expected.finalize();

        assert_eq!(response, expected);
    }

    #[ip_test(I)]
    fn ip_socket_to_netlink_response_udp<I: TestIpExt>() {
        let state = fnet_sockets::IpSocketState {
            family: Some(I::VERSION.into_ext()),
            src_addr: Some(I::SRC_ADDR.to_ip_addr().into_ext()),
            dst_addr: Some(I::DST_ADDR.to_ip_addr().into_ext()),
            cookie: Some(456),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Udp(
                fnet_sockets::IpSocketUdpState {
                    src_port: Some(4321),
                    dst_port: Some(8765),

                    state: Some(fnet_udp::State::Bound),
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };

        let mut response: NetlinkMessage<SockDiagResponse> =
            ip_socket_to_netlink_response(state.try_into().unwrap()).into();
        response.finalize();

        let payload = InetResponse {
            header: InetResponseHeader {
                family: match I::VERSION {
                    net_types::ip::IpVersion::V4 => AF_INET,
                    net_types::ip::IpVersion::V6 => AF_INET6,
                } as u8,
                state: TCP_CLOSE,
                timer: None,
                socket_id: netlink_packet_sock_diag::inet::SocketId {
                    source_port: 4321,
                    destination_port: 8765,
                    source_address: I::SRC_ADDR.to_ip_addr().into(),
                    destination_address: I::DST_ADDR.to_ip_addr().into(),
                    interface_id: u32::MAX,
                    cookie: 456u64.to_ne_bytes(),
                },
                recv_queue: u32::MAX,
                send_queue: u32::MAX,
                uid: u32::MAX,
                inode: u32::MAX,
            },
            nlas: vec![].into(),
        };
        let mut expected: NetlinkMessage<SockDiagResponse> =
            SockDiagResponse::InetResponse(Box::new(payload)).into();
        expected.finalize();

        assert_eq!(response, expected);
    }

    // Validates that destruction notifications from the Netstack are delivered
    // to the correct group and only that group.
    #[fuchsia::test(logging = false)]
    async fn destruction_notification_routing() {
        set_logger_for_test();

        let (diagnostics_proxy, mut diagnostics_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_sockets::DiagnosticsMarker>();
        let (control_proxy, _control_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_sockets::ControlMarker>();
        let (_request_sink, request_stream) = mpsc::channel(1);
        let (async_work_sink, async_work_receiver) = mpsc::unbounded();

        let client_table = ClientTable::<NetlinkSockDiag, _>::default();
        let mut event_loop = SockDiagEventLoop::new(
            diagnostics_proxy,
            control_proxy,
            request_stream,
            async_work_receiver,
            client_table.clone(),
        );

        let (mock_watcher_tx, mut mock_watcher_rx) = mpsc::unbounded();

        let mock_diagnostics_fut = async move {
            while let Some(req) = diagnostics_request_stream.next().await {
                match req.unwrap() {
                    fnet_sockets::DiagnosticsRequest::GetDestructionWatcher {
                        watcher,
                        responder,
                    } => {
                        let stream = watcher.into_stream();
                        responder.send().unwrap();
                        let _ = mock_watcher_tx.unbounded_send(stream);
                    }
                    req => panic!("unexpected diagnostics request: {:?}", req),
                }
            }
        };

        let scope = fasync::Scope::new();
        let _diagnostics_task = scope.spawn(mock_diagnostics_fut);

        async fn set_up_client(
            client_id: ClientId,
            group: NetlinkSockDiagNotifiedGroup,
            client_table: &ClientTable<
                NetlinkSockDiag,
                crate::messaging::testutil::FakeSender<SockDiagResponse>,
            >,
            async_work_sink: &mpsc::UnboundedSender<AsyncWorkItem<NetlinkSockDiag>>,
            event_loop: &mut SockDiagEventLoop<
                crate::messaging::testutil::FakeSender<SockDiagResponse>,
            >,
        ) -> crate::messaging::testutil::FakeSenderSink<SockDiagResponse> {
            let (sender, sink) = crate::messaging::testutil::fake_sender_with_sink();
            let (ext_client, int_client) =
                crate::client::new_client_pair(client_id, sender, async_work_sink.clone());
            client_table.add_client(int_client);
            let waiter = ext_client.add_membership(group.into()).unwrap();

            event_loop.run_one_step().await;

            waiter.wait_until_complete();
            sink
        }

        let (watcher_stream_opt, mut sink_tcp_v4) = future::join(
            mock_watcher_rx.next(),
            set_up_client(
                crate::client::testutil::CLIENT_ID_1,
                NetlinkSockDiagNotifiedGroup::TcpV4Destroy,
                &client_table,
                &async_work_sink,
                &mut event_loop,
            ),
        )
        .await;
        let mut watcher_stream = watcher_stream_opt.unwrap();

        let mut sink_tcp_v6 = set_up_client(
            crate::client::testutil::CLIENT_ID_2,
            NetlinkSockDiagNotifiedGroup::TcpV6Destroy,
            &client_table,
            &async_work_sink,
            &mut event_loop,
        )
        .await;

        let mut sink_udp_v4 = set_up_client(
            crate::client::testutil::CLIENT_ID_3,
            NetlinkSockDiagNotifiedGroup::UdpV4Destroy,
            &client_table,
            &async_work_sink,
            &mut event_loop,
        )
        .await;

        let mut sink_udp_v6 = set_up_client(
            crate::client::testutil::CLIENT_ID_4,
            NetlinkSockDiagNotifiedGroup::UdpV6Destroy,
            &client_table,
            &async_work_sink,
            &mut event_loop,
        )
        .await;

        async fn inject_and_verify(
            event_loop: &mut SockDiagEventLoop<
                crate::messaging::testutil::FakeSender<SockDiagResponse>,
            >,
            watcher_stream: &mut fnet_sockets::DestructionWatcherRequestStream,
            sink_tcp_v4: &mut crate::messaging::testutil::FakeSenderSink<SockDiagResponse>,
            sink_tcp_v6: &mut crate::messaging::testutil::FakeSenderSink<SockDiagResponse>,
            sink_udp_v4: &mut crate::messaging::testutil::FakeSenderSink<SockDiagResponse>,
            sink_udp_v6: &mut crate::messaging::testutil::FakeSenderSink<SockDiagResponse>,
            socket_state: &fnet_sockets::IpSocketState,
            expected_group: NetlinkSockDiagNotifiedGroup,
            dropped_events: u64,
        ) {
            {
                let run_step = event_loop.run_one_step();
                let next_req = watcher_stream.next();
                pin_mut!(run_step, next_req);
                let (watch_req, event_loop_fut) = match future::select(next_req, run_step).await {
                    future::Either::Left((req, fut)) => (req.unwrap().unwrap(), fut),
                    future::Either::Right(_) => unreachable!("event loop should not finish first"),
                };
                let responder = match watch_req {
                    fnet_sockets::DestructionWatcherRequest::Watch { responder } => responder,
                    fnet_sockets::DestructionWatcherRequest::_UnknownMethod { .. } => {
                        panic!("unknown method request");
                    }
                };
                responder.send(&[socket_state.clone()], dropped_events).unwrap();

                event_loop_fut.await;
            }

            // We log and keep going if events are dropped.
            if dropped_events > 0 {
                event_loop.run_one_step().await;
            }

            let msg_tcp_v4 = sink_tcp_v4.take_messages();
            let msg_tcp_v6 = sink_tcp_v6.take_messages();
            let msg_udp_v4 = sink_udp_v4.take_messages();
            let msg_udp_v6 = sink_udp_v6.take_messages();

            let expected_payload =
                ip_socket_to_netlink_response(socket_state.clone().try_into().unwrap());
            let expected_raw_payload = NetlinkMessage::from(expected_payload).payload;

            let check = |messages: Vec<SentMessage<SockDiagResponse>>, is_expected: bool| {
                if is_expected {
                    assert_eq!(messages.len(), 1);
                    assert_eq!(messages[0].message.payload, expected_raw_payload);
                } else {
                    assert_eq!(messages.len(), 0);
                }
            };

            check(msg_tcp_v4, expected_group == NetlinkSockDiagNotifiedGroup::TcpV4Destroy);
            check(msg_tcp_v6, expected_group == NetlinkSockDiagNotifiedGroup::TcpV6Destroy);
            check(msg_udp_v4, expected_group == NetlinkSockDiagNotifiedGroup::UdpV4Destroy);
            check(msg_udp_v6, expected_group == NetlinkSockDiagNotifiedGroup::UdpV6Destroy);
        }

        let socket_tcp_v4 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(1),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Close),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };
        inject_and_verify(
            &mut event_loop,
            &mut watcher_stream,
            &mut sink_tcp_v4,
            &mut sink_tcp_v6,
            &mut sink_udp_v4,
            &mut sink_udp_v6,
            &socket_tcp_v4,
            NetlinkSockDiagNotifiedGroup::TcpV4Destroy,
            0,
        )
        .await;

        let socket_tcp_v6 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V6),
            src_addr: Some(fidl_ip!("fe80::1")),
            dst_addr: Some(fidl_ip!("fe80::2")),
            cookie: Some(2),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Tcp(
                fnet_sockets::IpSocketTcpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_tcp::State::Close),
                    tcp_info: None,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };
        inject_and_verify(
            &mut event_loop,
            &mut watcher_stream,
            &mut sink_tcp_v4,
            &mut sink_tcp_v6,
            &mut sink_udp_v4,
            &mut sink_udp_v6,
            &socket_tcp_v6,
            NetlinkSockDiagNotifiedGroup::TcpV6Destroy,
            10,
        )
        .await;

        let socket_udp_v4 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V4),
            src_addr: Some(fidl_ip!("192.168.1.1")),
            dst_addr: Some(fidl_ip!("192.168.1.2")),
            cookie: Some(3),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Udp(
                fnet_sockets::IpSocketUdpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_udp::State::Bound),
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };
        inject_and_verify(
            &mut event_loop,
            &mut watcher_stream,
            &mut sink_tcp_v4,
            &mut sink_tcp_v6,
            &mut sink_udp_v4,
            &mut sink_udp_v6,
            &socket_udp_v4,
            NetlinkSockDiagNotifiedGroup::UdpV4Destroy,
            0,
        )
        .await;

        let socket_udp_v6 = fnet_sockets::IpSocketState {
            family: Some(fnet::IpVersion::V6),
            src_addr: Some(fidl_ip!("fe80::1")),
            dst_addr: Some(fidl_ip!("fe80::2")),
            cookie: Some(4),
            marks: Some(fnet::Marks {
                mark_1: None,
                mark_2: None,
                __source_breaking: fidl::marker::SourceBreaking,
            }),
            transport: Some(fnet_sockets::IpSocketTransportState::Udp(
                fnet_sockets::IpSocketUdpState {
                    src_port: Some(1111),
                    dst_port: Some(2222),
                    state: Some(fnet_udp::State::Bound),
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )),
            __source_breaking: fidl::marker::SourceBreaking,
        };
        inject_and_verify(
            &mut event_loop,
            &mut watcher_stream,
            &mut sink_tcp_v4,
            &mut sink_tcp_v6,
            &mut sink_udp_v4,
            &mut sink_udp_v6,
            &socket_udp_v6,
            NetlinkSockDiagNotifiedGroup::UdpV6Destroy,
            10,
        )
        .await;

        drop(event_loop);
        scope.join().await;
    }

    #[fuchsia::test(logging = false)]
    async fn test_watcher_stream_lifecycle() {
        set_logger_for_test();

        let (diagnostics_proxy, mut diagnostics_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_sockets::DiagnosticsMarker>();
        let (control_proxy, _control_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_sockets::ControlMarker>();
        let (_request_sink, request_stream) = mpsc::channel(1);
        let (async_work_sink, async_work_receiver) = mpsc::unbounded();

        let client_table = ClientTable::<NetlinkSockDiag, _>::default();
        let mut event_loop = SockDiagEventLoop::new(
            diagnostics_proxy,
            control_proxy,
            request_stream,
            async_work_receiver,
            client_table.clone(),
        );

        let (watcher_tx, mut watcher_rx) = mpsc::unbounded();

        let mock_diagnostics_fut = async move {
            while let Some(req) = diagnostics_request_stream.next().await {
                match req.unwrap() {
                    fnet_sockets::DiagnosticsRequest::GetDestructionWatcher {
                        watcher,
                        responder,
                    } => {
                        let stream = watcher.into_stream();
                        responder.send().unwrap();
                        let _ = watcher_tx.unbounded_send(stream);
                    }
                    req => panic!("unexpected diagnostics request: {:?}", req),
                }
            }
        };

        let scope = fasync::Scope::new();
        let _diagnostics_task = scope.spawn(mock_diagnostics_fut);

        // Client 1: TCP V4
        let (sender_1, _sink_1) = crate::messaging::testutil::fake_sender_with_sink();
        let (ext_client_1, int_client_1) = crate::client::new_client_pair(
            crate::client::testutil::CLIENT_ID_1,
            sender_1,
            async_work_sink.clone(),
        );
        client_table.add_client(int_client_1);

        // Client 2: UDP V4
        let (sender_2, _sink_2) = crate::messaging::testutil::fake_sender_with_sink();
        let (ext_client_2, int_client_2) = crate::client::new_client_pair(
            crate::client::testutil::CLIENT_ID_2,
            sender_2,
            async_work_sink.clone(),
        );
        client_table.add_client(int_client_2);

        assert!(event_loop.destruction_watcher_stream.is_none());

        // Connect client 1 -> Destruction stream created.
        let w1 =
            ext_client_1.add_membership(NetlinkSockDiagNotifiedGroup::TcpV4Destroy.into()).unwrap();
        let run_join = async {
            event_loop.run_one_step().await;
        };
        let (watcher_stream_1_opt, ()) = future::join(watcher_rx.next(), run_join).await;
        let _watcher_stream_1 = watcher_stream_1_opt.unwrap();
        w1.wait_until_complete();

        assert!(event_loop.destruction_watcher_stream.is_some());

        // Connect client 2 -> No change to destruction stream.
        let w2 =
            ext_client_2.add_membership(NetlinkSockDiagNotifiedGroup::UdpV4Destroy.into()).unwrap();
        event_loop.run_one_step().await;
        w2.wait_until_complete();
        assert!(event_loop.destruction_watcher_stream.is_some());
        // No new watcher request was made
        assert!(watcher_rx.try_next().is_err());

        // Disconnect client 1 -> No change to destruction stream.
        ext_client_1.del_membership(NetlinkSockDiagNotifiedGroup::TcpV4Destroy.into()).unwrap();
        event_loop.run_one_step().await;

        assert!(event_loop.destruction_watcher_stream.is_some());

        // Disconnect client 2 -> Destruction stream dropped because no clients.
        ext_client_2.del_membership(NetlinkSockDiagNotifiedGroup::UdpV4Destroy.into()).unwrap();
        event_loop.run_one_step().await;

        assert!(event_loop.destruction_watcher_stream.is_none());

        // Connect client 1 again -> Destruction stream recreated.
        let w3 =
            ext_client_1.add_membership(NetlinkSockDiagNotifiedGroup::TcpV4Destroy.into()).unwrap();
        let run_join_again = async {
            event_loop.run_one_step().await;
        };
        let (_watcher_stream_2_opt, ()) = future::join(watcher_rx.next(), run_join_again).await;
        w3.wait_until_complete();
        assert!(event_loop.destruction_watcher_stream.is_some());

        drop(event_loop);
        scope.join().await;
    }
}
