// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A Netstack3 worker to serve fuchsia.net.debug.Interfaces API requests.

use fidl::endpoints::{ProtocolMarker as _, ServerEnd};
use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net_debug as fnet_debug;
use fuchsia_async::LocalExecutorBuilder;
use futures::{StreamExt as _, TryStreamExt as _};
use log::{error, warn};

use crate::bindings::devices::BindingId;
use crate::bindings::{DeviceIdExt as _, DeviceSpecificInfo};

// Serve a stream of fuchsia.net.debug.Interfaces API requests for a single
// channel (e.g. a single client connection).
pub(crate) async fn serve_interfaces(
    ctx: crate::bindings::Ctx,
    rs: fnet_debug::InterfacesRequestStream,
) -> Result<(), fidl::Error> {
    rs.try_for_each(|req| {
        let ctx = ctx.clone();
        async move {
            match req {
                fnet_debug::InterfacesRequest::GetPort { id, port, control_handle: _ } => {
                    handle_get_port(ctx.bindings_ctx(), id, port);
                    Ok(())
                }
                fnet_debug::InterfacesRequest::CloseBackingSession { id, responder } => {
                    handle_close_backing_session(ctx, id, responder).await
                }
            }
        }
    })
    .await
}

async fn handle_close_backing_session(
    ctx: crate::bindings::Ctx,
    interface_id: u64,
    responder: fnet_debug::InterfacesCloseBackingSessionResponder,
) -> Result<(), fidl::Error> {
    let binding_id = BindingId::new(interface_id);
    let result = if let Some(binding_id) = binding_id {
        let core_id = ctx.bindings_ctx().devices.get_core_id(binding_id);
        if let Some(core_id) = core_id {
            let port_handler = match core_id.external_state() {
                DeviceSpecificInfo::Ethernet(info) => Ok(&info.netdevice.handler),
                DeviceSpecificInfo::PureIp(info) => Ok(&info.netdevice.handler),
                DeviceSpecificInfo::Loopback(_) | DeviceSpecificInfo::Blackhole(_) => {
                    Err(fnet_debug::CloseSessionError::NotSupported)
                }
            };
            match port_handler {
                Ok(handler) => match handler.close_session().await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        warn!("failed to close session: {:?}", e);
                        Err(fnet_debug::CloseSessionError::NotSupported)
                    }
                },
                Err(e) => Err(e),
            }
        } else {
            Err(fnet_debug::CloseSessionError::InterfaceNotFound)
        }
    } else {
        Err(fnet_debug::CloseSessionError::InterfaceNotFound)
    };
    let _ = responder.send(result);
    Ok(())
}

fn handle_get_port(
    bindings_ctx: &crate::bindings::BindingsCtx,
    interface_id: u64,
    port: ServerEnd<fhardware_network::PortMarker>,
) {
    let core_id = BindingId::new(interface_id).and_then(|id| bindings_ctx.devices.get_core_id(id));
    let port_handler =
        core_id.as_ref().ok_or(zx::Status::NOT_FOUND).map(|core_id| core_id.external_state());
    let port_handler = port_handler.as_ref().map_err(Clone::clone).and_then(|state| match state {
        DeviceSpecificInfo::Loopback(_) => Err(zx::Status::NOT_SUPPORTED),
        DeviceSpecificInfo::Blackhole(_) => Err(zx::Status::NOT_SUPPORTED),
        DeviceSpecificInfo::Ethernet(info) => Ok(&info.netdevice.handler),
        DeviceSpecificInfo::PureIp(info) => Ok(&info.netdevice.handler),
    });
    match port_handler {
        Ok(port_handler) => port_handler
            .connect_port(port)
            .unwrap_or_else(|e: netdevice_client::Error| warn!("failed to connect to port: {e:?}")),
        Err(epitaph) => {
            port.close_with_epitaph(epitaph)
                .unwrap_or_else(|e| warn!("failed to send epitaph: {e:?}"));
        }
    }
}

struct DiagnosticsInner {
    thread: std::thread::JoinHandle<()>,
    sender: futures::channel::mpsc::UnboundedSender<ServerEnd<fnet_debug::DiagnosticsMarker>>,
}

impl DiagnosticsInner {
    fn new() -> Self {
        let (sender, mut receiver) =
            futures::channel::mpsc::unbounded::<ServerEnd<fnet_debug::DiagnosticsMarker>>();
        let thread = std::thread::spawn(move || {
            let mut executor = LocalExecutorBuilder::new().build();
            let fut = async move {
                let mut futures = futures::stream::FuturesUnordered::new();
                loop {
                    let result = futures::select! {
                        s = receiver.next() => s,
                        n = futures.next() => {
                            // We don't care if FuturesUnordered
                            // finished, we might push more data into it
                            // later.
                            n.unwrap_or(());
                            continue
                        },
                    };
                    match result {
                        Some(rs) => {
                            futures
                                .push(DiagnosticsHandler::serve_request_stream(rs.into_stream()));
                        }
                        None => {
                            // When the receiver ends we want to stop
                            // serving all streams.
                            break;
                        }
                    }
                }
            };
            executor.run_singlethreaded(fut)
        });
        Self { sender, thread }
    }
}

/// Offers a server implementation of `fuchsia.net.debug/Diagnostics` that
/// serves all requests in a dedicated thread, so diagnostics can be provided
/// even if the main stack's executor is blocked.
#[derive(Default)]
pub(crate) struct DiagnosticsHandler {
    inner: once_cell::sync::OnceCell<DiagnosticsInner>,
}

impl Drop for DiagnosticsHandler {
    fn drop(&mut self) {
        let Self { inner } = self;
        if let Some(DiagnosticsInner { thread, sender }) = inner.take() {
            // Drop the sender so the receiver side on the worker thread will
            // terminate.
            std::mem::drop(sender);
            thread.join().expect("failed to join diagnostics thread");
        }
    }
}

impl DiagnosticsHandler {
    pub(crate) fn serve_diagnostics(&self, server_end: ServerEnd<fnet_debug::DiagnosticsMarker>) {
        let Self { inner } = self;
        let DiagnosticsInner { sender, thread: _ } = inner.get_or_init(DiagnosticsInner::new);
        sender.unbounded_send(server_end).expect("sender was orphaned unexpectedly");
    }

    async fn serve_request_stream(rs: fnet_debug::DiagnosticsRequestStream) {
        rs.try_for_each(|req| {
            futures::future::ready(match req {
                fnet_debug::DiagnosticsRequest::LogDebugInfoToSyslog { responder } => {
                    warn!(
                        "Requesting stack trace to logs as requested by {}, this is not a crash.",
                        fnet_debug::DiagnosticsMarker::DEBUG_NAME
                    );
                    debug::backtrace_request_all_threads();
                    responder.send()
                }
                fnet_debug::DiagnosticsRequest::GetProcessHandleForInspection { responder } => {
                    let process = fuchsia_runtime::process_self()
                        .duplicate_handle(zx::Rights::INSPECT | zx::Rights::TRANSFER)
                        .expect("duplicate process handle");
                    responder.send(process)
                }
            })
        })
        .await
        .unwrap_or_else(|e: fidl::Error| error!("error operating diagnostics stream: {e:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::DiagnosticsHandler;

    use test_case::test_case;

    // DiagnosticsHandler has a nontrivial Drop path that isn't really exercised
    // in integration tests. This gets us some coverage.
    #[test_case(0; "empty")]
    #[test_case(3; "multiple")]
    #[fuchsia::test(logging = false)]
    async fn drop_diagnostics_handler(streams: usize) {
        let handler = DiagnosticsHandler::default();

        // Attach channels.
        let channels = {
            let handler = &handler;
            core::iter::repeat_with(|| fidl::endpoints::create_endpoints())
                .map(|(client, server)| {
                    handler.serve_diagnostics(server);
                    client.into_channel()
                })
                .take(streams)
                .collect::<Vec<_>>()
        };

        // Dropping the handler should stop the alternative executor and join
        // the thread.
        std::mem::drop(handler);

        for channel in channels {
            let signals =
                fuchsia_async::OnSignals::new(&channel, zx::Signals::CHANNEL_PEER_CLOSED).await;
            assert_eq!(signals, Ok(zx::Signals::CHANNEL_PEER_CLOSED));
        }
    }
}
