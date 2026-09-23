// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements fuchsia.posix.socket.Provider and fuchsia.posix.socket.raw.Provider.

use anyhow::{Context as _, Error};
use fidl::endpoints::{ClientEnd, ProtocolMarker, Proxy as _};
use fidl_fuchsia_net::{self as fnet, MarkDomain};
use fidl_fuchsia_posix_socket::{
    self as fposix_socket, OptionalUint32, ProviderDatagramSocketResponse,
    ProviderDatagramSocketWithOptionsResponse,
};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_inspect_derive::{IValue, Inspect, Unit};
use futures::lock::Mutex;
use futures::{Future, StreamExt as _, TryStreamExt as _};
use std::sync::Arc;
use {fidl_fuchsia_posix as fposix, fidl_fuchsia_posix_socket_raw as fposix_socket_raw};

/// Inspect node for socket provider.
///
/// Manages the following inspect nodes:
/// socket_provider:
///   sockets = 0
///   datagram:
///     proxied = 0
///     unmarked = 0
///   raw:
///     proxied = 0
///     unmarked = 0
///   stream:
///     proxied = 0
///     unmarked = 0
///   synchronous_datagram:
///     proxied = 0
///     unmarked = 0
#[derive(Unit, Default)]
struct SocketProviderInspect {
    sockets: u32,
    stream: PerType,
    synchronous_datagram: PerType,
    datagram: PerType,
    raw: PerType,
}

#[derive(Unit, Default)]
struct PerType {
    proxied: u32,
    unmarked: u32,
}

impl PerType {
    pub(crate) fn track(&mut self, marks: crate::SocketMarks) {
        self.proxied += 1;
        self.unmarked += u32::from(!marks.has_value());
    }
}

trait Markable {
    fn mark(
        &self,
        domain: MarkDomain,
        mark: OptionalUint32,
    ) -> impl Future<Output = Result<Result<(), fposix::Errno>, fidl::Error>>;
}

macro_rules! impl_markable {
    ($($ty:ty),*) => {
        $(
            impl Markable for $ty {
                fn mark(
                    &self,
                    domain: fnet::MarkDomain,
                    mark: OptionalUint32,
                ) -> impl Future<Output = Result<Result<(), fposix::Errno>, fidl::Error>> {
                    self.set_mark(domain, &mark)
                }
            }
        )*
    };
    ($($ty:ty),*,) => { impl_markable!($($ty),*); };
}

impl_markable!(
    fposix_socket::StreamSocketProxy,
    fposix_socket::SynchronousDatagramSocketProxy,
    fposix_socket::DatagramSocketProxy,
    fposix_socket_raw::SocketProxy,
);

trait Marked: Sized {
    fn marked(
        self,
        marks: crate::SocketMarks,
    ) -> impl Future<Output = Result<Result<Self, fposix::Errno>, Error>>;
}

impl<Marker> Marked for ClientEnd<Marker>
where
    Marker: ProtocolMarker,
    Marker::Proxy: fidl::endpoints::Proxy<Protocol = Marker> + Markable,
{
    fn marked(
        self,
        marks: crate::SocketMarks,
    ) -> impl Future<Output = Result<Result<Self, fposix::Errno>, Error>> {
        async move {
            let proxy = self.into_proxy();
            Ok(
                match Result::and(
                    proxy.mark(MarkDomain::Mark1, marks.mark_1).await?,
                    proxy.mark(MarkDomain::Mark2, marks.mark_2).await?,
                ) {
                    Ok(()) => Ok(proxy.into_client_end().map_err(|_| {
                        anyhow::anyhow!("Failed to convert socket proxy back into client end")
                    })?),

                    Err(e) => Err(e),
                },
            )
        }
    }
}

#[derive(Inspect, Clone)]
pub(crate) struct SocketProvider {
    marks: Arc<Mutex<crate::SocketMarks>>,

    #[inspect(forward)]
    metrics: Arc<Mutex<IValue<SocketProviderInspect>>>,
}

impl SocketProvider {
    pub(crate) fn new(mark: Arc<Mutex<crate::SocketMarks>>) -> Self {
        Self { marks: mark, metrics: Default::default() }
    }

    /// Run an instance of fuchsia.posix.socket.Provider.
    pub(crate) async fn run(
        &self,
        stream: fposix_socket::ProviderRequestStream,
    ) -> Result<(), Error> {
        let inner_provider = connect_to_protocol::<fposix_socket::ProviderMarker>()
            .context("Failed to connect to inner server")?;
        stream
            .map(|result| result.context("failed request"))
            .try_for_each(|request| async {
                match request {
                    fposix_socket::ProviderRequest::StreamSocket { domain, proto, responder } => {
                        let marks = *self.marks.lock().await;
                        let mut metrics_lock = self.metrics.lock().await;
                        let mut metrics = metrics_lock.as_mut();
                        metrics.sockets += 1;
                        metrics.stream.track(marks);
                        responder.send(
                            inner_provider
                                .stream_socket_with_options(domain, proto, fposix_socket::SocketCreationOptions {
                                    marks: Some(marks.into()),
                                    ..Default::default()
                                })
                                .await?,
                        )?;
                    }
                    fposix_socket::ProviderRequest::StreamSocketWithOptions {
                        domain,
                        proto,
                        responder,
                        opts,
                    } => {
                        let marks = *self.marks.lock().await;
                        let mut metrics_lock = self.metrics.lock().await;
                        let mut metrics = metrics_lock.as_mut();
                        metrics.sockets += 1;
                        metrics.stream.track(marks);
                        if let Some(opts_marks) = opts.marks {
                            log::warn!(
                                "stream socket marks supplied by creation opts {:?} \
                                will be overriden by {:?}",
                                opts_marks,
                                marks
                            );
                        }
                        responder.send(
                            inner_provider
                                .stream_socket_with_options(domain, proto, fposix_socket::SocketCreationOptions {
                                    marks: Some(marks.into()),
                                    ..opts
                                })
                                .await?,
                        )?;
                    }
                    fposix_socket::ProviderRequest::DatagramSocketDeprecated {
                        domain,
                        proto,
                        responder,
                    } => {
                        let marks = *self.marks.lock().await;
                        let mut metrics_lock = self.metrics.lock().await;
                        let mut metrics = metrics_lock.as_mut();
                        metrics.sockets += 1;
                        metrics.synchronous_datagram.track(marks);
                        responder.send(
                            match inner_provider.datagram_socket_deprecated(domain, proto).await? {
                                Ok(socket) => socket.marked(marks).await?,
                                e => e,
                            },
                        )?;
                    }
                    fposix_socket::ProviderRequest::DatagramSocket { domain, proto, responder } => {
                        let marks = *self.marks.lock().await;
                        let mut metrics_lock = self.metrics.lock().await;
                        let mut metrics = metrics_lock.as_mut();
                        metrics.sockets += 1;
                        let datagram_socket = inner_provider
                            .datagram_socket_with_options(domain, proto, fposix_socket::SocketCreationOptions {
                                marks: Some(marks.into()),
                                ..Default::default()
                            })
                            .await?
                            .map(|response| {
                                match response {
                                ProviderDatagramSocketWithOptionsResponse::DatagramSocket(client_end)
                                => {
                                    metrics.datagram.track(marks);
                                    ProviderDatagramSocketResponse::DatagramSocket(client_end)
                                }
                                ProviderDatagramSocketWithOptionsResponse::SynchronousDatagramSocket(
                                    client_end,
                                ) => {
                                    metrics.synchronous_datagram.track(marks);
                                    ProviderDatagramSocketResponse::SynchronousDatagramSocket(
                                    client_end,
                                    )
                                },
                            }
                            });
                        responder.send(datagram_socket)?
                    }
                    fposix_socket::ProviderRequest::DatagramSocketWithOptions {
                        domain,
                        proto,
                        opts,
                        responder,
                    } => {
                        let marks = *self.marks.lock().await;
                        let mut metrics_lock = self.metrics.lock().await;
                        let mut metrics = metrics_lock.as_mut();
                        metrics.sockets += 1;
                        if let Some(opts_marks) = opts.marks {
                            log::warn!(
                                "datagram marks supplied by creation opts {:?} \
                                will be overriden by {:?}",
                                opts_marks,
                                marks
                            );
                        }
                        let datagram_socket = inner_provider
                                .datagram_socket_with_options(domain, proto, fposix_socket::SocketCreationOptions {
                                    marks: Some(marks.into()),
                                    ..opts
                                })
                                .await?
                            .map(|response| {
                                match response {
                                ProviderDatagramSocketWithOptionsResponse::DatagramSocket(_)
                                => {
                                    metrics.datagram.track(marks);
                                }
                                ProviderDatagramSocketWithOptionsResponse::SynchronousDatagramSocket(_) => {
                                    metrics.synchronous_datagram.track(marks);
                                },
                                };
                                response
                            });
                        responder.send(datagram_socket)?
                    }
                    fposix_socket::ProviderRequest::InterfaceIndexToName { index, responder } => {
                        let name = inner_provider.interface_index_to_name(index).await?;
                        responder.send(match &name {
                            Ok(n) => Ok(n),
                            Err(e) => Err(*e),
                        })?;
                    }
                    fposix_socket::ProviderRequest::InterfaceNameToIndex { name, responder } => {
                        responder.send(inner_provider.interface_name_to_index(&name).await?)?
                    }
                    fposix_socket::ProviderRequest::InterfaceNameToFlags { name, responder } => {
                        responder.send(inner_provider.interface_name_to_flags(&name).await?)?
                    }
                    fposix_socket::ProviderRequest::GetInterfaceAddresses { responder } => {
                        responder.send(&inner_provider.get_interface_addresses().await?)?
                    }
                }

                Ok(())
            })
            .await
    }

    /// Run an instance of fuchsia.posix.socket.raw.Provider.
    pub(crate) async fn run_raw(
        &self,
        stream: fposix_socket_raw::ProviderRequestStream,
    ) -> Result<(), Error> {
        let inner_provider = connect_to_protocol::<fposix_socket_raw::ProviderMarker>()
            .context("Failed to connect to inner server")?;
        stream
            .map(|result| result.context("failed request"))
            .try_for_each(|request| async {
                match request {
                    fposix_socket_raw::ProviderRequest::Socket { domain, proto, responder } => {
                        let marks = *self.marks.lock().await;
                        let mut metrics_lock = self.metrics.lock().await;
                        let mut metrics = metrics_lock.as_mut();
                        metrics.sockets += 1;
                        metrics.raw.track(marks);
                        responder.send(
                            inner_provider
                                .socket_with_options(
                                    domain,
                                    &proto,
                                    fposix_socket::SocketCreationOptions {
                                        marks: Some(marks.into()),
                                        ..Default::default()
                                    },
                                )
                                .await?,
                        )?
                    }
                    fposix_socket_raw::ProviderRequest::SocketWithOptions {
                        domain,
                        proto,
                        opts,
                        responder,
                    } => {
                        let marks = *self.marks.lock().await;
                        let mut metrics_lock = self.metrics.lock().await;
                        let mut metrics = metrics_lock.as_mut();
                        metrics.sockets += 1;
                        metrics.raw.track(marks);
                        if let Some(opts_marks) = opts.marks {
                            log::warn!(
                                "raw socket marks supplied by creation opts {:?} \
                                will be overriden by {:?}",
                                opts_marks,
                                marks
                            );
                        }
                        responder.send(
                            inner_provider
                                .socket_with_options(
                                    domain,
                                    &proto,
                                    fposix_socket::SocketCreationOptions {
                                        marks: Some(marks.into()),
                                        ..opts
                                    },
                                )
                                .await?,
                        )?
                    }
                }

                Ok(())
            })
            .await
    }
}
