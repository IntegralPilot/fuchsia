// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementations of a service endpoint.

#[cfg(test)]
mod tests;

use crate::ProtocolsExt;
use crate::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use crate::execution_scope::ExecutionScope;
use crate::node::Node;
use crate::object_request::ObjectRequestRef;
#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
use crate::object_request::ObjectRequestSend;
use flex_client::AsyncChannel as Channel;
use flex_client::fidl::RequestStream;
use flex_fuchsia_io as fio;
use futures::future::Future;
use std::sync::Arc;
use zx_status::Status;

/// Objects that behave like services should implement this trait.
pub trait ServiceLike: Node {
    /// Used to establish a new connection.
    fn connect(
        &self,
        scope: ExecutionScope,
        options: ServiceOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status>;
}

#[derive(Default)]
pub struct ServiceOptions;

/// Constructs a node in your file system that will host a service that implements a statically
/// specified FIDL protocol.  `ServerRequestStream` specifies the type of the server side of this
/// protocol.
///
/// `create_server` is a callback that is invoked when a new connection to the file system node is
/// established.  The connection is reinterpreted as a `ServerRequestStream` FIDL connection and
/// passed to `create_server`.  A task produces by the `create_server` callback is execution in the
/// same [`ExecutionScope`] as the one hosting current connection.
///
/// Prefer to use this method, if the type of your FIDL protocol is statically known and you want
/// to use the connection execution scope to serve the protocol requests.  See [`endpoint`] for a
/// lower level version that gives you more flexibility.
pub fn host<ServerRequestStream, CreateServer, Task>(create_server: CreateServer) -> Arc<Service>
where
    ServerRequestStream: RequestStream,
    CreateServer: Fn(ServerRequestStream) -> Task + Send + Sync + 'static,
    Task: Future<Output = ()> + Send + 'static,
{
    endpoint(move |scope, channel| {
        let requests = RequestStream::from_channel(channel);
        let task = create_server(requests);
        // There is no way to report executor failures, and if it is failing it must be shutting
        // down.
        let _ = scope.spawn(task);
    })
}

/// Constructs a node in your file system that will host a service.
///
/// This is a lower level version of [`host`], which you should prefer if it matches your use case.
/// Unlike [`host`], `endpoint` uses a callback that will just consume the server side of the
/// channel when it is connected to the service node.  It is up to the implementer of the `open`
/// callback to decide how to interpret the channel (allowing for non-static protocol selection)
/// and/or where the processing of the messages received over the channel will occur (but the
/// [`ExecutionScope`] connected to the connection is provided every time).
pub fn endpoint<Open>(open: Open) -> Arc<Service>
where
    Open: Fn(ExecutionScope, Channel) + Send + Sync + 'static,
{
    Arc::new(Service { open: Box::new(open) })
}

/// Represents a node in the file system that hosts a service.  Opening a connection to this node
/// will switch to FIDL protocol that is different from the file system protocols, described in
/// fuchsia.io.  See there for additional details.
///
/// Use [`host`] or [`endpoint`] to construct nodes of this type.
pub struct Service {
    open: Box<dyn Fn(ExecutionScope, Channel) + Send + Sync>,
}

impl ServiceLike for Service {
    fn connect(
        &self,
        scope: ExecutionScope,
        _options: ServiceOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        #[cfg(any(
            fuchsia_api_level_at_least = "PLATFORM",
            not(fuchsia_api_level_at_least = "32")
        ))]
        let channel = if object_request.what_to_send() == ObjectRequestSend::OnOpen {
            object_request
                .take()
                .into_channel_after_sending_on_open(fio::NodeInfoDeprecated::Service(fio::Service))
                .map(|chan| crate::object_request::IntoAsyncChannel::into_async_channel(chan))
                .ok()
        } else {
            Some(crate::object_request::IntoAsyncChannel::into_async_channel(
                object_request.take().into_channel(),
            ))
        };
        #[cfg(not(any(
            fuchsia_api_level_at_least = "PLATFORM",
            not(fuchsia_api_level_at_least = "32")
        )))]
        let channel = Some(crate::object_request::IntoAsyncChannel::into_async_channel(
            object_request.take().into_channel(),
        ));

        if let Some(channel) = channel {
            (self.open)(scope, channel);
        }
        Ok(())
    }
}

impl GetEntryInfo for Service {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Service)
    }
}

impl DirectoryEntry for Service {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_service(self)
    }
}

impl Node for Service {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::CONNECTOR,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::CONNECT,
            }
        ))
    }
}

/// Helper to open a service or node as required.
pub fn serve(
    service: Arc<impl ServiceLike>,
    scope: ExecutionScope,
    protocols: &impl ProtocolsExt,
    object_request: ObjectRequestRef<'_>,
) -> Result<(), Status> {
    if protocols.is_node() {
        let options = protocols.to_node_options(service.entry_info().type_())?;
        service.open_as_node(scope, options, object_request)
    } else {
        service.connect(scope, protocols.to_service_options()?, object_request)
    }
}
