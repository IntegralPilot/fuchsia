// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Connection to a directory that can be modified by the client though a FIDL connection.

use crate::common::io1_to_io2_attrs;
use crate::directory::connection::{BaseConnection, ConnectionState};
use crate::directory::entry_container::MutableDirectory;
use crate::execution_scope::ExecutionScope;
use crate::name::validate_name;
use crate::node::OpenNode;
use crate::object_request::ConnectionCreator;
use crate::path::Path;
use crate::request_handler::{RequestHandler, RequestListener};
use crate::token_registry::{TokenInterface, TokenRegistry, Tokenizable};
use crate::{ObjectRequestRef, ProtocolsExt};

use anyhow::Error;
use flex_client::NullableHandle;
use flex_fuchsia_io as fio;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use storage_trace::{self as trace, TraceFutureExt};
use zx_status::Status;

pub struct MutableConnection<DirectoryType: MutableDirectory> {
    base: BaseConnection<DirectoryType>,
}

impl<DirectoryType: MutableDirectory> MutableConnection<DirectoryType> {
    /// Creates a new connection to serve the mutable directory. The directory will be served from a
    /// new async `Task`, not from the current `Task`. Errors in constructing the connection are not
    /// guaranteed to be returned, they may be sent directly to the client end of the connection.
    /// This method should be called from within an `ObjectRequest` handler to ensure that errors
    /// are sent to the client end of the connection.
    pub async fn create(
        scope: ExecutionScope,
        directory: Arc<DirectoryType>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        // Ensure we close the directory if we fail to prepare the connection.
        let directory = OpenNode::new(directory);

        let connection = MutableConnection {
            base: BaseConnection::new(scope.clone(), directory, protocols.to_directory_options()?),
        };

        if let Ok(requests) = object_request.take().into_request_stream(&connection.base).await {
            scope.spawn(RequestListener::new(requests, Tokenizable::new(connection)));
        }
        Ok(())
    }

    async fn handle_request(
        this: Pin<&mut Tokenizable<Self>>,
        request: fio::DirectoryRequest,
    ) -> Result<ConnectionState, Error> {
        match request {
            fio::DirectoryRequest::Unlink { name, options, responder } => {
                async move {
                    let result = this.handle_unlink(name, options).await;
                    responder.send(result.map_err(Status::into_raw))
                }
                .trace(trace::trace_future_args!("storage", "Directory::Unlink"))
                .await?;
            }
            fio::DirectoryRequest::GetToken { responder } => {
                trace::duration!("storage", "Directory::GetToken");
                let (raw_status, token) = match Self::handle_get_token(this.into_ref()) {
                    Ok(token) => (0, Some(token)),
                    Err(status) => (status.into_raw(), None),
                };
                responder.send(raw_status, token)?;
            }
            fio::DirectoryRequest::Rename { src, dst_parent_token, dst, responder } => {
                async move {
                    let result =
                        this.handle_rename(src, NullableHandle::from(dst_parent_token), dst).await;
                    responder.send(result.map_err(Status::into_raw))
                }
                .trace(trace::trace_future_args!("storage", "Directory::Rename"))
                .await?;
            }
            #[cfg(fuchsia_api_level_at_least = "28")]
            fio::DirectoryRequest::DeprecatedSetAttr { flags, attributes, responder } => {
                let raw_status = Status::result_into_raw(
                    this.handle_update_attributes(io1_to_io2_attrs(flags, attributes)).await,
                );
                responder.send(raw_status)?;
            }
            #[cfg(not(fuchsia_api_level_at_least = "28"))]
            fio::DirectoryRequest::SetAttr { flags, attributes, responder } => {
                let raw_status = Status::result_into_raw(
                    this.handle_update_attributes(io1_to_io2_attrs(flags, attributes)).await,
                );
                responder.send(raw_status)?;
            }
            fio::DirectoryRequest::Sync { responder } => {
                async move {
                    responder.send(this.base.directory.sync().await.map_err(Status::into_raw))
                }
                .trace(trace::trace_future_args!("storage", "Directory::Sync"))
                .await?;
            }
            fio::DirectoryRequest::CreateSymlink {
                responder, name, target, connection, ..
            } => {
                async move {
                    if !this.base.options.rights.contains(fio::Operations::MODIFY_DIRECTORY) {
                        responder.send(Err(Status::ACCESS_DENIED.into_raw()))
                    } else if validate_name(&name).is_err() {
                        responder.send(Err(Status::INVALID_ARGS.into_raw()))
                    } else {
                        responder.send(
                            this.base
                                .directory
                                .create_symlink(name, target, connection)
                                .await
                                .map_err(Status::into_raw),
                        )
                    }
                }
                .trace(trace::trace_future_args!("storage", "Directory::CreateSymlink"))
                .await?;
            }
            fio::DirectoryRequest::UpdateAttributes { payload, responder } => {
                async move {
                    responder.send(
                        this.handle_update_attributes(payload).await.map_err(Status::into_raw),
                    )
                }
                .trace(trace::trace_future_args!("storage", "Directory::UpdateAttributes"))
                .await?;
            }
            request => {
                return this.as_mut().base.handle_request(request).await;
            }
        }
        Ok(ConnectionState::Alive)
    }

    async fn handle_update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        if !self.base.options.rights.contains(fio::Operations::UPDATE_ATTRIBUTES) {
            return Err(Status::BAD_HANDLE);
        }
        // TODO(jfsulliv): Consider always permitting attributes to be deferrable. The risk with
        // this is that filesystems would require a background flush of dirty attributes to disk.
        self.base.directory.update_attributes(attributes).await
    }

    async fn handle_unlink(&self, name: String, options: fio::UnlinkOptions) -> Result<(), Status> {
        if !self.base.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::BAD_HANDLE);
        }

        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(Status::INVALID_ARGS);
        }

        self.base
            .directory
            .clone()
            .unlink(
                &name,
                options
                    .flags
                    .map(|f| f.contains(fio::UnlinkFlags::MUST_BE_DIRECTORY))
                    .unwrap_or(false),
            )
            .await
    }

    fn handle_get_token(this: Pin<&Tokenizable<Self>>) -> Result<NullableHandle, Status> {
        // TODO(https://fxbug.dev/503041342): The current GetToken method on directory requires
        // specific rights to get the token in the first place. The new GetToken on Node will
        // require no rights to get a token, but instead enforced on use of the token. When the old
        // one is deleted this method (and therefore this check) will be deleted too.
        if !this.base.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::BAD_HANDLE);
        }
        Ok(TokenRegistry::get_token(this)?)
    }

    async fn handle_rename(
        &self,
        src: String,
        dst_parent_token: NullableHandle,
        dst: String,
    ) -> Result<(), Status> {
        if !self.base.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::ACCESS_DENIED);
        }

        let src = Path::validate_and_split(src)?;
        let dst = Path::validate_and_split(dst)?;

        if !src.is_single_component() || !dst.is_single_component() {
            return Err(Status::INVALID_ARGS);
        }

        let (dst_parent, dst_rights) = self
            .base
            .scope
            .token_registry()
            .get_owner_and_rights(dst_parent_token)?
            .ok_or(Status::NOT_FOUND)?;

        if !dst_rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::ACCESS_DENIED);
        }

        let is_same_dir =
            Arc::ptr_eq(&(self.base.directory.clone() as Arc<dyn MutableDirectory>), &dst_parent);

        if !is_same_dir && !self.base.options.rights.contains(fio::RW_STAR_DIR) {
            return Err(Status::ACCESS_DENIED);
        }

        dst_parent.clone().rename(self.base.directory.clone(), src, dst).await
    }
}

impl<DirectoryType: MutableDirectory> ConnectionCreator<DirectoryType>
    for MutableConnection<DirectoryType>
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

impl<DirectoryType: MutableDirectory> RequestHandler
    for Tokenizable<MutableConnection<DirectoryType>>
{
    type Request = Result<fio::DirectoryRequest, fidl::Error>;

    async fn handle_request(self: Pin<&mut Self>, request: Self::Request) -> ControlFlow<()> {
        if let Some(_guard) = self.base.scope.try_active_guard() {
            match request {
                Ok(request) => {
                    match MutableConnection::<DirectoryType>::handle_request(self, request).await {
                        Ok(ConnectionState::Alive) => ControlFlow::Continue(()),
                        Ok(ConnectionState::Closed) | Err(_) => ControlFlow::Break(()),
                    }
                }
                Err(_) => ControlFlow::Break(()),
            }
        } else {
            ControlFlow::Break(())
        }
    }
}

impl<DirectoryType: MutableDirectory> TokenInterface for MutableConnection<DirectoryType> {
    fn get_node(&self) -> Arc<dyn MutableDirectory> {
        self.base.directory.clone()
    }

    fn get_rights(&self) -> fio::Rights {
        self.base.options.rights
    }

    fn token_registry(&self) -> &TokenRegistry {
        self.base.scope.token_registry()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToObjectRequest;
    use crate::directory::dirents_sink;
    use crate::directory::entry::{EntryInfo, GetEntryInfo};
    use crate::directory::entry_container::{Directory, DirectoryWatcher};
    use crate::directory::traversal_position::TraversalPosition;
    use crate::node::Node;
    use fuchsia_sync::Mutex;
    use futures::future::BoxFuture;
    use std::any::Any;
    use std::future::ready;
    use std::sync::Weak;

    #[derive(Debug, PartialEq)]
    enum MutableDirectoryAction {
        Link { id: u32, path: String },
        Unlink { id: u32, name: String },
        Rename { id: u32, src_name: String, dst_dir: u32, dst_name: String },
        UpdateAttributes { id: u32, attributes: fio::MutableNodeAttributes },
        Sync,
        Close,
    }

    #[derive(Debug)]
    struct MockDirectory {
        id: u32,
        fs: Arc<MockFilesystem>,
    }

    impl MockDirectory {
        pub fn new(id: u32, fs: Arc<MockFilesystem>) -> Arc<Self> {
            Arc::new(MockDirectory { id, fs })
        }
    }

    impl PartialEq for MockDirectory {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }

    impl GetEntryInfo for MockDirectory {
        fn entry_info(&self) -> EntryInfo {
            EntryInfo::new(0, fio::DirentType::Directory)
        }
    }

    impl Node for MockDirectory {
        async fn get_attributes(
            &self,
            _query: fio::NodeAttributesQuery,
        ) -> Result<fio::NodeAttributes2, Status> {
            unimplemented!("Not implemented");
        }

        fn close(self: Arc<Self>) {
            let _ = self.fs.handle_event(MutableDirectoryAction::Close);
        }
    }

    impl Directory for MockDirectory {
        fn open(
            self: Arc<Self>,
            _scope: ExecutionScope,
            _path: Path,
            _flags: fio::Flags,
            _object_request: ObjectRequestRef<'_>,
        ) -> Result<(), Status> {
            unimplemented!("Not implemented!");
        }

        async fn read_dirents(
            &self,
            _pos: &TraversalPosition,
            _sink: Box<dyn dirents_sink::Sink>,
        ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), Status> {
            unimplemented!("Not implemented");
        }

        fn register_watcher(
            self: Arc<Self>,
            _scope: ExecutionScope,
            _mask: fio::WatchMask,
            _watcher: DirectoryWatcher,
        ) -> Result<(), Status> {
            unimplemented!("Not implemented");
        }

        fn unregister_watcher(self: Arc<Self>, _key: usize) {
            unimplemented!("Not implemented");
        }
    }

    impl MutableDirectory for MockDirectory {
        fn link<'a>(
            self: Arc<Self>,
            path: String,
            _source_dir: Arc<dyn Any + Send + Sync>,
            _source_name: &'a str,
        ) -> BoxFuture<'a, Result<(), Status>> {
            let result = self.fs.handle_event(MutableDirectoryAction::Link { id: self.id, path });
            Box::pin(ready(result))
        }

        async fn unlink(
            self: Arc<Self>,
            name: &str,
            _must_be_directory: bool,
        ) -> Result<(), Status> {
            self.fs.handle_event(MutableDirectoryAction::Unlink {
                id: self.id,
                name: name.to_string(),
            })
        }

        async fn update_attributes(
            &self,
            attributes: fio::MutableNodeAttributes,
        ) -> Result<(), Status> {
            self.fs
                .handle_event(MutableDirectoryAction::UpdateAttributes { id: self.id, attributes })
        }

        async fn sync(&self) -> Result<(), Status> {
            self.fs.handle_event(MutableDirectoryAction::Sync)
        }

        fn rename(
            self: Arc<Self>,
            src_dir: Arc<dyn MutableDirectory>,
            src_name: Path,
            dst_name: Path,
        ) -> BoxFuture<'static, Result<(), Status>> {
            let src_dir = src_dir.into_any().downcast::<MockDirectory>().unwrap();
            let result = self.fs.handle_event(MutableDirectoryAction::Rename {
                id: src_dir.id,
                src_name: src_name.into_string(),
                dst_dir: self.id,
                dst_name: dst_name.into_string(),
            });
            Box::pin(ready(result))
        }
    }

    struct Events(Mutex<Vec<MutableDirectoryAction>>);

    impl Events {
        fn new() -> Arc<Self> {
            Arc::new(Events(Mutex::new(vec![])))
        }
    }

    struct MockFilesystem {
        cur_id: Mutex<u32>,
        scope: ExecutionScope,
        events: Weak<Events>,
    }

    impl MockFilesystem {
        pub fn new(events: &Arc<Events>) -> Self {
            #[cfg(feature = "fdomain")]
            let scope =
                crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
            #[cfg(not(feature = "fdomain"))]
            let scope = crate::execution_scope::ExecutionScope::new();
            MockFilesystem { cur_id: Mutex::new(0), scope, events: Arc::downgrade(events) }
        }

        pub fn handle_event(&self, event: MutableDirectoryAction) -> Result<(), Status> {
            self.events.upgrade().map(|x| x.0.lock().push(event));
            Ok(())
        }

        pub fn make_connection(
            self: &Arc<Self>,
            flags: fio::Flags,
        ) -> (Arc<MockDirectory>, fio::DirectoryProxy) {
            let mut cur_id = self.cur_id.lock();
            let dir = MockDirectory::new(*cur_id, self.clone());
            *cur_id += 1;
            let (proxy, server_end) = self.scope.domain().create_proxy::<fio::DirectoryMarker>();
            flags.to_object_request(server_end).create_connection_sync::<MutableConnection<_>, _>(
                self.scope.clone(),
                dir.clone(),
                flags,
            );
            (dir, proxy)
        }
    }

    impl std::fmt::Debug for MockFilesystem {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockFilesystem").field("cur_id", &self.cur_id).finish()
        }
    }

    #[cfg(not(feature = "fdomain"))]
    #[fuchsia::test]
    async fn test_rename() {
        let events = Events::new();
        let fs = Arc::new(MockFilesystem::new(&events));

        let (_dir, proxy) = fs.clone().make_connection(fio::PERM_READABLE | fio::PERM_WRITABLE);
        let (dir2, proxy2) = fs.clone().make_connection(fio::PERM_READABLE | fio::PERM_WRITABLE);

        let (status, token) = proxy2.get_token().await.unwrap();
        assert_eq!(status, zx_status::sys::ZX_OK);

        let status = proxy.rename("src", token.unwrap().into(), "dest").await.unwrap();
        assert!(status.is_ok());

        let events = events.0.lock();
        assert_eq!(
            *events,
            vec![MutableDirectoryAction::Rename {
                id: 0,
                src_name: "src".to_owned(),
                dst_dir: dir2.id,
                dst_name: "dest".to_owned(),
            },]
        );
    }

    #[fuchsia::test]
    async fn test_update_attributes() {
        let events = Events::new();
        let fs = Arc::new(MockFilesystem::new(&events));
        let (_dir, proxy) = fs.clone().make_connection(fio::PERM_READABLE | fio::PERM_WRITABLE);
        let attributes = fio::MutableNodeAttributes {
            creation_time: Some(30),
            modification_time: Some(100),
            mode: Some(200),
            ..Default::default()
        };
        proxy
            .update_attributes(&attributes)
            .await
            .expect("FIDL call failed")
            .map_err(Status::err_from_raw)
            .expect("update attributes failed");

        let events = events.0.lock();
        assert_eq!(*events, vec![MutableDirectoryAction::UpdateAttributes { id: 0, attributes }]);
    }

    #[cfg(not(feature = "fdomain"))]
    #[fuchsia::test]
    async fn test_link() {
        let events = Events::new();
        let fs = Arc::new(MockFilesystem::new(&events));
        let (_dir, proxy) = fs.clone().make_connection(fio::PERM_READABLE | fio::PERM_WRITABLE);
        let (_dir2, proxy2) = fs.clone().make_connection(fio::PERM_READABLE | fio::PERM_WRITABLE);

        let (status, token) = proxy2.get_token().await.unwrap();
        assert_eq!(status, zx_status::sys::ZX_OK);

        let status = proxy.link("src", token.unwrap(), "dest").await.unwrap();
        assert_eq!(status, zx_status::sys::ZX_OK);
        let events = events.0.lock();
        assert_eq!(*events, vec![MutableDirectoryAction::Link { id: 1, path: "dest".to_owned() },]);
    }

    #[fuchsia::test]
    async fn test_unlink() {
        let events = Events::new();
        let fs = Arc::new(MockFilesystem::new(&events));
        let (_dir, proxy) = fs.clone().make_connection(fio::PERM_READABLE | fio::PERM_WRITABLE);
        proxy
            .unlink("test", &fio::UnlinkOptions::default())
            .await
            .expect("fidl call failed")
            .expect("unlink failed");
        let events = events.0.lock();
        assert_eq!(
            *events,
            vec![MutableDirectoryAction::Unlink { id: 0, name: "test".to_string() },]
        );
    }

    #[fuchsia::test]
    async fn test_sync() {
        let events = Events::new();
        let fs = Arc::new(MockFilesystem::new(&events));
        let (_dir, proxy) = fs.clone().make_connection(fio::PERM_READABLE | fio::PERM_WRITABLE);
        let () = proxy.sync().await.unwrap().map_err(Status::err_from_raw).unwrap();
        let events = events.0.lock();
        assert_eq!(*events, vec![MutableDirectoryAction::Sync]);
    }

    #[fuchsia::test]
    async fn test_close() {
        let events = Events::new();
        let fs = Arc::new(MockFilesystem::new(&events));
        let (_dir, proxy) = fs.clone().make_connection(fio::PERM_READABLE | fio::PERM_WRITABLE);
        let () = proxy.close().await.unwrap().map_err(Status::err_from_raw).unwrap();
        let events = events.0.lock();
        assert_eq!(*events, vec![MutableDirectoryAction::Close]);
    }

    #[fuchsia::test]
    async fn test_implicit_close() {
        let events = Events::new();
        let fs = Arc::new(MockFilesystem::new(&events));
        let (_dir, _proxy) = fs.clone().make_connection(fio::PERM_READABLE | fio::PERM_WRITABLE);

        fs.scope.shutdown();
        fs.scope.wait().await;

        let events = events.0.lock();
        assert_eq!(*events, vec![MutableDirectoryAction::Close]);
    }
}
