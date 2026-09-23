// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FIDL Worker for the `fuchsia.net.sockets` API.

use std::collections::VecDeque;
use std::convert::Infallible as Never;
use std::sync::Arc;

use fidl::endpoints::{ProtocolMarker, Responder as _};
use fidl_fuchsia_net_sockets as fnet_sockets;
use fidl_fuchsia_net_sockets_ext as fnet_sockets_ext;
use fidl_fuchsia_net_sockets_ext::IpSocketMatcherError;
use fidl_fuchsia_net_tcp as fnet_tcp;
use fidl_fuchsia_net_udp as fnet_udp;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};
use net_types::ip::{Ip, IpVersion, Ipv4, Ipv6};
use netstack3_core::socket::{IpSocketMatcher, SocketTransportProtocolMatcher};
use netstack3_core::sync::Mutex;
use netstack3_core::tcp::{
    CongestionControlState, TcpSocketDestructionContext, TcpSocketDiagnostics, TcpSocketInfo,
    TcpSocketState,
};
use netstack3_core::udp::{UdpSocketDiagnosticTuple, UdpSocketDiagnostics};
use netstack3_core::{Instant as _, MatcherBindingsTypes, SocketDiagnosticsSeed};

use crate::bindings::time::StackTime;
use crate::bindings::util::{
    IntoCore as _, IntoFidl, IntoFidlExtender, ScopeExt as _, TryFromFidl, TryIntoFidl,
};
use crate::bindings::{BindingsCtx, Ctx};

pub(crate) async fn serve_diagnostics(
    mut stream: fnet_sockets::DiagnosticsRequestStream,
    mut ctx: Ctx,
) -> Result<(), fidl::Error> {
    log::debug!("serving {}", fnet_sockets::DiagnosticsMarker::DEBUG_NAME);
    while let Some(req) = stream.try_next().await? {
        match req {
            fidl_fuchsia_net_sockets::DiagnosticsRequest::IterateIp {
                s,
                extensions,
                matchers,
                responder,
            } => match iterate_ip(&mut ctx, extensions, matchers) {
                Ok(results) => {
                    fasync::Scope::current()
                        .spawn_request_stream_handler(s.into_stream(), |requests| {
                            serve_ipiterator(requests, results)
                        });
                    responder.send(&fnet_sockets::IterateIpResult::Ok(fnet_sockets::Empty))?
                }
                Err(err) => responder.send(&fnet_sockets::IterateIpResult::InvalidMatcher(err))?,
            },
            fidl_fuchsia_net_sockets::DiagnosticsRequest::GetDestructionWatcher {
                watcher,
                responder,
            } => {
                let dispatcher = ctx.bindings_ctx().destruction_dispatcher.clone();
                fasync::Scope::current().spawn_server_end(watcher, move |watcher| {
                    serve_watcher(watcher.into_stream(), dispatcher, responder)
                });
            }
        }
    }

    Ok(())
}

fn iterate_ip(
    ctx: &mut Ctx,
    extensions: fnet_sockets::Extensions,
    matchers: Vec<fnet_sockets::IpSocketMatcher>,
) -> Result<Vec<fnet_sockets_ext::IpSocketState>, fnet_sockets::InvalidMatcher> {
    let matchers = match convert_matchers(matchers) {
        Ok(matchers) => matchers,
        Err((err, index)) => {
            log::debug!("encountered matcher error in IterateIp request: {err}");
            return Err(fnet_sockets::InvalidMatcher {
                // Unwrap is safe because the target type is a u32, and the
                // index will never be more than MAX_IP_SOCKET_MATCHERS, which
                // is a u32.
                index: index.try_into().unwrap(),
            });
        }
    };

    let tcp_info = extensions.contains(fnet_sockets::Extensions::TCP_INFO);

    let matching = matching_families_and_protocols(&matchers);

    let mut results = IntoFidlExtender::new(Vec::new());
    if matching.tcp && matching.ipv4 {
        ctx.api().tcp::<Ipv4>().bound_sockets_diagnostics(&matchers[..], &mut results, tcp_info);
    }
    if matching.udp && matching.ipv4 {
        ctx.api().udp::<Ipv4>().bound_sockets_diagnostics(&matchers[..], &mut results);
    }
    if matching.tcp && matching.ipv6 {
        ctx.api().tcp::<Ipv6>().bound_sockets_diagnostics(&matchers[..], &mut results, tcp_info);
    }
    if matching.udp && matching.ipv6 {
        ctx.api().udp::<Ipv6>().bound_sockets_diagnostics(&matchers[..], &mut results);
    }

    Ok(results.into_inner())
}

async fn serve_ipiterator(
    mut stream: fnet_sockets::IpIteratorRequestStream,
    results: Vec<fnet_sockets_ext::IpSocketState>,
) -> Result<(), fidl::Error> {
    let results: Vec<_> = results.into_iter().map(|s| s.into()).collect();
    let mut iter = results.chunks(fnet_sockets::MAX_IP_SOCKET_BATCH_SIZE as usize).peekable();

    // TODO(https://fxbug.dev/452354359): Close the connection if the reader
    // hasn't asked for anything recently.
    while let Some(req) = stream.try_next().await? {
        match req {
            fidl_fuchsia_net_sockets::IpIteratorRequest::Next { responder } => {
                let to_send = iter.next().unwrap_or_default();
                let has_more = iter.peek().is_some();
                responder.send(to_send, has_more)?;

                if !has_more {
                    break;
                }
            }

            fidl_fuchsia_net_sockets::IpIteratorRequest::_UnknownMethod { ordinal, .. } => {
                log::warn!("Received unknown ordinal {ordinal} on IpIterator");
            }
        }
    }

    Ok(())
}

pub(crate) async fn serve_watcher(
    stream: fnet_sockets::DestructionWatcherRequestStream,
    dispatcher: SocketDestructionDispatcher,
    responder: fidl_fuchsia_net_sockets::DiagnosticsGetDestructionWatcherResponder,
) -> Result<(), fidl::Error> {
    const MAX_BATCH_SIZE: usize = fnet_sockets::MAX_IP_SOCKET_BATCH_SIZE as usize;
    const MAX_STASHED_DESTRUCTION_EVENTS: usize = MAX_BATCH_SIZE * 5;

    let (sender, mut receiver) = mpsc::unbounded();
    let id = dispatcher.add_client(sender);
    let _cleanup = scopeguard::guard((dispatcher, id), |(d, id)| {
        d.remove_client(id);
    });
    // Send the response after adding the dispatcher so the client will get all
    // notifications once the call returns.
    responder.send()?;

    let mut stashed_events = VecDeque::new();
    let mut dropped_events = 0u64;
    let mut pending_responder = None;
    let mut stream = stream.fuse();

    #[derive(Debug)]
    enum WatcherEvent {
        NewEvent(Option<fnet_sockets::IpSocketState>),
        Request(Result<Option<fnet_sockets::DestructionWatcherRequest>, fidl::Error>),
    }

    loop {
        let event = futures::select_biased! {
            event = receiver.next().fuse() => WatcherEvent::NewEvent(event),
            request = stream.try_next() => WatcherEvent::Request(request),
        };

        match event {
            WatcherEvent::Request(request) => match request? {
                Some(fnet_sockets::DestructionWatcherRequest::Watch { responder }) => {
                    if pending_responder.is_some() {
                        log::warn!("Destruction watcher id={id}: peer called Watch() concurrently");
                        responder
                            .control_handle()
                            .shutdown_with_epitaph(fidl::Status::ALREADY_EXISTS);
                        break;
                    }

                    pending_responder = Some(responder);
                }
                Some(fnet_sockets::DestructionWatcherRequest::_UnknownMethod {
                    ordinal, ..
                }) => {
                    log::warn!("Received unknown method ({ordinal}) on DestructionWatcher");
                }
                None => break,
            },
            WatcherEvent::NewEvent(event) => match event {
                Some(event) => {
                    if stashed_events.len() < MAX_STASHED_DESTRUCTION_EVENTS {
                        stashed_events.push_back(event);
                    } else {
                        log::trace!("Destruction watcher id={id}: dropped event");
                        dropped_events += 1;
                    }
                }
                None => {
                    log::info!(
                        "Destruction watcher id={id}: event channel closed. Are we shutting down?"
                    );
                    break;
                }
            },
        }

        if !stashed_events.is_empty() {
            match pending_responder.take() {
                Some(responder) => {
                    // Only grab the first set of contiguous elements to avoid an
                    // unnecessary call to VecDeque::make_contiguous. The common
                    // case is a single event. Otherwise, there are likely
                    // MAX_STASHED_DESTRUCTION_EVENTS, in which case first will
                    // probably be longer than MAX_BATCH_SIZE.
                    let (first, _rest) = stashed_events.as_slices();
                    let batch_len = std::cmp::min(first.len(), MAX_BATCH_SIZE);

                    let dropped_events = std::mem::replace(&mut dropped_events, 0);
                    responder.send(&first[..batch_len], dropped_events)?;
                    let _ = stashed_events.drain(..batch_len);
                }
                None => {}
            }
        }
    }

    Ok(())
}

#[derive(Default, Clone)]
pub(crate) struct SocketDestructionDispatcher(Arc<Mutex<SocketDestructionDispatcherInner>>);

impl SocketDestructionDispatcher {
    pub(crate) fn add_client(
        &self,
        sender: mpsc::UnboundedSender<fnet_sockets::IpSocketState>,
    ) -> u64 {
        self.0.lock().add_client(sender)
    }

    pub(crate) fn remove_client(&self, id: u64) {
        self.0.lock().remove_client(id)
    }

    pub(crate) fn notify<S>(&self, seed: S)
    where
        S: SocketDiagnosticsSeed,
        S::Output: IntoFidl<fnet_sockets_ext::IpSocketState>,
    {
        self.0.lock().notify(seed)
    }
}

#[derive(Default)]
struct SocketDestructionDispatcherInner {
    clients: Vec<WatcherSink>,
    next_id: u64,
}

struct WatcherSink {
    id: u64,
    sender: mpsc::UnboundedSender<fnet_sockets::IpSocketState>,
}

impl WatcherSink {
    fn try_send(&mut self, state: fnet_sockets::IpSocketState) {
        let Self { id, sender } = self;
        sender.unbounded_send(state).unwrap_or_else(|_| {
            log::debug!("Destruction watcher id={id}: event send raced with drop");
        });
    }
}

impl SocketDestructionDispatcherInner {
    fn add_client(&mut self, sender: mpsc::UnboundedSender<fnet_sockets::IpSocketState>) -> u64 {
        let Self { clients, next_id } = self;

        let id = *next_id;
        *next_id = next_id.checked_add(1).expect("shouldn't have u64::MAX watchers");
        clients.push(WatcherSink { id, sender });

        id
    }

    fn remove_client(&mut self, id: u64) {
        let Self { clients, next_id: _ } = self;

        if let Some(idx) = clients.iter().position(|c| c.id == id) {
            let _: WatcherSink = clients.swap_remove(idx);
        } else {
            unreachable!("watcher ID wasn't in the list")
        }
    }

    fn notify<S>(&mut self, seed: S)
    where
        S: SocketDiagnosticsSeed,
        S::Output: IntoFidl<fnet_sockets_ext::IpSocketState>,
    {
        let Self { clients, next_id: _ } = self;

        if !clients.is_empty() {
            if let Some(diag) = seed.resolve() {
                let state_ext: fnet_sockets_ext::IpSocketState = diag.into_fidl();
                let state: fnet_sockets::IpSocketState = state_ext.into();

                if let Some((last, rest)) = clients.split_last_mut() {
                    for client in rest {
                        client.try_send(state.clone());
                    }
                    last.try_send(state);
                }
            }
        }
    }
}

pub(crate) async fn serve_control(
    mut stream: fnet_sockets::ControlRequestStream,
    mut ctx: Ctx,
) -> Result<(), fidl::Error> {
    log::debug!("serving {}", fnet_sockets::ControlMarker::DEBUG_NAME);
    while let Some(req) = stream.try_next().await? {
        match req {
            fnet_sockets::ControlRequest::DisconnectIp { payload, responder } => {
                let res = disconnect_ip(&mut ctx, payload);
                responder.send(&res)?;
            }
        }
    }
    Ok(())
}

fn disconnect_ip(
    ctx: &mut Ctx,
    payload: fnet_sockets::ControlDisconnectIpRequest,
) -> fnet_sockets::DisconnectIpResult {
    let matchers = payload.matchers.unwrap_or_default();
    if matchers.is_empty() {
        return fnet_sockets::DisconnectIpResult::UnconstrainedMatchers(fnet_sockets::Empty);
    }

    let matchers = match convert_matchers(matchers) {
        Ok(matchers) => matchers,
        Err((err, index)) => {
            log::debug!("encountered matcher error in DisconnectIp request: {err}");
            return fnet_sockets::DisconnectIpResult::InvalidMatcher(
                fnet_sockets::InvalidMatcher {
                    // Unwrap is safe because the target type is a u32, and the
                    // index will never be more than MAX_IP_SOCKET_MATCHERS, which
                    // is a u32.
                    index: index.try_into().unwrap(),
                },
            );
        }
    };

    let matching = matching_families_and_protocols(&matchers);

    let mut count: usize = 0;
    if matching.tcp && matching.ipv4 {
        count += ctx.api().tcp::<Ipv4>().disconnect_bound(&matchers[..]);
    }
    if matching.udp && matching.ipv4 {
        count += ctx.api().udp::<Ipv4>().disconnect_bound(&matchers[..]);
    }
    if matching.tcp && matching.ipv6 {
        count += ctx.api().tcp::<Ipv6>().disconnect_bound(&matchers[..]);
    }
    if matching.udp && matching.ipv6 {
        count += ctx.api().udp::<Ipv6>().disconnect_bound(&matchers[..]);
    }

    fnet_sockets::DisconnectIpResult::Ok(fnet_sockets::DisconnectIpResponse {
        disconnected: count.try_into().unwrap_or(u32::MAX),
    })
}

fn convert_matchers(
    matchers: Vec<fnet_sockets::IpSocketMatcher>,
) -> Result<
    Vec<IpSocketMatcher<<BindingsCtx as MatcherBindingsTypes>::DeviceClass>>,
    (IpSocketMatcherError, usize),
> {
    matchers
        .into_iter()
        .enumerate()
        .map(|(i, matcher)| match fnet_sockets_ext::IpSocketMatcher::try_from(matcher) {
            Ok(matcher) => Ok(matcher.into_core()),
            Err(err) => Err((err, i)),
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct MatchingFamiliesAndProtocols {
    tcp: bool,
    udp: bool,
    ipv4: bool,
    ipv6: bool,
}

fn matching_families_and_protocols(
    matchers: &[IpSocketMatcher<<BindingsCtx as MatcherBindingsTypes>::DeviceClass>],
) -> MatchingFamiliesAndProtocols {
    let mut tcp = true;
    let mut udp = true;
    let mut ipv4 = true;
    let mut ipv6 = true;

    for matcher in matchers {
        match matcher {
            IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(_)) => {
                udp = false;
            }
            IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Udp(_)) => {
                tcp = false;
            }
            IpSocketMatcher::Family(IpVersion::V4) => {
                ipv6 = false;
            }
            IpSocketMatcher::Family(IpVersion::V6) => {
                ipv4 = false;
            }
            _ => {}
        }
    }

    MatchingFamiliesAndProtocols { tcp, udp, ipv4, ipv6 }
}

impl TcpSocketDestructionContext for BindingsCtx {
    fn defer_tcp_socket_destruction<I, S>(
        &self,
        result: netstack3_core::sync::RemoveResourceResultWithContext<S, Self>,
    ) where
        I: Ip,
        S: SocketDiagnosticsSeed<Output = TcpSocketDiagnostics<I, StackTime>> + Send + 'static,
    {
        match result {
            netstack3_core::sync::RemoveResourceResult::Removed(seed) => {
                self.destruction_dispatcher.notify(seed);
            }
            netstack3_core::sync::RemoveResourceResult::Deferred(receiver) => {
                let crate::bindings::reference_notifier::ReferenceReceiver {
                    receiver,
                    debug_references,
                } = receiver;
                let bindings_ctx = self.clone();

                self.resource_removal.defer_removal(
                    debug_references,
                    receiver.map(|r| r.expect("sender dropped without notifying receiver")),
                    move |seed| bindings_ctx.destruction_dispatcher.notify(seed),
                );
            }
        }
    }
}

impl TryFromFidl<fnet_sockets_ext::IpSocketMatcher>
    for IpSocketMatcher<<BindingsCtx as MatcherBindingsTypes>::DeviceClass>
{
    type Error = Never;

    fn try_from_fidl(fidl: fnet_sockets_ext::IpSocketMatcher) -> Result<Self, Self::Error> {
        match fidl {
            fnet_sockets_ext::IpSocketMatcher::Family(ip_version) => {
                Ok(IpSocketMatcher::Family(ip_version))
            }
            fnet_sockets_ext::IpSocketMatcher::SrcAddr(address) => {
                Ok(IpSocketMatcher::SrcAddr(address.into_core()))
            }
            fnet_sockets_ext::IpSocketMatcher::DstAddr(address) => {
                Ok(IpSocketMatcher::DstAddr(address.into_core()))
            }
            fnet_sockets_ext::IpSocketMatcher::Proto(socket_transport_protocol) => {
                Ok(IpSocketMatcher::Proto(socket_transport_protocol.into_core()))
            }
            fnet_sockets_ext::IpSocketMatcher::BoundInterface(bound_interface) => {
                Ok(IpSocketMatcher::BoundInterface(bound_interface.into_core()))
            }
            fnet_sockets_ext::IpSocketMatcher::Cookie(socket_cookie) => {
                Ok(IpSocketMatcher::Cookie(socket_cookie.into_core()))
            }
            fnet_sockets_ext::IpSocketMatcher::Mark(mark) => {
                Ok(IpSocketMatcher::Mark(mark.into_core()))
            }
        }
    }
}

impl<I: Ip> TryIntoFidl<fnet_sockets_ext::IpSocketState> for UdpSocketDiagnostics<I> {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fnet_sockets_ext::IpSocketState, Self::Error> {
        let UdpSocketDiagnostics { state, cookie, marks } = self;

        let state_specific = fnet_sockets_ext::IpSocketStateSpecific {
            src_addr: state.src_addr(),
            dst_addr: state.dst_addr(),
            cookie: cookie.export_value(),
            marks: marks.into_fidl(),
            transport: fnet_sockets_ext::IpSocketTransportState::Udp(
                fnet_sockets_ext::IpSocketUdpState {
                    src_port: state.src_port().map(|p| p.get()),
                    dst_port: state.dst_port(),
                    state: match state {
                        UdpSocketDiagnosticTuple::Bound { .. } => fnet_udp::State::Bound,
                        UdpSocketDiagnosticTuple::Connected { .. } => fnet_udp::State::Connected,
                    },
                },
            ),
        };

        Ok(I::map_ip_in(
            state_specific,
            |state| fnet_sockets_ext::IpSocketState::V4(state),
            |state| fnet_sockets_ext::IpSocketState::V6(state),
        ))
    }
}

impl TryIntoFidl<fnet_tcp::State> for TcpSocketState {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fnet_tcp::State, Self::Error> {
        Ok(match self {
            TcpSocketState::Established => fnet_tcp::State::Established,
            TcpSocketState::SynSent => fnet_tcp::State::SynSent,
            TcpSocketState::SynRecv => fnet_tcp::State::SynRecv,
            TcpSocketState::FinWait1 => fnet_tcp::State::FinWait1,
            TcpSocketState::FinWait2 => fnet_tcp::State::FinWait2,
            TcpSocketState::TimeWait => fnet_tcp::State::TimeWait,
            TcpSocketState::CloseWait => fnet_tcp::State::CloseWait,
            TcpSocketState::LastAck => fnet_tcp::State::LastAck,
            TcpSocketState::Closing => fnet_tcp::State::Closing,
            TcpSocketState::Listen => fnet_tcp::State::Listen,
            TcpSocketState::Close => fnet_tcp::State::Close,
        })
    }
}

impl TryIntoFidl<fnet_tcp::CongestionControlState> for CongestionControlState {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fnet_tcp::CongestionControlState, Self::Error> {
        Ok(match self {
            CongestionControlState::Open => fnet_tcp::CongestionControlState::Open,
            CongestionControlState::Disorder => fnet_tcp::CongestionControlState::Disorder,
            CongestionControlState::CongestionWindowReduced => {
                fnet_tcp::CongestionControlState::CongestionWindowReduced
            }
            CongestionControlState::Recovery => fnet_tcp::CongestionControlState::Recovery,
            CongestionControlState::Loss => fnet_tcp::CongestionControlState::Loss,
        })
    }
}

impl TryIntoFidl<fnet_sockets_ext::TcpInfo> for TcpSocketInfo<StackTime> {
    type Error = Never;
    fn try_into_fidl(self) -> Result<fnet_sockets_ext::TcpInfo, Self::Error> {
        let TcpSocketInfo {
            state,
            ca_state,
            rto,
            rtt,
            rtt_var,
            snd_ssthresh,
            snd_cwnd,
            retransmits,
            last_ack_recv,
            segs_out,
            segs_in,
            snd_mss,
            rcv_mss,
            last_data_sent,
        } = self;

        let now = StackTime::now();

        Ok(fnet_sockets_ext::TcpInfo {
            state: state.into_fidl(),
            ca_state: ca_state.into_fidl(),
            rto_usec: rto.map(|d| d.as_micros().try_into().unwrap_or(u32::MAX)),
            rtt_usec: rtt.map(|d| d.as_micros().try_into().unwrap_or(u32::MAX)),
            rtt_var_usec: rtt_var.map(|d| d.as_micros().try_into().unwrap_or(u32::MAX)),
            snd_ssthresh: snd_ssthresh,
            snd_cwnd: snd_cwnd,
            tcpi_total_retrans: retransmits.try_into().unwrap_or(u32::MAX),
            tcpi_last_ack_recv_msec: last_ack_recv.and_then(|i| {
                now.checked_duration_since(i).map(|d| d.as_millis().try_into().unwrap_or(u32::MAX))
            }),
            tcpi_segs_out: segs_out,
            tcpi_segs_in: segs_in,
            // TODO(https://fxbug.dev/404910001): Netstack2 only reports
            // reordering when using RACK, which Netstack3 doesn't support.
            reorder_seen: false,
            tcpi_last_data_sent_msec: last_data_sent.and_then(|i| {
                now.checked_duration_since(i).map(|d| d.as_millis().try_into().unwrap_or(u32::MAX))
            }),
            tcpi_snd_mss: snd_mss,
            tcpi_rcv_mss: rcv_mss,
        })
    }
}

impl<I: Ip> TryIntoFidl<fnet_sockets_ext::IpSocketState> for TcpSocketDiagnostics<I, StackTime> {
    type Error = Never;

    fn try_into_fidl(self) -> Result<fnet_sockets_ext::IpSocketState, Self::Error> {
        let TcpSocketDiagnostics { tuple, state_machine, cookie, marks, tcp_info } = self;

        let state_specific = fnet_sockets_ext::IpSocketStateSpecific {
            src_addr: tuple.src_addr(),
            dst_addr: tuple.dst_addr(),
            cookie: cookie.export_value(),
            marks: marks.into_fidl(),
            transport: fnet_sockets_ext::IpSocketTransportState::Tcp(
                fnet_sockets_ext::IpSocketTcpState {
                    src_port: tuple.src_port().map(|p| p.get()),
                    dst_port: tuple.dst_port().map(|p| p.get()),
                    state: state_machine.into_fidl(),
                    tcp_info: tcp_info.map(|i| i.into_fidl()),
                },
            ),
        };

        Ok(I::map_ip_in(
            state_specific,
            |state| fnet_sockets_ext::IpSocketState::V4(state),
            |state| fnet_sockets_ext::IpSocketState::V6(state),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use netstack3_core::socket::{
        SocketTransportProtocolMatcher, TcpSocketMatcher, UdpSocketMatcher,
    };

    #[test]
    fn test_matching_families_and_protocols() {
        assert_eq!(
            matching_families_and_protocols(&Vec::new()),
            MatchingFamiliesAndProtocols { tcp: true, udp: true, ipv4: true, ipv6: true }
        );

        let tcp = vec![IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(
            TcpSocketMatcher::Empty,
        ))];
        assert_eq!(
            matching_families_and_protocols(&tcp),
            MatchingFamiliesAndProtocols { tcp: true, udp: false, ipv4: true, ipv6: true }
        );

        let udp = vec![IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Udp(
            UdpSocketMatcher::Empty,
        ))];
        assert_eq!(
            matching_families_and_protocols(&udp),
            MatchingFamiliesAndProtocols { tcp: false, udp: true, ipv4: true, ipv6: true }
        );

        let v4 = vec![IpSocketMatcher::Family(IpVersion::V4)];
        assert_eq!(
            matching_families_and_protocols(&v4),
            MatchingFamiliesAndProtocols { tcp: true, udp: true, ipv4: true, ipv6: false }
        );

        let v6 = vec![IpSocketMatcher::Family(IpVersion::V6)];
        assert_eq!(
            matching_families_and_protocols(&v6),
            MatchingFamiliesAndProtocols { tcp: true, udp: true, ipv4: false, ipv6: true }
        );

        let tcp_v4 = vec![
            IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(TcpSocketMatcher::Empty)),
            IpSocketMatcher::Family(IpVersion::V4),
        ];
        assert_eq!(
            matching_families_and_protocols(&tcp_v4),
            MatchingFamiliesAndProtocols { tcp: true, udp: false, ipv4: true, ipv6: false }
        );

        let udp_v6 = vec![
            IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Udp(UdpSocketMatcher::Empty)),
            IpSocketMatcher::Family(IpVersion::V6),
        ];
        assert_eq!(
            matching_families_and_protocols(&udp_v6),
            MatchingFamiliesAndProtocols { tcp: false, udp: true, ipv4: false, ipv6: true }
        );

        let both_proto = vec![
            IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Tcp(TcpSocketMatcher::Empty)),
            IpSocketMatcher::Proto(SocketTransportProtocolMatcher::Udp(UdpSocketMatcher::Empty)),
        ];
        assert_eq!(
            matching_families_and_protocols(&both_proto),
            MatchingFamiliesAndProtocols { tcp: false, udp: false, ipv4: true, ipv6: true }
        );

        let both_family =
            vec![IpSocketMatcher::Family(IpVersion::V4), IpSocketMatcher::Family(IpVersion::V6)];
        assert_eq!(
            matching_families_and_protocols(&both_family),
            MatchingFamiliesAndProtocols { tcp: true, udp: true, ipv4: false, ipv6: false }
        );
    }
}
