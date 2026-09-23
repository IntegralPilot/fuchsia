// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Connection to a directory that can not be modified by the client, no matter what permissions
//! the client has on the FIDL connection.

use crate::directory::connection::{BaseConnection, ConnectionState};
use crate::directory::entry_container::Directory;
use crate::execution_scope::ExecutionScope;
use crate::node::OpenNode;
use crate::object_request::ConnectionCreator;
use crate::request_handler::{RequestHandler, RequestListener};
use crate::{ObjectRequestRef, ProtocolsExt};

use fio::DirectoryRequest;
use flex_fuchsia_io as fio;
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use zx_status::Status;

pub struct ImmutableConnection<DirectoryType: Directory> {
    base: BaseConnection<DirectoryType>,
}

impl<DirectoryType: Directory> ImmutableConnection<DirectoryType> {
    /// Creates a new connection to serve the directory. The directory will be served from a new
    /// async `Task`, not from the current `Task`. Errors in constructing the connection are not
    /// guaranteed to be returned, they may be sent directly to the client end of the connection.
    /// This method should be called from within an `ObjectRequest` handler to ensure that errors
    /// are sent to the client end of the connection.
    pub fn create<'a>(
        scope: ExecutionScope,
        directory: Arc<DirectoryType>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'a>,
    ) -> impl Future<Output = Result<(), Status>> + 'a {
        Self::create_transform_stream(
            scope,
            directory,
            protocols,
            object_request,
            std::convert::identity,
        )
    }

    /// TODO(https://fxbug.dev/326626515): this is an experimental method to run a FIDL
    /// directory connection until stalled, with the purpose to cleanly stop a component.
    /// We'll expect to revisit how this works to generalize to all connections later.
    /// Try not to use this function for other purposes.
    pub async fn create_transform_stream<Transform, RS>(
        scope: ExecutionScope,
        directory: Arc<DirectoryType>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'_>,
        transform: Transform,
    ) -> Result<(), Status>
    where
        Transform: FnOnce(fio::DirectoryRequestStream) -> RS,
        RS: futures::stream::Stream<Item = Result<DirectoryRequest, fidl::Error>> + Send + 'static,
    {
        // Ensure we close the directory if we fail to create the connection.
        let directory = OpenNode::new(directory);

        let connection = ImmutableConnection {
            base: BaseConnection::new(scope.clone(), directory, protocols.to_directory_options()?),
        };

        // If we fail to send the task to the executor, it is probably shut down or is in the
        // process of shutting down (this is the only error state currently).  So there is nothing
        // for us to do - the connection will be closed automatically when the connection object is
        // dropped.
        if let Ok(requests) = object_request.take().into_request_stream(&connection.base).await {
            scope.spawn(RequestListener::new(transform(requests), connection));
        }
        Ok(())
    }
}

impl<DirectoryType: Directory> RequestHandler for ImmutableConnection<DirectoryType> {
    type Request = Result<DirectoryRequest, fidl::Error>;

    async fn handle_request(self: Pin<&mut Self>, request: Self::Request) -> ControlFlow<()> {
        let this = self.get_mut();
        if let Some(_guard) = this.base.scope.try_active_guard() {
            match request {
                Ok(request) => match this.base.handle_request(request).await {
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

impl<DirectoryType: Directory> ConnectionCreator<DirectoryType>
    for ImmutableConnection<DirectoryType>
{
    fn create<'a>(
        scope: ExecutionScope,
        node: Arc<DirectoryType>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'a>,
    ) -> impl Future<Output = Result<(), Status>> + 'a {
        Self::create(scope, node, protocols, object_request)
    }
}
