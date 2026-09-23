// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
use crate::common::send_on_open_with_error;
use crate::common::{
    decode_extended_attribute_value, encode_extended_attribute_value, extended_attributes_sender,
};
#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
use crate::directory::common::check_child_connection_flags;
use crate::directory::entry_container::{Directory, DirectoryWatcher};
use crate::directory::traversal_position::TraversalPosition;
use crate::directory::{DirectoryOptions, read_dirents};
use crate::execution_scope::{ExecutionScope, yield_to_executor};
use crate::node::OpenNode;
use crate::object_request::Representation;
use crate::path::Path;
use flex_client::fidl::DiscoverableProtocolMarker as _;

use anyhow::Error;
use flex_client::fidl::ServerEnd;
use flex_fuchsia_io as fio;
use storage_trace::{self as trace, TraceFutureExt};
use zx_status::Status;

use crate::common::CreationMode;
use crate::{ObjectRequest, ObjectRequestRef, ProtocolsExt};

/// Return type for `BaseConnection::handle_request`.
pub enum ConnectionState {
    /// Connection is still alive.
    Alive,
    /// Connection have received Node::Close message and should be closed.
    Closed,
}

/// Handles functionality shared between mutable and immutable FIDL connections to a directory.  A
/// single directory may contain multiple connections.  Instances of the `BaseConnection`
/// will also hold any state that is "per-connection".  Currently that would be the access flags
/// and the seek position.
pub(in crate::directory) struct BaseConnection<DirectoryType: Directory> {
    /// Execution scope this connection and any async operations and connections it creates will
    /// use.
    pub(in crate::directory) scope: ExecutionScope,

    pub(in crate::directory) directory: OpenNode<DirectoryType>,

    /// Flags set on this connection when it was opened or cloned.
    pub(in crate::directory) options: DirectoryOptions,

    /// Seek position for this connection to the directory.  We just store the element that was
    /// returned last by ReadDirents for this connection.  Next call will look for the next element
    /// in alphabetical order and resume from there.
    ///
    /// An alternative is to use an intrusive tree to have a dual index in both names and IDs that
    /// are assigned to the entries in insertion order.  Then we can store an ID instead of the
    /// full entry name.  This is what the C++ version is doing currently.
    ///
    /// It should be possible to do the same intrusive dual-indexing using, for example,
    ///
    ///     https://docs.rs/intrusive-collections/0.7.6/intrusive_collections/
    ///
    /// but, as, I think, at least for the pseudo directories, this approach is fine, and it simple
    /// enough.
    seek: TraversalPosition,
}

impl<DirectoryType: Directory> BaseConnection<DirectoryType> {
    /// Constructs an instance of `BaseConnection` - to be used by derived connections, when they
    /// need to create a nested `BaseConnection` "sub-object".  But when implementing
    /// `create_connection`, derived connections should use the [`create_connection`] call.
    pub(in crate::directory) fn new(
        scope: ExecutionScope,
        directory: OpenNode<DirectoryType>,
        options: DirectoryOptions,
    ) -> Self {
        BaseConnection { scope, directory, options, seek: Default::default() }
    }

    /// Handle a [`DirectoryRequest`].  This function is responsible for handing all the basic
    /// directory operations.
    pub(in crate::directory) async fn handle_request(
        &mut self,
        request: fio::DirectoryRequest,
    ) -> Result<ConnectionState, Error> {
        match request {
            #[cfg(any(
                fuchsia_api_level_at_least = "PLATFORM",
                not(fuchsia_api_level_at_least = "29")
            ))]
            fio::DirectoryRequest::DeprecatedClone { flags, object, control_handle: _ } => {
                trace::duration!("storage", "Directory::DeprecatedClone");
                crate::common::send_on_open_with_error(
                    flags.contains(fio::OpenFlags::DESCRIBE),
                    object,
                    Status::NOT_SUPPORTED,
                );
            }
            fio::DirectoryRequest::Clone { request, control_handle: _ } => {
                trace::duration!("storage", "Directory::Clone");
                self.handle_clone(request.into_channel());
            }
            fio::DirectoryRequest::Close { responder } => {
                trace::duration!("storage", "Directory::Close");
                responder.send(Ok(()))?;
                return Ok(ConnectionState::Closed);
            }
            #[cfg(fuchsia_api_level_at_least = "28")]
            fio::DirectoryRequest::DeprecatedGetAttr { responder } => {
                async move {
                    let (status, attrs) = crate::common::io2_to_io1_attrs(
                        self.directory.as_ref(),
                        self.options.rights,
                    )
                    .await;
                    responder.send(status, &attrs)
                }
                .trace(trace::trace_future_args!("storage", "Directory::GetAttr"))
                .await?;
            }
            #[cfg(not(fuchsia_api_level_at_least = "28"))]
            fio::DirectoryRequest::GetAttr { responder } => {
                async move {
                    let (status, attrs) = crate::common::io2_to_io1_attrs(
                        self.directory.as_ref(),
                        self.options.rights,
                    )
                    .await;
                    responder.send(status, &attrs)
                }
                .trace(trace::trace_future_args!("storage", "Directory::GetAttr"))
                .await?;
            }
            fio::DirectoryRequest::GetAttributes { query, responder } => {
                async move {
                    match self.handle_get_attributes(query).await {
                        Ok(attrs) => responder
                            .send(Ok((&attrs.mutable_attributes, &attrs.immutable_attributes))),
                        Err(status) => responder.send(Err(status.into_raw())),
                    }
                }
                .trace(trace::trace_future_args!("storage", "Directory::GetAttributes"))
                .await?;
            }
            fio::DirectoryRequest::UpdateAttributes { payload: _, responder } => {
                trace::duration!("storage", "Directory::UpdateAttributes");
                // TODO(https://fxbug.dev/324112547): Handle unimplemented io2 method.
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::DirectoryRequest::ListExtendedAttributes { iterator, control_handle: _ } => {
                self.handle_list_extended_attribute(iterator)
                    .trace(trace::trace_future_args!(
                        "storage",
                        "Directory::ListExtendedAttributes"
                    ))
                    .await;
            }
            fio::DirectoryRequest::GetExtendedAttribute { name, responder } => {
                async move {
                    let res =
                        self.handle_get_extended_attribute(name).await.map_err(Status::into_raw);
                    responder.send(res)
                }
                .trace(trace::trace_future_args!("storage", "Directory::GetExtendedAttribute"))
                .await?;
            }
            fio::DirectoryRequest::SetExtendedAttribute { name, value, mode, responder } => {
                async move {
                    let res = self
                        .handle_set_extended_attribute(name, value, mode)
                        .await
                        .map_err(Status::into_raw);
                    responder.send(res)
                }
                .trace(trace::trace_future_args!("storage", "Directory::SetExtendedAttribute"))
                .await?;
            }
            fio::DirectoryRequest::RemoveExtendedAttribute { name, responder } => {
                async move {
                    let res =
                        self.handle_remove_extended_attribute(name).await.map_err(Status::into_raw);
                    responder.send(res)
                }
                .trace(trace::trace_future_args!("storage", "Directory::RemoveExtendedAttribute"))
                .await?;
            }
            fio::DirectoryRequest::GetFlags { responder } => {
                trace::duration!("storage", "Directory::GetFlags");
                responder.send(Ok(fio::Flags::from(&self.options)))?;
            }
            fio::DirectoryRequest::SetFlags { flags: _, responder } => {
                trace::duration!("storage", "Directory::SetFlags");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::DirectoryRequest::DeprecatedGetFlags { responder } => {
                trace::duration!("storage", "Directory::DeprecatedGetFlags");
                responder.send(zx_status::sys::ZX_OK, self.options.to_io1())?;
            }
            fio::DirectoryRequest::DeprecatedSetFlags { flags: _, responder } => {
                trace::duration!("storage", "Directory::DeprecatedSetFlags");
                responder.send(Status::NOT_SUPPORTED.into_raw())?;
            }
            #[cfg(any(
                fuchsia_api_level_at_least = "PLATFORM",
                not(fuchsia_api_level_at_least = "32")
            ))]
            fio::DirectoryRequest::DeprecatedOpen {
                flags,
                mode: _,
                path,
                object,
                control_handle: _,
            } => {
                {
                    trace::duration!("storage", "Directory::Open");
                    self.handle_deprecated_open(flags, path, object);
                }
                // Since open typically spawns a task, yield to the executor now to give that task a
                // chance to run before we try and process the next request for this directory.
                yield_to_executor().await;
            }
            fio::DirectoryRequest::AdvisoryLock { request: _, responder } => {
                trace::duration!("storage", "Directory::AdvisoryLock");
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::DirectoryRequest::ReadDirents { max_bytes, responder } => {
                async move {
                    let (status, entries) = self.handle_read_dirents(max_bytes).await;
                    responder.send(Status::result_into_raw(status), entries.as_slice())
                }
                .trace(trace::trace_future_args!("storage", "Directory::ReadDirents"))
                .await?;
            }
            fio::DirectoryRequest::Rewind { responder } => {
                trace::duration!("storage", "Directory::Rewind");
                let status = if !self.options.rights.intersects(fio::Operations::ENUMERATE) {
                    Status::ACCESS_DENIED.into_raw()
                } else {
                    self.seek = Default::default();
                    zx_status::sys::ZX_OK
                };
                responder.send(status)?;
            }
            fio::DirectoryRequest::Link { src, dst_parent_token, dst, responder } => {
                async move {
                    responder.send(Status::result_into_raw(
                        self.handle_link(&src, dst_parent_token, dst).await,
                    ))
                }
                .trace(trace::trace_future_args!("storage", "Directory::Link"))
                .await?;
            }
            fio::DirectoryRequest::Watch { mask, options, watcher, responder } => {
                trace::duration!("storage", "Directory::Watch");
                let raw_status = if options != 0 {
                    Status::INVALID_ARGS.into_raw()
                } else {
                    Status::result_into_raw(self.handle_watch(mask, watcher.into()))
                };
                responder.send(raw_status)?;
            }
            fio::DirectoryRequest::Query { responder } => {
                trace::duration!("storage", "Directory::Query");
                let () = responder.send(fio::DirectoryMarker::PROTOCOL_NAME.as_bytes())?;
            }
            fio::DirectoryRequest::QueryFilesystem { responder } => {
                trace::duration!("storage", "Directory::QueryFilesystem");
                match self.directory.query_filesystem() {
                    Err(status) => responder.send(status.into_raw(), None)?,
                    Ok(info) => responder.send(zx_status::sys::ZX_OK, Some(&info))?,
                }
            }
            fio::DirectoryRequest::Unlink { name: _, options: _, responder } => {
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::DirectoryRequest::GetToken { responder } => {
                responder.send(Status::NOT_SUPPORTED.into_raw(), None)?;
            }
            fio::DirectoryRequest::Rename { src: _, dst_parent_token: _, dst: _, responder } => {
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            #[cfg(fuchsia_api_level_at_least = "28")]
            fio::DirectoryRequest::DeprecatedSetAttr { flags: _, attributes: _, responder } => {
                responder.send(Status::NOT_SUPPORTED.into_raw())?;
            }
            #[cfg(not(fuchsia_api_level_at_least = "28"))]
            fio::DirectoryRequest::SetAttr { flags: _, attributes: _, responder } => {
                responder.send(Status::NOT_SUPPORTED.into_raw())?;
            }
            fio::DirectoryRequest::Sync { responder } => {
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::DirectoryRequest::CreateSymlink { responder, .. } => {
                responder.send(Err(Status::NOT_SUPPORTED.into_raw()))?;
            }
            fio::DirectoryRequest::Open { path, mut flags, options, object, control_handle: _ } => {
                {
                    // Remove POSIX flags when the respective rights are not available.
                    if !self.options.rights.contains(fio::INHERITED_WRITE_PERMISSIONS) {
                        flags &= !fio::Flags::PERM_INHERIT_WRITE;
                    }
                    if !self.options.rights.contains(fio::Rights::EXECUTE) {
                        flags &= !fio::Flags::PERM_INHERIT_EXECUTE;
                    }

                    ObjectRequest::new(flags, &options, object)
                        .handle_async(async |req| self.handle_open(path, flags, req).await)
                        .trace(trace::trace_future_args!("storage", "Directory::Open3"))
                        .await;
                }
                // Since open typically spawns a task, yield to the executor now to give that task a
                // chance to run before we try and process the next request for this directory.
                yield_to_executor().await;
            }
            fio::DirectoryRequest::_UnknownMethod { .. } => (),
        }
        Ok(ConnectionState::Alive)
    }
    async fn handle_get_attributes(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        if !self.options.rights.intersects(fio::Operations::GET_ATTRIBUTES) {
            return Err(Status::ACCESS_DENIED);
        }
        self.directory.get_attributes(query).await
    }

    fn handle_clone(&mut self, object: flex_client::Channel) {
        let flags = fio::Flags::from(&self.options);
        ObjectRequest::new(flags, &Default::default(), object)
            .handle(|req| self.directory.clone().open(self.scope.clone(), Path::dot(), flags, req));
    }

    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn handle_deprecated_open(
        &self,
        mut flags: fio::OpenFlags,
        path: String,
        server_end: ServerEnd<fio::NodeMarker>,
    ) {
        let describe = flags.intersects(fio::OpenFlags::DESCRIBE);

        let path = match Path::validate_and_split(path) {
            Ok(path) => path,
            Err(status) => {
                send_on_open_with_error(describe, server_end, status);
                return;
            }
        };

        if !path.is_dot() && !self.options.rights.contains(fio::Operations::TRAVERSE) {
            send_on_open_with_error(describe, server_end, Status::ACCESS_DENIED);
            return;
        }

        if path.is_dir() {
            flags |= fio::OpenFlags::DIRECTORY;
        }

        let flags = match check_child_connection_flags(self.options.to_io1(), flags) {
            Ok(updated) => updated,
            Err(status) => {
                send_on_open_with_error(describe, server_end, status);
                return;
            }
        };
        if path.is_dot() {
            if flags.intersects(fio::OpenFlags::NOT_DIRECTORY) {
                send_on_open_with_error(describe, server_end, Status::INVALID_ARGS);
                return;
            }
            if flags.intersects(fio::OpenFlags::CREATE_IF_ABSENT) {
                send_on_open_with_error(describe, server_end, Status::ALREADY_EXISTS);
                return;
            }
        }

        // It is up to the open method to handle OPEN_FLAG_DESCRIBE from this point on.
        let directory = self.directory.clone();
        directory.deprecated_open(self.scope.clone(), flags, path, server_end);
    }

    async fn handle_open(
        &self,
        path: String,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        let path = Path::validate_and_split(path)?;

        if !path.is_dot() && !self.options.rights.contains(fio::Operations::TRAVERSE) {
            return Err(Status::ACCESS_DENIED);
        }

        // Child connection must have stricter or same rights as the parent connection.
        if let Some(rights) = flags.rights() {
            if rights.intersects(!self.options.rights) {
                return Err(Status::ACCESS_DENIED);
            }
        }

        // If the target negotiated to a service, ensure this directory connection permits
        // connecting.
        if flags.intersects(fio::Flags::PROTOCOL_SERVICE)
            && !self.options.rights.contains(fio::Operations::CONNECT)
        {
            return Err(Status::ACCESS_DENIED);
        }

        // If requesting attributes, check permission.
        if !object_request.attributes().is_empty()
            && !self.options.rights.contains(fio::Operations::GET_ATTRIBUTES)
        {
            return Err(Status::ACCESS_DENIED);
        }

        match flags.creation_mode() {
            CreationMode::Never => {
                if object_request.create_attributes().is_some() {
                    return Err(Status::INVALID_ARGS);
                }
            }
            CreationMode::UnnamedTemporary | CreationMode::UnlinkableUnnamedTemporary => {
                // We only support creating unnamed temporary files.
                if !flags.intersects(fio::Flags::PROTOCOL_FILE) {
                    return Err(Status::NOT_SUPPORTED);
                }
                // The parent connection must be able to modify directories if creating an object.
                if !self.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
                    return Err(Status::ACCESS_DENIED);
                }
                // The ability to create an unnamed temporary file is dependent on the filesystem.
                // We won't know if the directory the path eventually leads to supports the creation
                // of unnamed temporary files until we have fully traversed the path. The way that
                // Rust VFS is set up is such that the filesystem is responsible for traversing the
                // path, so it is the filesystem's responsibility to report if it does not support
                // this feature.
            }
            CreationMode::AllowExisting | CreationMode::Always => {
                // The parent connection must be able to modify directories if creating an object.
                if !self.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
                    return Err(Status::ACCESS_DENIED);
                }

                let protocol_flags = flags & fio::MASK_KNOWN_PROTOCOLS;
                // If creating an object, exactly one protocol must be specified (the flags must be
                // a power of two and non-zero).
                if protocol_flags.is_empty()
                    || (protocol_flags.bits() & (protocol_flags.bits() - 1)) != 0
                {
                    return Err(Status::INVALID_ARGS);
                }
                // Only a directory or file object can be created.
                if !protocol_flags
                    .intersects(fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PROTOCOL_FILE)
                {
                    return Err(Status::NOT_SUPPORTED);
                }
            }
        }

        if path.is_dot() && flags.creation_mode() == CreationMode::Always {
            return Err(Status::ALREADY_EXISTS);
        }

        self.directory.clone().open_async(self.scope.clone(), path, flags, object_request).await
    }

    async fn handle_read_dirents(&mut self, max_bytes: u64) -> (Result<(), Status>, Vec<u8>) {
        if !self.options.rights.intersects(fio::Operations::ENUMERATE) {
            return (Err(Status::ACCESS_DENIED), Vec::new());
        }
        let (new_pos, sealed) =
            match self.directory.read_dirents(&self.seek, read_dirents::Sink::new(max_bytes)).await
            {
                Ok(res) => res,
                Err(e) => return (Err(e), Vec::new()),
            };
        self.seek = new_pos;
        match sealed.open().downcast::<read_dirents::Done>() {
            Ok(done) => (done.status, done.buf),
            Err(_) => {
                debug_assert!(
                    false,
                    "`read_dirents()` returned a `dirents_sink::Sealed` \
                    instance that is not an instance of the \
                    `read_dirents::Done`. This is a bug in the \
                    `read_dirents()` implementation."
                );
                (Err(Status::NOT_SUPPORTED), Vec::new())
            }
        }
    }

    async fn handle_link(
        &self,
        source_name: &str,
        target_parent_token: flex_client::NullableHandle,
        target_name: String,
    ) -> Result<(), Status> {
        if source_name.contains('/') || target_name.contains('/') {
            return Err(Status::INVALID_ARGS);
        }

        // To avoid rights escalation, we must make sure that the connection to the source directory
        // has the maximal set of file rights.  We do not check for EXECUTE because mutable
        // filesystems that support link don't currently support EXECUTE rights.
        if !self.options.rights.contains(fio::RW_STAR_DIR) {
            return Err(Status::BAD_HANDLE);
        }

        let (target_parent, target_rights) = self
            .scope
            .token_registry()
            .get_owner_and_rights(target_parent_token)?
            .ok_or(Status::NOT_FOUND)?;

        if !target_rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::BAD_HANDLE);
        }

        target_parent.link(target_name, self.directory.clone().into_any(), source_name).await
    }

    fn handle_watch(
        &mut self,
        mask: fio::WatchMask,
        watcher: DirectoryWatcher,
    ) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::ENUMERATE) {
            return Err(Status::ACCESS_DENIED);
        }
        let directory = self.directory.clone();
        directory.register_watcher(self.scope.clone(), mask, watcher)
    }

    async fn handle_list_extended_attribute(
        &self,
        iterator: ServerEnd<fio::ExtendedAttributeIteratorMarker>,
    ) {
        if !self.options.rights.intersects(fio::Operations::READ_BYTES) {
            let _ = iterator.close_with_epitaph(Status::BAD_HANDLE);
            return;
        }
        let attributes = match self.directory.list_extended_attributes().await {
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
        let value = self.directory.get_extended_attribute(name).await?;
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
        self.directory.set_extended_attribute(name, val, mode).await
    }

    async fn handle_remove_extended_attribute(&self, name: Vec<u8>) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        self.directory.remove_extended_attribute(name).await
    }
}

impl<DirectoryType: Directory> Representation for BaseConnection<DirectoryType> {
    type Protocol = fio::DirectoryMarker;

    async fn get_representation(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::Representation, Status> {
        Ok(fio::Representation::Directory(fio::DirectoryInfo {
            attributes: if requested_attributes.is_empty() {
                None
            } else {
                Some(self.directory.get_attributes(requested_attributes).await?)
            },
            ..Default::default()
        }))
    }

    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    async fn node_info(&self) -> Result<fio::NodeInfoDeprecated, Status> {
        Ok(fio::NodeInfoDeprecated::Directory(fio::DirectoryObject))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::immutable::Simple;
    use assert_matches::assert_matches;
    use flex_fuchsia_io as fio;

    #[cfg(not(feature = "fdomain"))]
    use fuchsia_fs::directory;
    #[cfg(feature = "fdomain")]
    use fuchsia_fs_fdomain::directory;

    fn test_scope() -> crate::execution_scope::ExecutionScope {
        #[cfg(feature = "fdomain")]
        let client = flex_local::local_client_empty();
        #[cfg(feature = "fdomain")]
        return crate::execution_scope::ExecutionScope::new(client);
        #[cfg(not(feature = "fdomain"))]
        return crate::execution_scope::ExecutionScope::new();
    }

    #[fuchsia::test]
    async fn test_open_not_found() {
        let dir = Simple::new();
        let scope = test_scope();
        let dir_proxy = crate::directory::serve(dir, scope.clone(), fio::PERM_READABLE);

        // Try to open a file that doesn't exist.
        let node_proxy =
            directory::open_async::<fio::NodeMarker>(&dir_proxy, "foo", fio::PERM_READABLE)
                .unwrap();

        // The channel is closed with a NOT_FOUND epitaph.
        assert_matches!(
            node_proxy.query().await,
            Err(fidl::Error::ClientChannelClosed {
                epitaph,
                protocol_name: "fuchsia.io.Node",
                ..
            }) if epitaph == Status::NOT_FOUND
        );
    }

    #[fuchsia::test]
    async fn test_open_with_send_representation_not_found() {
        let dir = Simple::new();
        let scope = test_scope();
        let dir_proxy = crate::directory::serve(dir, scope.clone(), fio::PERM_READABLE);

        // Try to open a file that doesn't exist.
        let node_proxy = directory::open_async::<fio::NodeMarker>(
            &dir_proxy,
            "foo",
            fio::PERM_READABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
        )
        .unwrap();

        // The channel is closed with a NOT_FOUND epitaph.
        assert_matches!(
            node_proxy.query().await,
            Err(fidl::Error::ClientChannelClosed {
                epitaph,
                protocol_name: "fuchsia.io.Node",
                ..
            }) if epitaph == Status::NOT_FOUND
        );
    }
}
