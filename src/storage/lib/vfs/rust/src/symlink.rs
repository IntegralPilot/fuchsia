// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Server support for symbolic links.

use crate::common::{
    decode_extended_attribute_value, encode_extended_attribute_value, extended_attributes_sender,
};
use crate::execution_scope::ExecutionScope;
use crate::name::Name;
use crate::node::{Node, OpenNode};
use crate::object_request::{ConnectionCreator, Representation, run_synchronous_future_or_spawn};
use crate::request_handler::{RequestHandler, RequestListener};
use crate::{ObjectRequest, ObjectRequestRef, ProtocolsExt, ToObjectRequest};
use flex_client::fidl::{DiscoverableProtocolMarker as _, Responder, ServerEnd};
use flex_fuchsia_io as fio;
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use storage_trace::{self as trace, TraceFutureExt};
use zx_status::Status;

pub trait Symlink: Node {
    fn read_target(&self) -> impl Future<Output = Result<Vec<u8>, Status>> + Send;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SymlinkOptions {
    pub rights: fio::Operations,
}

pub struct Connection<T: Node> {
    scope: ExecutionScope,
    symlink: OpenNode<T>,
    options: SymlinkOptions,
}

impl<T: Symlink> Connection<T> {
    /// Creates a new connection to serve the symlink. The symlink will be served from a new async
    /// `Task`, not from the current `Task`. Errors in constructing the connection are not
    /// guaranteed to be returned, they may be sent directly to the client end of the connection.
    /// This method should be called from within an `ObjectRequest` handler to ensure that errors
    /// are sent to the client end of the connection.
    pub async fn create(
        scope: ExecutionScope,
        symlink: Arc<T>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        let options = protocols.to_symlink_options()?;
        let connection = Self { scope: scope.clone(), symlink: OpenNode::new(symlink), options };
        if let Ok(requests) = object_request.take().into_request_stream(&connection).await {
            scope.spawn(RequestListener::new(requests, connection));
        }
        Ok(())
    }

    /// Similar to `create` but optimized for symlinks whose implementation is synchronous and
    /// creating the connection is being done from a non-async context.
    pub fn create_sync(
        scope: ExecutionScope,
        symlink: Arc<T>,
        options: impl ProtocolsExt,
        object_request: ObjectRequest,
    ) {
        run_synchronous_future_or_spawn(
            scope.clone(),
            object_request.handle_async(async |object_request| {
                Self::create(scope, symlink, options, object_request).await
            }),
        )
    }

    // Returns true if the connection should terminate.
    async fn handle_request(&mut self, req: fio::SymlinkRequest) -> Result<bool, fidl::Error> {
        match req {
            #[cfg(any(
                fuchsia_api_level_at_least = "PLATFORM",
                not(fuchsia_api_level_at_least = "29")
            ))]
            fio::SymlinkRequest::DeprecatedClone { flags, object, control_handle: _ } => {
                crate::common::send_on_open_with_error(
                    flags.contains(fio::OpenFlags::DESCRIBE),
                    object,
                    Status::NOT_SUPPORTED,
                );
            }
            fio::SymlinkRequest::Clone { request, control_handle: _ } => {
                self.handle_clone(ServerEnd::new(request.into_channel()))
                    .trace(trace::trace_future_args!("storage", "Symlink::Clone"))
                    .await
            }
            fio::SymlinkRequest::Close { responder } => {
                trace::duration!("storage", "Symlink::Close");
                responder.send(Ok(()))?;
                return Ok(true);
            }
            fio::SymlinkRequest::LinkInto { dst_parent_token, dst, responder } => {
                async move {
                    responder.send(
                        self.handle_link_into(dst_parent_token, dst)
                            .await
                            .map_err(|s| s.into_raw()),
                    )
                }
                .trace(trace::trace_future_args!("storage", "Symlink::LinkInto"))
                .await?;
            }
            fio::SymlinkRequest::Sync { responder } => {
                trace::duration!("storage", "Symlink::Sync");
                responder.send(Ok(()))?;
            }
            #[cfg(fuchsia_api_level_at_least = "28")]
            fio::SymlinkRequest::DeprecatedGetAttr { responder } => {
                // TODO(https://fxbug.dev/293947862): Restrict GET_ATTRIBUTES.
                let (status, attrs) = crate::common::io2_to_io1_attrs(
                    self.symlink.as_ref(),
                    fio::Rights::GET_ATTRIBUTES,
                )
                .await;
                responder.send(status, &attrs)?;
            }
            #[cfg(not(fuchsia_api_level_at_least = "28"))]
            fio::SymlinkRequest::GetAttr { responder } => {
                // TODO(https://fxbug.dev/293947862): Restrict GET_ATTRIBUTES.
                let (status, attrs) = crate::common::io2_to_io1_attrs(
                    self.symlink.as_ref(),
                    fio::Rights::GET_ATTRIBUTES,
                )
                .await;
                responder.send(status, &attrs)?;
            }
            #[cfg(fuchsia_api_level_at_least = "28")]
            fio::SymlinkRequest::DeprecatedSetAttr { responder, .. } => {
                responder.send(Status::ACCESS_DENIED.into_raw())?;
            }
            #[cfg(not(fuchsia_api_level_at_least = "28"))]
            fio::SymlinkRequest::SetAttr { responder, .. } => {
                responder.send(Status::ACCESS_DENIED.into_raw())?;
            }
            fio::SymlinkRequest::GetAttributes { query, responder } => {
                async move {
                    match self.handle_get_attributes(query).await {
                        Ok(attrs) => responder
                            .send(Ok((&attrs.mutable_attributes, &attrs.immutable_attributes))),
                        Err(status) => responder.send(Err(status.into_raw())),
                    }
                }
                .trace(trace::trace_future_args!("storage", "Symlink::GetAttributes"))
                .await?;
            }
            fio::SymlinkRequest::UpdateAttributes { payload: _, responder } => {
                trace::duration!("storage", "Symlink::UpdateAttributes");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::SymlinkRequest::ListExtendedAttributes { iterator, control_handle: _ } => {
                self.handle_list_extended_attribute(iterator)
                    .trace(trace::trace_future_args!("storage", "Symlink::ListExtendedAttributes"))
                    .await;
            }
            fio::SymlinkRequest::GetExtendedAttribute { responder, name } => {
                async move {
                    let res = self.handle_get_extended_attribute(name).await;
                    responder.send(res.map_err(Status::into_raw))
                }
                .trace(trace::trace_future_args!("storage", "Symlink::GetExtendedAttribute"))
                .await?;
            }
            fio::SymlinkRequest::SetExtendedAttribute { responder, name, value, mode } => {
                async move {
                    let res = self.handle_set_extended_attribute(name, value, mode).await;
                    responder.send(res.map_err(Status::into_raw))
                }
                .trace(trace::trace_future_args!("storage", "Symlink::SetExtendedAttribute"))
                .await?;
            }
            fio::SymlinkRequest::RemoveExtendedAttribute { responder, name } => {
                async move {
                    let res = self.handle_remove_extended_attribute(name).await;
                    responder.send(res.map_err(Status::into_raw))
                }
                .trace(trace::trace_future_args!("storage", "Symlink::RemoveExtendedAttribute"))
                .await?;
            }
            fio::SymlinkRequest::Describe { responder } => {
                return async move {
                    match self.symlink.read_target().await {
                        Ok(target) => {
                            responder.send(&fio::SymlinkInfo {
                                target: Some(target),
                                ..Default::default()
                            })?;
                            Ok(false)
                        }
                        Err(status) => {
                            responder.control_handle().shutdown_with_epitaph(status);
                            Ok(true)
                        }
                    }
                }
                .trace(trace::trace_future_args!("storage", "Symlink::Describe"))
                .await;
            }
            fio::SymlinkRequest::GetFlags { responder } => {
                trace::duration!("storage", "Symlink::GetFlags");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::SymlinkRequest::SetFlags { flags: _, responder } => {
                trace::duration!("storage", "Symlink::SetFlags");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::SymlinkRequest::DeprecatedGetFlags { responder } => {
                responder.send(Status::NOT_SUPPORTED.into_raw(), fio::OpenFlags::empty())?;
            }
            fio::SymlinkRequest::DeprecatedSetFlags { responder, .. } => {
                responder.send(Status::ACCESS_DENIED.into_raw())?;
            }
            fio::SymlinkRequest::Query { responder } => {
                trace::duration!("storage", "Symlink::Query");
                responder.send(fio::SymlinkMarker::PROTOCOL_NAME.as_bytes())?;
            }
            fio::SymlinkRequest::QueryFilesystem { responder } => {
                trace::duration!("storage", "Symlink::QueryFilesystem");
                match self.symlink.query_filesystem() {
                    Err(status) => responder.send(status.into_raw(), None)?,
                    Ok(info) => responder.send(zx_status::sys::ZX_OK, Some(&info))?,
                }
            }
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            fio::SymlinkRequest::Open { object, .. } => {
                use fidl::epitaph::ChannelEpitaphExt;
                let _ = object.close_with_epitaph(Status::NOT_DIR);
            }
            fio::SymlinkRequest::_UnknownMethod { ordinal: _ordinal, .. } => {
                #[cfg(any(test, feature = "use_log"))]
                log::warn!(_ordinal; "Received unknown method")
            }
        }
        Ok(false)
    }
    async fn handle_get_attributes(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        // Note: Symlink connections are required to have GET_ATTRIBUTES rights upon creation
        // (enforced in `to_symlink_options`). We check it here anyway for consistency and safety.
        if !self.options.rights.intersects(fio::Operations::GET_ATTRIBUTES) {
            return Err(Status::ACCESS_DENIED);
        }
        self.symlink.get_attributes(query).await
    }

    async fn handle_clone(&mut self, server_end: ServerEnd<fio::SymlinkMarker>) {
        let mut flags = fio::Flags::PROTOCOL_SYMLINK;
        if self.options.rights.contains(fio::Operations::GET_ATTRIBUTES) {
            flags |= fio::Flags::PERM_GET_ATTRIBUTES;
        }
        if self.options.rights.contains(fio::Operations::READ_BYTES) {
            flags |= fio::Flags::PERM_READ_BYTES;
        }
        if self.options.rights.contains(fio::Operations::WRITE_BYTES) {
            flags |= fio::Flags::PERM_WRITE_BYTES;
        }
        self.symlink.will_clone();
        flags
            .to_object_request(server_end)
            .handle_async(async |object_request| {
                Self::create(self.scope.clone(), self.symlink.clone(), flags, object_request).await
            })
            .await;
    }

    async fn handle_link_into(
        &mut self,
        target_parent_token: flex_client::Event,
        target_name: String,
    ) -> Result<(), Status> {
        let target_name = Name::try_from(target_name).map_err(|_| Status::INVALID_ARGS)?;

        // Enforce the maximum supported rights for symlinks to prevent rights escalation by
        // hardlinking it into a more privileged connection. In particular, this protects
        // attributes guarded by GET_ATTRIBUTES as well as extended attributes which are governed
        // by READ_BYTES and WRITE_BYTES.
        if !self.options.rights.contains(
            fio::Operations::READ_BYTES
                | fio::Operations::WRITE_BYTES
                | fio::Operations::GET_ATTRIBUTES,
        ) {
            return Err(Status::ACCESS_DENIED);
        }

        let (target_parent, target_rights) = self
            .scope
            .token_registry()
            .get_owner_and_rights(target_parent_token.into())?
            .ok_or(Status::NOT_FOUND)?;

        if !target_rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::ACCESS_DENIED);
        }

        self.symlink.clone().link_into(target_parent, target_name).await
    }

    async fn handle_list_extended_attribute(
        &self,
        iterator: ServerEnd<fio::ExtendedAttributeIteratorMarker>,
    ) {
        if !self.options.rights.intersects(fio::Operations::READ_BYTES) {
            let _ = iterator.close_with_epitaph(Status::BAD_HANDLE);
            return;
        }
        let attributes = match self.symlink.list_extended_attributes().await {
            Ok(attributes) => attributes,
            Err(status) => {
                #[cfg(any(test, feature = "use_log"))]
                log::error!(status:?; "list extended attributes failed");
                #[allow(clippy::unnecessary_lazy_evaluations)]
                iterator.close_with_epitaph(status).unwrap_or_else(|_error| {
                    #[cfg(any(test, feature = "use_log"))]
                    log::error!(_error:?; "failed to send epitaph")
                });
                return;
            }
        };
        self.scope.spawn(extended_attributes_sender(iterator, attributes));
    }

    async fn handle_get_extended_attribute(
        &self,
        name: Vec<u8>,
    ) -> Result<fio::ExtendedAttributeValue, Status> {
        if !self.options.rights.intersects(fio::Operations::READ_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        let value = self.symlink.get_extended_attribute(name).await?;
        encode_extended_attribute_value(value)
    }

    async fn handle_set_extended_attribute(
        &self,
        name: Vec<u8>,
        value: fio::ExtendedAttributeValue,
        mode: fio::SetExtendedAttributeMode,
    ) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        if name.contains(&0) {
            return Err(Status::INVALID_ARGS);
        }
        let val = decode_extended_attribute_value(value)?;
        self.symlink.set_extended_attribute(name, val, mode).await
    }

    async fn handle_remove_extended_attribute(&self, name: Vec<u8>) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        self.symlink.remove_extended_attribute(name).await
    }
}

impl<T: Symlink> RequestHandler for Connection<T> {
    type Request = Result<fio::SymlinkRequest, fidl::Error>;

    async fn handle_request(self: Pin<&mut Self>, request: Self::Request) -> ControlFlow<()> {
        let this = self.get_mut();
        if let Some(_guard) = this.scope.try_active_guard() {
            match request {
                Ok(request) => match this.handle_request(request).await {
                    Ok(false) => ControlFlow::Continue(()),
                    Ok(true) | Err(_) => ControlFlow::Break(()),
                },
                Err(_) => ControlFlow::Break(()),
            }
        } else {
            ControlFlow::Break(())
        }
    }
}

impl<T: Symlink> Representation for Connection<T> {
    type Protocol = fio::SymlinkMarker;

    async fn get_representation(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::Representation, Status> {
        Ok(fio::Representation::Symlink(fio::SymlinkInfo {
            attributes: if requested_attributes.is_empty() {
                None
            } else {
                Some(self.symlink.get_attributes(requested_attributes).await?)
            },
            target: Some(self.symlink.read_target().await?),
            ..Default::default()
        }))
    }

    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    async fn node_info(&self) -> Result<fio::NodeInfoDeprecated, Status> {
        Ok(fio::NodeInfoDeprecated::Symlink(fio::SymlinkObject {
            target: self.symlink.read_target().await?,
        }))
    }
}

impl<T: Symlink> ConnectionCreator<T> for Connection<T> {
    fn create<'a>(
        scope: ExecutionScope,
        node: Arc<T>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'a>,
    ) -> impl Future<Output = Result<(), Status>> + 'a {
        Self::create(scope, node, protocols, object_request)
    }
}

/// Helper to open a symlink or node as required.
pub fn serve(
    link: Arc<impl Symlink>,
    scope: ExecutionScope,
    protocols: impl ProtocolsExt,
    object_request: ObjectRequestRef<'_>,
) -> Result<(), Status> {
    if protocols.is_node() {
        let options = protocols.to_node_options(link.entry_info().type_())?;
        link.open_as_node(scope, options, object_request)
    } else {
        Connection::create_sync(scope, link, protocols, object_request.take());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, ExecutionScope, Symlink};
    use crate::ToObjectRequest;
    use crate::directory::entry::{EntryInfo, GetEntryInfo};
    use crate::node::Node;
    use assert_matches::assert_matches;
    use flex_client::fidl::ServerEnd;
    use flex_fuchsia_io as fio;
    use fuchsia_sync::Mutex;
    use futures::StreamExt;
    use std::collections::HashMap;
    use std::sync::Arc;
    use zx_status::Status;

    fn test_scope() -> ExecutionScope {
        #[cfg(feature = "fdomain")]
        let client = flex_local::local_client_empty();
        #[cfg(feature = "fdomain")]
        return ExecutionScope::new(client);
        #[cfg(not(feature = "fdomain"))]
        return ExecutionScope::new();
    }

    const TARGET: &[u8] = b"target";

    struct TestSymlink {
        xattrs: Mutex<HashMap<Vec<u8>, Vec<u8>>>,
    }

    impl TestSymlink {
        fn new() -> Self {
            TestSymlink { xattrs: Mutex::new(HashMap::new()) }
        }
    }

    impl Symlink for TestSymlink {
        async fn read_target(&self) -> Result<Vec<u8>, Status> {
            Ok(TARGET.to_vec())
        }
    }

    impl Node for TestSymlink {
        async fn get_attributes(
            &self,
            requested_attributes: fio::NodeAttributesQuery,
        ) -> Result<fio::NodeAttributes2, Status> {
            Ok(immutable_attributes!(
                requested_attributes,
                Immutable {
                    content_size: TARGET.len() as u64,
                    storage_size: TARGET.len() as u64,
                    protocols: fio::NodeProtocolKinds::SYMLINK,
                    abilities: fio::Abilities::GET_ATTRIBUTES,
                }
            ))
        }
        async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, Status> {
            let map = self.xattrs.lock();
            Ok(map.values().map(|x| x.clone()).collect())
        }
        async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, Status> {
            let map = self.xattrs.lock();
            map.get(&name).map(|x| x.clone()).ok_or(Status::NOT_FOUND)
        }
        async fn set_extended_attribute(
            &self,
            name: Vec<u8>,
            value: Vec<u8>,
            _mode: fio::SetExtendedAttributeMode,
        ) -> Result<(), Status> {
            let mut map = self.xattrs.lock();
            // Don't bother replicating the mode behavior, we just care that this method is hooked
            // up at all.
            map.insert(name, value);
            Ok(())
        }
        async fn remove_extended_attribute(&self, name: Vec<u8>) -> Result<(), Status> {
            let mut map = self.xattrs.lock();
            map.remove(&name);
            Ok(())
        }
    }

    impl GetEntryInfo for TestSymlink {
        fn entry_info(&self) -> EntryInfo {
            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Symlink)
        }
    }

    async fn serve_test_symlink(
        client: &flex_client::ClientArg,
        symlink: Arc<TestSymlink>,
        rights: fio::Flags,
    ) -> fio::SymlinkProxy {
        let (client_end, server_end) = client.create_proxy::<fio::SymlinkMarker>();
        let flags = rights | fio::Flags::PROTOCOL_SYMLINK;

        #[cfg(feature = "fdomain")]
        let scope = crate::execution_scope::ExecutionScope::new(client.clone());
        #[cfg(not(feature = "fdomain"))]
        let scope = crate::execution_scope::ExecutionScope::new();

        Connection::create_sync(scope, symlink, flags, flags.to_object_request(server_end));

        client_end
    }

    #[fuchsia::test]
    async fn test_read_target() {
        let client = flex_local::local_client_empty();
        let client_end =
            serve_test_symlink(&client, Arc::new(TestSymlink::new()), fio::PERM_READABLE).await;

        assert_eq!(
            client_end.describe().await.expect("fidl failed").target.expect("missing target"),
            b"target"
        );
    }

    #[fuchsia::test]
    async fn test_validate_flags() {
        let scope = test_scope();

        let check = |mut flags: fio::Flags| {
            let (client_end, server_end) = scope.domain().create_proxy::<fio::SymlinkMarker>();
            flags |= fio::Flags::FLAG_SEND_REPRESENTATION;
            flags.to_object_request(server_end).create_connection_sync::<Connection<_>, _>(
                scope.clone(),
                Arc::new(TestSymlink::new()),
                flags,
            );

            async move { client_end.take_event_stream().next().await.expect("no event") }
        };

        for flags in [
            fio::Flags::PROTOCOL_DIRECTORY,
            fio::Flags::PROTOCOL_FILE,
            fio::Flags::PROTOCOL_SERVICE,
        ] {
            assert_matches!(
                check(fio::PERM_READABLE | flags).await,
                Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                    if epitaph == Status::WRONG_TYPE,
                "{flags:?}"
            );
        }

        assert_matches!(
            check(fio::PERM_READABLE | fio::Flags::PROTOCOL_SYMLINK)
                .await
                .expect("error from next")
                .into_on_representation()
                .expect("expected on representation"),
            fio::Representation::Symlink(fio::SymlinkInfo { .. })
        );
        assert_matches!(
            check(fio::PERM_READABLE)
                .await
                .expect("error from next")
                .into_on_representation()
                .expect("expected on representation"),
            fio::Representation::Symlink(fio::SymlinkInfo { .. })
        );
    }

    #[fuchsia::test]
    async fn test_get_attr() {
        let client = flex_local::local_client_empty();
        let client_end =
            serve_test_symlink(&client, Arc::new(TestSymlink::new()), fio::PERM_READABLE).await;

        let (mutable_attrs, immutable_attrs) = client_end
            .get_attributes(fio::NodeAttributesQuery::all())
            .await
            .expect("fidl failed")
            .expect("GetAttributes failed");

        assert_eq!(mutable_attrs, Default::default());
        assert_eq!(
            immutable_attrs,
            fio::ImmutableNodeAttributes {
                content_size: Some(TARGET.len() as u64),
                storage_size: Some(TARGET.len() as u64),
                protocols: Some(fio::NodeProtocolKinds::SYMLINK),
                abilities: Some(fio::Abilities::GET_ATTRIBUTES),
                ..Default::default()
            }
        );
    }

    #[fuchsia::test]
    async fn test_clone() {
        let client = flex_local::local_client_empty();
        let client_end =
            serve_test_symlink(&client, Arc::new(TestSymlink::new()), fio::PERM_READABLE).await;

        let orig_attrs = client_end
            .get_attributes(fio::NodeAttributesQuery::all())
            .await
            .expect("fidl failed")
            .unwrap();
        // Clone the original connection and query it's attributes, which should match the original.
        let (cloned_client, cloned_server) = client.create_proxy::<fio::SymlinkMarker>();
        client_end.clone(ServerEnd::new(cloned_server.into_channel())).unwrap();
        let cloned_attrs = cloned_client
            .get_attributes(fio::NodeAttributesQuery::all())
            .await
            .expect("fidl failed")
            .unwrap();
        assert_eq!(orig_attrs, cloned_attrs);
    }

    #[fuchsia::test]
    async fn test_describe() {
        let client = flex_local::local_client_empty();
        let client_end =
            serve_test_symlink(&client, Arc::new(TestSymlink::new()), fio::PERM_READABLE).await;

        assert_matches!(
            client_end.describe().await.expect("fidl failed"),
            fio::SymlinkInfo {
                target: Some(target),
                ..
            } if target == b"target"
        );
    }

    #[fuchsia::test]
    async fn test_xattrs() {
        let client = flex_local::local_client_empty();
        let symlink = Arc::new(TestSymlink::new());
        let rw_client_end =
            serve_test_symlink(&client, symlink.clone(), fio::PERM_READABLE | fio::PERM_WRITABLE)
                .await;
        let ro_client_end = serve_test_symlink(&client, symlink, fio::PERM_READABLE).await;

        assert_eq!(
            ro_client_end
                .set_extended_attribute(
                    b"foo",
                    fio::ExtendedAttributeValue::Bytes(b"bar".to_vec()),
                    fio::SetExtendedAttributeMode::Set,
                )
                .await
                .unwrap()
                .unwrap_err(),
            Status::BAD_HANDLE.into_raw(),
        );

        rw_client_end
            .set_extended_attribute(
                b"foo",
                fio::ExtendedAttributeValue::Bytes(b"bar".to_vec()),
                fio::SetExtendedAttributeMode::Set,
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            ro_client_end.get_extended_attribute(b"foo").await.unwrap().unwrap(),
            fio::ExtendedAttributeValue::Bytes(b"bar".to_vec()),
        );

        let (iterator_client_end, iterator_server_end) =
            client.create_proxy::<fio::ExtendedAttributeIteratorMarker>();
        ro_client_end.list_extended_attributes(iterator_server_end).unwrap();
        assert_eq!(
            iterator_client_end.get_next().await.unwrap().unwrap(),
            (vec![b"bar".to_vec()], true)
        );

        assert_eq!(
            ro_client_end.remove_extended_attribute(b"foo").await.unwrap().unwrap_err(),
            Status::BAD_HANDLE.into_raw(),
        );

        rw_client_end.remove_extended_attribute(b"foo").await.unwrap().unwrap();

        assert_eq!(
            ro_client_end.get_extended_attribute(b"foo").await.unwrap().unwrap_err(),
            Status::NOT_FOUND.into_raw(),
        );
    }

    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    #[fuchsia::test]
    async fn test_open() {
        let client = flex_local::local_client_empty();
        let client_end =
            serve_test_symlink(&client, Arc::new(TestSymlink::new()), fio::PERM_READABLE).await;

        #[cfg(feature = "fdomain")]
        let (object, server_end) = client.create_channel();
        #[cfg(not(feature = "fdomain"))]
        let (object, server_end) = fidl::Channel::create();
        client_end
            .open("path", fio::Flags::empty(), &fio::Options::default(), server_end)
            .expect("fidl failed");

        #[cfg(feature = "fdomain")]
        let requests = fio::NodeProxy::new(object);
        #[cfg(not(feature = "fdomain"))]
        let requests = {
            use fidl::endpoints::Proxy;
            fio::NodeProxy::from_channel(fuchsia_async::Channel::from_channel(object))
        };

        let error = requests
            .take_event_stream()
            .next()
            .await
            .expect("no event")
            .expect_err("error expected");

        assert_matches!(error, fidl::Error::ClientChannelClosed { epitaph, .. }
            if epitaph == Status::NOT_DIR);
    }
}
