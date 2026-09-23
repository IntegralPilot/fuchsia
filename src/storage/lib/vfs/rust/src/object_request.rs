// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ProtocolsExt;
use crate::execution_scope::ExecutionScope;
use crate::node::{self, Node};
use fidl::epitaph::ChannelEpitaphExt;
#[cfg(feature = "fdomain")]
use flex_client::AsHandleRef;
use flex_client::fidl::{ProtocolMarker, RequestStream, ServerEnd};
use flex_fuchsia_io as fio;
#[cfg(not(feature = "fdomain"))]
use fuchsia_async as fasync;
use futures::FutureExt;
use std::future::Future;
use std::sync::Arc;
use zx_status::Status;

/// Wraps the channel provided in the open methods and provide convenience methods for sending
pub trait IntoAsyncChannel {
    fn into_async_channel(self) -> flex_client::AsyncChannel;
}

#[cfg(not(feature = "fdomain"))]
impl IntoAsyncChannel for flex_client::Channel {
    fn into_async_channel(self) -> flex_client::AsyncChannel {
        fasync::Channel::from_channel(self)
    }
}

#[cfg(feature = "fdomain")]
impl IntoAsyncChannel for flex_client::Channel {
    fn into_async_channel(self) -> flex_client::AsyncChannel {
        self
    }
}

/// Wraps the channel provided in the open methods and provide convenience methods for sending
/// appropriate responses.  It also records actions that should be taken upon successful connection
/// such as truncating file objects.
#[derive(Debug)]
pub struct ObjectRequest {
    // The channel.
    object_request: flex_client::Channel,

    // What should be sent first.
    what_to_send: ObjectRequestSend,

    // Attributes required in the open method.
    attributes: fio::NodeAttributesQuery,

    // Creation attributes.
    create_attributes: Option<Box<fio::MutableNodeAttributes>>,

    /// Truncate the object before use.
    pub truncate: bool,
}

impl ObjectRequest {
    pub(crate) fn new_deprecated(
        object_request: flex_client::Channel,
        what_to_send: ObjectRequestSend,
        attributes: fio::NodeAttributesQuery,
        create_attributes: Option<&fio::MutableNodeAttributes>,
        truncate: bool,
    ) -> Self {
        let create_attributes = create_attributes.map(|a| Box::new(a.clone()));
        Self {
            object_request: object_request,
            what_to_send,
            attributes,
            create_attributes,
            truncate,
        }
    }

    /// Create a new [`ObjectRequest`] from a set of [`fio::Flags`] and [`fio::Options`]`.
    pub fn new(
        flags: fio::Flags,
        options: &fio::Options,
        object_request: flex_client::Channel,
    ) -> Self {
        Self::new_deprecated(
            object_request,
            if flags.contains(fio::Flags::FLAG_SEND_REPRESENTATION) {
                ObjectRequestSend::OnRepresentation
            } else {
                ObjectRequestSend::Nothing
            },
            options.attributes.unwrap_or(fio::NodeAttributesQuery::empty()),
            options.create_attributes.as_ref(),
            flags.is_truncate(),
        )
    }

    pub(crate) fn what_to_send(&self) -> ObjectRequestSend {
        self.what_to_send
    }

    pub fn attributes(&self) -> fio::NodeAttributesQuery {
        self.attributes
    }

    pub fn create_attributes(&self) -> Option<&fio::MutableNodeAttributes> {
        self.create_attributes.as_deref()
    }

    pub fn options(&self) -> fio::Options {
        fio::Options {
            attributes: (!self.attributes.is_empty()).then_some(self.attributes),
            create_attributes: self
                .create_attributes
                .as_ref()
                .map(|a| fio::MutableNodeAttributes::clone(&a)),
            ..Default::default()
        }
    }

    /// Returns the request stream after sending requested information.
    pub async fn into_request_stream<T: Representation>(
        self,
        connection: &T,
    ) -> Result<<T::Protocol as ProtocolMarker>::RequestStream, Status> {
        let stream = fio::NodeRequestStream::from_channel(self.object_request.into_async_channel());
        match self.what_to_send {
            #[cfg(any(
                fuchsia_api_level_at_least = "PLATFORM",
                not(fuchsia_api_level_at_least = "32")
            ))]
            ObjectRequestSend::OnOpen => {
                let control_handle = stream.control_handle();
                let node_info = connection.node_info().await.map_err(|s| {
                    control_handle.shutdown_with_epitaph(s);
                    s
                })?;
                send_on_open(&stream.control_handle(), node_info)?;
            }
            ObjectRequestSend::OnRepresentation => {
                let control_handle = stream.control_handle();
                let representation =
                    connection.get_representation(self.attributes).await.map_err(|s| {
                        control_handle.shutdown_with_epitaph(s);
                        s
                    })?;
                control_handle
                    .send_on_representation(representation)
                    .map_err(|_| Status::PEER_CLOSED)?;
            }
            ObjectRequestSend::Nothing => {}
        }
        let (inner, is_terminated) = stream.into_inner();
        Ok(<<T as Representation>::Protocol as flex_client::fidl::ProtocolMarker>::RequestStream::from_inner(inner, is_terminated))
    }

    /// Converts to ServerEnd<T>.
    pub fn into_server_end<T: ProtocolMarker>(self) -> ServerEnd<T> {
        ServerEnd::new(self.object_request)
    }

    /// Extracts the channel (without sending on_open).
    pub fn into_channel(self) -> flex_client::Channel {
        self.object_request
    }

    /// Extracts the channel after sending on_open.
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    pub fn into_channel_after_sending_on_open(
        self,
        node_info: fio::NodeInfoDeprecated,
    ) -> Result<flex_client::Channel, Status> {
        let stream = fio::NodeRequestStream::from_channel(self.object_request.into_async_channel());
        send_on_open(&stream.control_handle(), node_info)?;
        let (inner, _is_terminated) = stream.into_inner();
        // It's safe to unwrap here because inner is clearly the only Arc reference left.
        Ok(Arc::try_unwrap(inner).unwrap().into_channel().into())
    }

    /// Terminates the object request with the given status.
    pub fn shutdown(self, status: Status) {
        if self.object_request.as_handle_ref().is_invalid() {
            return;
        }
        #[cfg(any(
            fuchsia_api_level_at_least = "PLATFORM",
            not(fuchsia_api_level_at_least = "32")
        ))]
        if let ObjectRequestSend::OnOpen = self.what_to_send {
            let (_, control_handle) = ServerEnd::<fio::NodeMarker>::new(self.object_request)
                .into_stream_and_control_handle();
            let _ = control_handle.send_on_open_(status.into_raw(), None);
            control_handle.shutdown_with_epitaph(status);
        } else {
            let _ = self.object_request.close_with_epitaph(status);
        }
        #[cfg(not(any(
            fuchsia_api_level_at_least = "PLATFORM",
            not(fuchsia_api_level_at_least = "32")
        )))]
        {
            let _ = self.object_request.close_with_epitaph(status);
        }
    }

    /// Calls `f` and sends an error on the object request channel upon failure.
    pub fn handle<T>(
        mut self,
        f: impl FnOnce(ObjectRequestRef<'_>) -> Result<T, Status>,
    ) -> Option<T> {
        match f(&mut self) {
            Ok(o) => Some(o),
            Err(s) => {
                self.shutdown(s);
                None
            }
        }
    }

    /// Calls `f` and sends an error on the object request channel upon failure.
    pub async fn handle_async(
        mut self,
        f: impl AsyncFnOnce(&mut ObjectRequest) -> Result<(), Status>,
    ) {
        if let Err(s) = f(&mut self).await {
            self.shutdown(s);
        }
    }

    /// Waits until the request has a request waiting in its channel.  Returns immediately if this
    /// request requires sending an initial event such as OnOpen or OnRepresentation.  Returns
    /// `true` if the channel is readable (rather than just closed).
    pub async fn wait_till_ready(&self) -> bool {
        if !matches!(self.what_to_send, ObjectRequestSend::Nothing) {
            return true;
        }
        flex_client::wait_for_signals(
            &self.object_request.as_handle_ref(),
            fidl::Signals::OBJECT_READABLE | fidl::Signals::CHANNEL_PEER_CLOSED,
        )
        .await
        .map(|x| x.contains(fidl::Signals::OBJECT_READABLE))
        .unwrap_or(false)
    }

    /// Take the ObjectRequest.  The caller is responsible for sending errors.
    pub fn take(&mut self) -> ObjectRequest {
        // assert!(!self.object_request.as_handle_ref().is_invalid());
        Self {
            object_request: std::mem::replace(
                &mut self.object_request,
                flex_client::NullableHandle::invalid().into(),
            ),
            what_to_send: self.what_to_send,
            attributes: self.attributes,
            create_attributes: self.create_attributes.take(),
            truncate: self.truncate,
        }
    }

    /// Constructs a new connection to `node` and spawns an async `Task` that will handle requests
    /// on the connection. `f` is a callback that constructs the connection but it will not be
    /// called if the connection is supposed to be a node connection. This should be called from
    /// within a [`ObjectRequest::handle_async`] callback.
    pub async fn create_connection<C, N>(
        &mut self,
        scope: ExecutionScope,
        node: Arc<N>,
        protocols: impl ProtocolsExt,
    ) -> Result<(), Status>
    where
        C: ConnectionCreator<N>,
        N: Node,
    {
        // assert!(!self.object_request.as_handle_ref().is_invalid());
        if protocols.is_node() {
            node::Connection::create(scope, node, protocols, self).await
        } else {
            C::create(scope, node, protocols, self).await
        }
    }

    /// Constructs a new connection to `node` and spawns an async `Task` that will handle requests
    /// on the connection. `f` is a callback that constructs the connection but it will not be
    /// called if the connection is supposed to be a node connection. This should be called from
    /// within a [`ObjectRequest::handle`] callback.
    ///
    /// This method synchronously calls async code and may require spawning an extra Task if the
    /// async code does something asynchronous. `create_connection` should be preferred if the
    /// caller is already in an async context.
    pub fn create_connection_sync<C, N>(
        self,
        scope: ExecutionScope,
        node: Arc<N>,
        protocols: impl ProtocolsExt,
    ) where
        C: ConnectionCreator<N>,
        N: Node,
    {
        // assert!(!self.object_request.as_handle_ref().is_invalid());
        if protocols.is_node() {
            self.create_connection_sync_or_spawn::<node::Connection<N>, N>(scope, node, protocols);
        } else {
            self.create_connection_sync_or_spawn::<C, N>(scope, node, protocols);
        }
    }

    fn create_connection_sync_or_spawn<C, N>(
        self,
        scope: ExecutionScope,
        node: Arc<N>,
        protocols: impl ProtocolsExt,
    ) where
        C: ConnectionCreator<N>,
        N: Node,
    {
        let scope2 = scope.clone();
        let fut = self.handle_async(async |object_request| {
            C::create(scope2, node, protocols, object_request).await
        });
        run_synchronous_future_or_spawn(scope, fut);
    }
}

pub type ObjectRequestRef<'a> = &'a mut ObjectRequest;

#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
pub(crate) enum ObjectRequestSend {
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    OnOpen,
    OnRepresentation,
    Nothing,
}

/// Trait to get either fio::Representation or fio::NodeInfoDeprecated.  Connection types
/// should implement this.
pub trait Representation {
    /// The protocol used for the connection.
    type Protocol: ProtocolMarker;

    /// Returns io2's Representation for the object.
    fn get_representation(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> impl Future<Output = Result<fio::Representation, Status>> + Send;

    /// Returns io1's NodeInfoDeprecated.
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn node_info(&self) -> impl Future<Output = Result<fio::NodeInfoDeprecated, Status>> + Send;
}

/// Convenience trait for converting [`fio::Flags`] and [`fio::OpenFlags`] into ObjectRequest.
///
/// If [`fio::Options`] need to be specified, use [`ObjectRequest::new`].
pub trait ToObjectRequest: ProtocolsExt {
    fn to_object_request(
        &self,
        object_request: impl Into<flex_client::NullableHandle>,
    ) -> ObjectRequest;
}

impl ToObjectRequest for fio::OpenFlags {
    fn to_object_request(
        &self,
        object_request: impl Into<flex_client::NullableHandle>,
    ) -> ObjectRequest {
        ObjectRequest::new_deprecated(
            object_request.into().into(),
            #[cfg(any(
                fuchsia_api_level_at_least = "PLATFORM",
                not(fuchsia_api_level_at_least = "32")
            ))]
            if self.contains(fio::OpenFlags::DESCRIBE) {
                ObjectRequestSend::OnOpen
            } else {
                ObjectRequestSend::Nothing
            },
            #[cfg(not(any(
                fuchsia_api_level_at_least = "PLATFORM",
                not(fuchsia_api_level_at_least = "32")
            )))]
            ObjectRequestSend::Nothing,
            fio::NodeAttributesQuery::empty(),
            None,
            self.is_truncate(),
        )
    }
}

impl ToObjectRequest for fio::Flags {
    fn to_object_request(
        &self,
        object_request: impl Into<flex_client::NullableHandle>,
    ) -> ObjectRequest {
        ObjectRequest::new(*self, &Default::default(), object_request.into().into())
    }
}

#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
fn send_on_open(
    control_handle: &fio::NodeControlHandle,
    node_info: fio::NodeInfoDeprecated,
) -> Result<(), Status> {
    control_handle
        .send_on_open_(zx_status::sys::ZX_OK, Some(node_info))
        .map_err(|_| Status::PEER_CLOSED)
}

/// Trait for constructing connections to nodes.
pub trait ConnectionCreator<T: Node> {
    /// Creates a new connection to `node` and spawns a new `Task` to run the connection.
    fn create<'a>(
        scope: ExecutionScope,
        node: Arc<T>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'a>,
    ) -> impl Future<Output = Result<(), Status>> + Send + 'a;
}

/// Synchronously polls `future` with the expectation that it won't return Pending. If the future
/// does return Pending then this function will spawn a Task to run the future.
pub(crate) fn run_synchronous_future_or_spawn(
    scope: ExecutionScope,
    future: impl Future<Output = ()> + Send + 'static,
) {
    let mut task = scope.new_task(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());

    match task.poll_unpin(&mut cx) {
        std::task::Poll::Pending => task.spawn(),
        std::task::Poll::Ready(()) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::future::ready;

    use crate::execution_scope::yield_to_executor;

    use super::*;

    #[fuchsia::test]
    async fn test_run_synchronous_future_or_spawn_with_sync_future() {
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        run_synchronous_future_or_spawn(scope.clone(), ready(()));
        scope.wait().await;
    }

    #[fuchsia::test]
    async fn test_run_synchronous_future_or_spawn_with_async_future() {
        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();
        run_synchronous_future_or_spawn(scope.clone(), yield_to_executor());
        scope.wait().await;
    }
}
