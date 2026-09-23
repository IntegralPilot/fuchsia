// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of a (limited) node connection.

use crate::common::IntoAny;
use crate::directory::entry::GetEntryInfo;
use crate::directory::entry_container::MutableDirectory;
use crate::execution_scope::ExecutionScope;
use crate::name::Name;
use crate::object_request::{ConnectionCreator, Representation, run_synchronous_future_or_spawn};
use crate::protocols::ToNodeOptions;
use crate::request_handler::{RequestHandler, RequestListener};
use crate::{ObjectRequest, ObjectRequestRef};
use anyhow::Error;
use flex_client::fidl::{DiscoverableProtocolMarker as _, ServerEnd};
use flex_fuchsia_io as fio;
use libc::{S_IRUSR, S_IWUSR};
use std::future::{Future, ready};
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use storage_trace::{self as trace, TraceFutureExt};
use zx_status::Status;

/// POSIX emulation layer access attributes for all services created with service().
#[cfg(not(target_os = "macos"))]
pub const POSIX_READ_WRITE_PROTECTION_ATTRIBUTES: u32 = S_IRUSR | S_IWUSR;
#[cfg(target_os = "macos")]
pub const POSIX_READ_WRITE_PROTECTION_ATTRIBUTES: u16 = S_IRUSR | S_IWUSR;

#[derive(Clone, Copy)]
pub struct NodeOptions {
    pub rights: fio::Operations,
}

impl From<&NodeOptions> for fio::Flags {
    fn from(options: &NodeOptions) -> Self {
        // There is 1:1 mapping between `fio::Operations` and `fio::Flags`.
        fio::Flags::PROTOCOL_NODE | fio::Flags::from_bits_truncate(options.rights.bits())
    }
}

/// All nodes must implement this trait.
pub trait Node: GetEntryInfo + IntoAny + Send + Sync + 'static {
    /// Returns node attributes (io2).
    fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> impl Future<Output = Result<fio::NodeAttributes2, Status>> + Send
    where
        Self: Sized;

    /// Called when the node is about to be opened as the node protocol.  Implementers can use this
    /// to perform any initialization or reference counting.  Errors here will result in the open
    /// failing.  By default, this forwards to the infallible will_clone.
    fn will_open_as_node(&self) -> Result<(), Status> {
        self.will_clone();
        Ok(())
    }

    /// Called when the node is about to be cloned (and also by the default implementation of
    /// will_open_as_node).  Implementations that perform their own open count can use this.  Each
    /// call to `will_clone` will be accompanied by an eventual call to `close`.
    fn will_clone(&self) {}

    /// Called when the node is closed.
    fn close(self: Arc<Self>) {}

    fn link_into(
        self: Arc<Self>,
        _destination_dir: Arc<dyn MutableDirectory>,
        _name: Name,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Returns information about the filesystem and/or volume.
    fn query_filesystem(&self) -> Result<fio::FilesystemInfo, Status> {
        Err(Status::NOT_SUPPORTED)
    }

    /// Opens the node using the node protocol.
    fn open_as_node(
        self: Arc<Self>,
        scope: ExecutionScope,
        options: NodeOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status>
    where
        Self: Sized,
    {
        self.will_open_as_node()?;
        Connection::create_sync(scope, self, options, object_request.take());
        Ok(())
    }

    /// List extended attributes.
    fn list_extended_attributes(&self) -> impl Future<Output = Result<Vec<Vec<u8>>, Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Get the value for an extended attribute.
    fn get_extended_attribute(
        &self,
        _name: Vec<u8>,
    ) -> impl Future<Output = Result<Vec<u8>, Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Set the value for an extended attribute.
    fn set_extended_attribute(
        &self,
        _name: Vec<u8>,
        _value: Vec<u8>,
        _mode: fio::SetExtendedAttributeMode,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }

    /// Remove the value for an extended attribute.
    fn remove_extended_attribute(
        &self,
        _name: Vec<u8>,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }
}

/// Represents a FIDL (limited) node connection.
pub struct Connection<N: Node> {
    // Execution scope this connection and any async operations and connections it creates will
    // use.
    scope: ExecutionScope,

    // The underlying node.
    node: OpenNode<N>,

    // Node options.
    options: NodeOptions,
}

/// Return type for [`handle_request()`] functions.
enum ConnectionState {
    /// Connection is still alive.
    Alive,
    /// Connection have received Node::Close message, it was dropped by the peer, or an error had
    /// occurred.  As we do not perform any actions, except for closing our end we do not
    /// distinguish those cases, unlike file and directory connections.
    Closed,
}

impl<N: Node> Connection<N> {
    /// Creates a new connection to serve the node. The node will be served from a new async `Task`,
    /// not from the current `Task`. Errors in constructing the connection are not guaranteed to be
    /// returned, they may be sent directly to the client end of the connection. This method should
    /// be called from within an `ObjectRequest` handler to ensure that errors are sent to the
    /// client end of the connection.
    pub async fn create(
        scope: ExecutionScope,
        node: Arc<N>,
        options: impl ToNodeOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        let node = OpenNode::new(node);
        let options = options.to_node_options(node.entry_info().type_())?;
        let connection = Connection { scope: scope.clone(), node, options };
        if let Ok(requests) = object_request.take().into_request_stream(&connection).await {
            scope.spawn(RequestListener::new(requests, connection));
        }
        Ok(())
    }

    /// Similar to `create` but optimized for nodes whose implementation is synchronous and creating
    /// the connection is being done from a non-async context.
    pub fn create_sync(
        scope: ExecutionScope,
        node: Arc<N>,
        options: impl ToNodeOptions,
        object_request: ObjectRequest,
    ) {
        run_synchronous_future_or_spawn(
            scope.clone(),
            object_request.handle_async(async |object_request| {
                Self::create(scope, node, options, object_request).await
            }),
        )
    }

    /// Handle a [`NodeRequest`].
    async fn handle_request(&mut self, req: fio::NodeRequest) -> Result<ConnectionState, Error> {
        match req {
            #[cfg(any(
                fuchsia_api_level_at_least = "PLATFORM",
                not(fuchsia_api_level_at_least = "29")
            ))]
            fio::NodeRequest::DeprecatedClone { flags, object, control_handle: _ } => {
                trace::duration!("storage", "Node::Clone");
                crate::common::send_on_open_with_error(
                    flags.contains(fio::OpenFlags::DESCRIBE),
                    object,
                    Status::NOT_SUPPORTED,
                );
            }
            fio::NodeRequest::Clone { request, control_handle: _ } => {
                trace::duration!("storage", "Node::Clone");
                // Suppress any errors in the event a bad `request` channel was provided.
                self.handle_clone(ServerEnd::new(request.into_channel()));
            }
            fio::NodeRequest::Close { responder } => {
                trace::duration!("storage", "Node::Close");
                responder.send(Ok(()))?;
                return Ok(ConnectionState::Closed);
            }
            fio::NodeRequest::Sync { responder } => {
                trace::duration!("storage", "Node::Sync");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            #[cfg(fuchsia_api_level_at_least = "28")]
            fio::NodeRequest::DeprecatedGetAttr { responder } => {
                async move {
                    let (status, attrs) =
                        crate::common::io2_to_io1_attrs(self.node.as_ref(), self.options.rights)
                            .await;
                    responder.send(status, &attrs)
                }
                .trace(trace::trace_future_args!("storage", "Node::GetAttr"))
                .await?;
            }
            #[cfg(not(fuchsia_api_level_at_least = "28"))]
            fio::NodeRequest::GetAttr { responder } => {
                async move {
                    let (status, attrs) =
                        crate::common::io2_to_io1_attrs(self.node.as_ref(), self.options.rights)
                            .await;
                    responder.send(status, &attrs)
                }
                .trace(trace::trace_future_args!("storage", "Node::GetAttr"))
                .await?;
            }
            #[cfg(fuchsia_api_level_at_least = "28")]
            fio::NodeRequest::DeprecatedSetAttr { flags: _, attributes: _, responder } => {
                trace::duration!("storage", "Node::SetAttr");
                responder.send(Status::BAD_HANDLE.into_raw())?;
            }
            #[cfg(not(fuchsia_api_level_at_least = "28"))]
            fio::NodeRequest::SetAttr { flags: _, attributes: _, responder } => {
                trace::duration!("storage", "Node::SetAttr");
                responder.send(Status::BAD_HANDLE.into_raw())?;
            }
            fio::NodeRequest::GetAttributes { query, responder } => {
                async move {
                    if !self.options.rights.intersects(fio::Operations::GET_ATTRIBUTES) {
                        responder.send(Err(Status::ACCESS_DENIED.into_raw()))
                    } else {
                        let attrs = self.node.get_attributes(query).await;
                        responder.send(
                            attrs
                                .as_ref()
                                .map(|attrs| {
                                    (&attrs.mutable_attributes, &attrs.immutable_attributes)
                                })
                                .map_err(|status| status.into_raw()),
                        )
                    }
                }
                .trace(trace::trace_future_args!("storage", "Node::GetAttributes"))
                .await?;
            }
            fio::NodeRequest::UpdateAttributes { payload: _, responder } => {
                trace::duration!("storage", "Node::UpdateAttributes");
                responder.send(Err(Status::BAD_HANDLE.into_raw()))?;
            }
            fio::NodeRequest::ListExtendedAttributes { iterator, .. } => {
                trace::duration!("storage", "Node::ListExtendedAttributes");
                iterator.close_with_epitaph(Status::NOT_SUPPORTED)?;
            }
            fio::NodeRequest::GetExtendedAttribute { responder, .. } => {
                trace::duration!("storage", "Node::GetExtendedAttribute");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::NodeRequest::SetExtendedAttribute { responder, .. } => {
                trace::duration!("storage", "Node::SetExtendedAttribute");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::NodeRequest::RemoveExtendedAttribute { responder, .. } => {
                trace::duration!("storage", "Node::RemoveExtendedAttribute");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::NodeRequest::GetFlags { responder } => {
                trace::duration!("storage", "Node::GetFlags");
                responder.send(Ok(fio::Flags::from(&self.options)))?;
            }
            fio::NodeRequest::SetFlags { flags: _, responder } => {
                trace::duration!("storage", "Node::SetFlags");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::NodeRequest::DeprecatedGetFlags { responder } => {
                trace::duration!("storage", "Node::GetFlags");
                responder.send(zx_status::sys::ZX_OK, fio::OpenFlags::NODE_REFERENCE)?;
            }
            fio::NodeRequest::DeprecatedSetFlags { flags: _, responder } => {
                trace::duration!("storage", "Node::SetFlags");
                responder.send(Status::BAD_HANDLE.into_raw())?;
            }
            fio::NodeRequest::Query { responder } => {
                trace::duration!("storage", "Node::Query");
                responder.send(fio::NodeMarker::PROTOCOL_NAME.as_bytes())?;
            }
            fio::NodeRequest::QueryFilesystem { responder } => {
                trace::duration!("storage", "Node::QueryFilesystem");
                responder.send(Status::NOT_SUPPORTED.into_raw(), None)?;
            }
            fio::NodeRequest::_UnknownMethod { .. } => (),
        }
        Ok(ConnectionState::Alive)
    }

    fn handle_clone(&mut self, server_end: ServerEnd<fio::NodeMarker>) {
        self.node.will_clone();
        let connection = Self {
            scope: self.scope.clone(),
            node: OpenNode::new(self.node.clone()),
            options: self.options,
        };
        self.scope.spawn(RequestListener::new(server_end.into_stream(), connection));
    }
}

impl<N: Node> RequestHandler for Connection<N> {
    type Request = Result<fio::NodeRequest, fidl::Error>;

    async fn handle_request(self: Pin<&mut Self>, request: Self::Request) -> ControlFlow<()> {
        let this = self.get_mut();
        if let Some(_guard) = this.scope.try_active_guard() {
            match request {
                Ok(request) => match this.handle_request(request).await {
                    Ok(ConnectionState::Alive) => ControlFlow::Continue(()),
                    Ok(ConnectionState::Closed) | Err(_) => ControlFlow::Break(()),
                },
                Err(_) => ControlFlow::Break(()),
            }
        } else {
            ControlFlow::Break(())
        }
    }
}

impl<N: Node> Representation for Connection<N> {
    type Protocol = fio::NodeMarker;

    async fn get_representation(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::Representation, Status> {
        Ok(fio::Representation::Node(fio::NodeInfo {
            attributes: if requested_attributes.is_empty() {
                None
            } else {
                Some(self.node.get_attributes(requested_attributes).await?)
            },
            ..Default::default()
        }))
    }

    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    async fn node_info(&self) -> Result<fio::NodeInfoDeprecated, Status> {
        Ok(fio::NodeInfoDeprecated::Service(fio::Service))
    }
}

impl<N: Node> ConnectionCreator<N> for Connection<N> {
    fn create<'a>(
        scope: ExecutionScope,
        node: Arc<N>,
        protocols: impl crate::ProtocolsExt,
        object_request: ObjectRequestRef<'a>,
    ) -> impl Future<Output = Result<(), Status>> + 'a {
        Self::create(scope, node, protocols, object_request)
    }
}

/// This struct is a RAII wrapper around a node that will call close() on it when dropped.
pub struct OpenNode<T: Node> {
    node: Arc<T>,
}

impl<T: Node> OpenNode<T> {
    pub fn new(node: Arc<T>) -> Self {
        Self { node }
    }
}

impl<T: Node> Drop for OpenNode<T> {
    fn drop(&mut self) {
        self.node.clone().close();
    }
}

impl<T: Node> std::ops::Deref for OpenNode<T> {
    type Target = Arc<T>;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}
