// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use std::cell::RefCell;
use std::collections::HashSet;
use std::num::NonZeroU64;

use fidl_fuchsia_net_dhcp::{ClientExitReason, ClientProviderRequest, ClientProviderRequestStream};
use fidl_fuchsia_posix_socket_packet as fpacket;
use futures::{StreamExt as _, TryStreamExt as _};

use crate::udpsocket::LibcUdpSocketProvider;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("the interface identifier was zero")]
    InvalidInterfaceIdentifier,
    #[error("tried to create multiple DHCP clients with interface_id={0}; exiting")]
    MultipleClientsOnSameInterface(NonZeroU64),
}

struct InterfacesInUse {
    set: RefCell<HashSet<NonZeroU64>>,
}

impl InterfacesInUse {
    fn new() -> Self {
        Self { set: RefCell::new(HashSet::new()) }
    }

    fn mark_interface_in_use(
        &self,
        interface_id: NonZeroU64,
    ) -> Result<InterfaceInUseHandle<'_>, AlreadyInUse> {
        let Self { set } = self;
        if set.borrow_mut().insert(interface_id) {
            Ok(InterfaceInUseHandle { parent: self, interface_id })
        } else {
            Err(AlreadyInUse)
        }
    }

    fn remove(&self, interface_id: NonZeroU64) {
        let Self { set } = self;
        assert!(set.borrow_mut().remove(&interface_id));
    }
}

#[must_use]
struct InterfaceInUseHandle<'a> {
    parent: &'a InterfacesInUse,
    interface_id: NonZeroU64,
}

impl<'a> Drop for InterfaceInUseHandle<'a> {
    fn drop(&mut self) {
        let Self { parent, interface_id } = self;
        parent.remove(*interface_id);
    }
}

struct AlreadyInUse;

pub(crate) async fn serve_client_provider(
    stream: ClientProviderRequestStream,
    provider: fpacket::ProviderProxy,
    inspect_root: &fuchsia_inspect::Node,
) -> Result<(), Error> {
    let provider = &provider;
    let interfaces_in_use = &InterfacesInUse::new();

    stream
        .filter_map(|result| {
            futures::future::ready(result.map(Some).unwrap_or_else(|error| match error {
                fidl::Error::ClientChannelClosed { .. } => None,
                error => {
                    panic!("unexpected FIDL error in client provider request stream: {error:?}")
                }
            }))
        })
        .map(Ok)
        .try_for_each_concurrent(None, |request| async move {
            match request {
                ClientProviderRequest::NewClient {
                    interface_id,
                    params,
                    request,
                    control_handle: _,
                } => {
                    let interface_id =
                        NonZeroU64::new(interface_id).ok_or(Error::InvalidInterfaceIdentifier)?;
                    let udp_socket_provider = LibcUdpSocketProvider { interface_id };

                    let (client_requests_stream, control_handle) =
                        request.into_stream_and_control_handle();
                    let _handle: InterfaceInUseHandle<'_> = {
                        match interfaces_in_use.mark_interface_in_use(interface_id) {
                            Ok(handle) => handle,
                            Err(AlreadyInUse) => {
                                control_handle
                                    .send_on_exit(ClientExitReason::ClientAlreadyExistsOnInterface)
                                    .unwrap_or_else(|e| {
                                        log::error!(
                                            "FIDL error while sending on_exit event: {:?}",
                                            e
                                        );
                                    });
                                return Err(Error::MultipleClientsOnSameInterface(interface_id));
                            }
                        }
                    };

                    let provider = &crate::packetsocket::PacketSocketProviderImpl::new(
                        provider.clone(),
                        interface_id,
                    );

                    let mac = match provider.get_mac().await {
                        Ok(mac) => mac,
                        Err(e) => {
                            log::warn!("error while getting MAC address: {:?}", e);
                            control_handle
                                .send_on_exit({
                                    match e {
                                        dhcp_client_core::deps::SocketError::UnsupportedHardwareType
                                        | dhcp_client_core::deps::SocketError::NoInterface => {
                                            ClientExitReason::InvalidInterface
                                        }
                                        dhcp_client_core::deps::SocketError::FailedToOpen(_)
                                        | dhcp_client_core::deps::SocketError::AddrNotAvailable
                                        | dhcp_client_core::deps::SocketError::HostUnreachable
                                        | dhcp_client_core::deps::SocketError::NetworkUnreachable
                                        | dhcp_client_core::deps::SocketError::BrokenPipe
                                        | dhcp_client_core::deps::SocketError::ConnectionAborted
                                        | dhcp_client_core::deps::SocketError::Other(_) => {
                                            ClientExitReason::UnableToOpenSocket
                                        }
                                    }
                                })
                                .unwrap_or_else(|e| {
                                    log::error!("FIDL error while sending on_exit event: {:?}", e);
                                });
                            return Ok(());
                        }
                    };

                    crate::client::serve_client(
                        mac,
                        interface_id,
                        provider,
                        &udp_socket_provider,
                        params,
                        client_requests_stream,
                        inspect_root,
                    )
                    .await
                    .unwrap_or_else(|error| match error {
                        crate::client::Error::Exit(reason) => {
                            log::info!(
                                "(interface_id = {interface_id}) client exiting: {:?}",
                                reason
                            );
                            control_handle.send_on_exit(reason).unwrap_or_else(|e| {
                                log::error!("FIDL error while sending on_exit event: {:?}", e);
                            });
                        }
                        crate::client::Error::Fidl(e) => {
                            if e.is_closed() {
                                log::warn!("Channel closed while serving client: {:?}", e);
                            } else {
                                log::error!("FIDL error while serving client: {:?}", e);
                            }
                        }
                        crate::client::Error::Core(e) => {
                            log::warn!("error while serving client: {:?}", e);
                        }
                    });
                    Ok(())
                }
                ClientProviderRequest::CheckPresence { responder } => {
                    // This is a no-op method, so ignore any errors.
                    let _: Result<(), fidl::Error> = responder.send();
                    Ok(())
                }
            }
        })
        .await
}
