// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::ProtectionFlags;
use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
use crate::task::{CurrentTask, Kernel};
use crate::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
use crate::vfs::{
    DirectoryEntryType, DirectoryMode, DirentSink, FileHandle, FileObject, FsStr, FsString,
    LookupContext, NamespaceNode, OpenAccessCheck, RenameFlags, SeekTarget, UnlinkKind,
};
use fidl::endpoints::{ClientEnd, ServerEnd};
use fidl_fuchsia_io as fio;
use fuchsia_runtime::UtcInstant;
use futures::StreamExt;
use futures::future::BoxFuture;
use itertools::Either;
use starnix_logging::{log_error, track_stub};
use starnix_sync::{FileServerStatsLock, LockDepMutex};
use starnix_types::convert::IntoFidl as _;
use starnix_uapi::auth::Credentials;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::{ENOSPC, Errno};
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::ResolveFlags;
use starnix_uapi::{errno, error, from_status_like_fdio, ino_t, off_t};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use vfs::directory::mutable::connection::MutableConnection;
use vfs::directory::{self};
use vfs::{
    ObjectRequestRef, ProtocolsExt, ToObjectRequest, attributes, execution_scope, file, path,
};

#[derive(Default)]
struct FileServerStats {
    /// The number of objects currently being served.  This will not count multiple connections to
    /// the same object, or, for a directory, connections to any of its children.
    serving: AtomicU64,

    /// The total number of reads performed for files served.
    reads: AtomicU64,

    /// The total number of bytes read for files served.
    read_bytes: AtomicU64,

    /// The total number of writes performed for files served.
    writes: AtomicU64,

    /// The total number of writes written for files served.
    write_bytes: AtomicU64,
}

struct FileServerRegistry {
    stats: LockDepMutex<HashMap<&'static str, Arc<FileServerStats>>, FileServerStatsLock>,
}

impl FileServerRegistry {
    fn get(kernel: &Kernel) -> Arc<Self> {
        let mut is_new = false;
        let registry = kernel.expando.get_or_init(|| {
            is_new = true;
            Self { stats: Default::default() }
        });
        if is_new {
            let registry_weak = Arc::downgrade(&registry);
            kernel.inspect_node.record_lazy_child("file_server", move || {
                let inspector = fuchsia_inspect::Inspector::default();
                if let Some(registry) = registry_weak.upgrade() {
                    let root = inspector.root();
                    for (tag, stats) in registry.stats.lock().iter() {
                        let node = root.create_child(*tag);
                        node.record_uint("serving", stats.serving.load(Ordering::Relaxed));
                        node.record_uint("reads", stats.reads.load(Ordering::Relaxed));
                        node.record_uint("read_bytes", stats.read_bytes.load(Ordering::Relaxed));
                        node.record_uint("writes", stats.writes.load(Ordering::Relaxed));
                        node.record_uint("write_bytes", stats.write_bytes.load(Ordering::Relaxed));
                        root.record(node);
                    }
                }
                Box::pin(async { Ok(inspector) })
            });
        }
        registry
    }

    fn get_stats(&self, tag: &'static str) -> Arc<FileServerStats> {
        self.stats.lock().entry(tag).or_insert_with(|| Arc::default()).clone()
    }
}

pub fn serve_file_tagged(
    current_task: &CurrentTask,
    file: &FileObject,
    credentials: Arc<Credentials>,
    tag: &'static str,
) -> Result<(ClientEnd<fio::NodeMarker>, execution_scope::ExecutionScope), Errno> {
    let (client_end, server_end) = fidl::endpoints::create_endpoints::<fio::NodeMarker>();
    let scope = serve_file_at_tagged(server_end, current_task, file, credentials, tag)?;
    Ok((client_end, scope))
}

/// Returns a handle implementing a fuchsia.io.Node delegating to the given `file`.
pub fn serve_file(
    current_task: &CurrentTask,
    file: &FileObject,
    credentials: Arc<Credentials>,
) -> Result<(ClientEnd<fio::NodeMarker>, execution_scope::ExecutionScope), Errno> {
    serve_file_tagged(current_task, file, credentials, "default")
}

pub fn serve_file_at_tagged(
    server_end: ServerEnd<fio::NodeMarker>,
    current_task: &CurrentTask,
    file: &FileObject,
    credentials: Arc<Credentials>,
    tag: &'static str,
) -> Result<execution_scope::ExecutionScope, Errno> {
    let kernel = current_task.kernel();
    let stats = FileServerRegistry::get(&kernel).get_stats(tag);
    // The TRUNC flag needs to be stripped as otherwise the VFS library will try and truncate
    // the file when it creates the connection.
    let fidl_flags: fio::OpenFlags = (file.flags() & !OpenFlags::TRUNC).into_fidl();
    let starnix_file = StarnixNodeConnection::new(
        &kernel,
        file.weak_handle.upgrade().unwrap(),
        credentials,
        stats.clone(),
    );
    let scope = execution_scope::ExecutionScope::new();
    kernel.kthreads.spawn_future(
        {
            let scope = scope.clone();
            move || async move {
                stats.serving.fetch_add(1, Ordering::Relaxed);
                if starnix_file.is_dir() {
                    fidl_flags.to_object_request(server_end).handle(|object_request| {
                        object_request.take().create_connection_sync::<MutableConnection<_>, _>(
                            scope.clone(),
                            starnix_file,
                            fidl_flags,
                        );
                        Ok(())
                    });
                } else {
                    fidl_flags.to_object_request(server_end).handle(|object_request| {
                        object_request
                            .take()
                            .create_connection_sync::<file::RawIoConnection<_>, _>(
                                scope.clone(),
                                starnix_file,
                                fidl_flags,
                            );
                        Ok(())
                    });
                }
                scope.wait().await;
                stats.serving.fetch_sub(1, Ordering::Relaxed);
            }
        },
        "serve_file_at",
    );
    Ok(scope)
}

pub fn serve_file_at(
    server_end: ServerEnd<fio::NodeMarker>,
    current_task: &CurrentTask,
    file: &FileObject,
    credentials: Arc<Credentials>,
) -> Result<execution_scope::ExecutionScope, Errno> {
    serve_file_at_tagged(server_end, current_task, file, credentials, "default")
}

#[async_trait::async_trait(?Send)]
trait Work: Send + 'static {
    async fn run(self: Box<Self>, current_task: &CurrentTask, file: &FileHandle);
}

struct EitherSender<R>(Either<futures::channel::oneshot::Sender<R>, std::sync::mpsc::Sender<R>>);

impl<R> From<futures::channel::oneshot::Sender<R>> for EitherSender<R> {
    fn from(v: futures::channel::oneshot::Sender<R>) -> Self {
        Self(Either::Left(v))
    }
}

impl<R> From<std::sync::mpsc::Sender<R>> for EitherSender<R> {
    fn from(v: std::sync::mpsc::Sender<R>) -> Self {
        Self(Either::Right(v))
    }
}

impl<R> EitherSender<R> {
    async fn send(self, r: R) {
        match self.0 {
            Either::Left(s) => {
                let _ = s.send(r);
            }
            Either::Right(s) => {
                let _ = s.send(r);
            }
        }
    }
}

struct WorkWrapper<R, F>
where
    R: Send + 'static,
    F: AsyncFnOnce(&CurrentTask, &FileHandle) -> R + Send + 'static,
{
    f: F,
    sender: EitherSender<R>,
}

#[async_trait::async_trait(?Send)]
impl<R, F> Work for WorkWrapper<R, F>
where
    R: Send + 'static,
    F: AsyncFnOnce(&CurrentTask, &FileHandle) -> R + Send + 'static,
{
    async fn run(self: Box<Self>, current_task: &CurrentTask, file: &FileHandle) {
        let f: F = self.f;
        let r = f(current_task, file).await;
        self.sender.send(r).await;
    }
}

async fn handle_file(
    current_task: &CurrentTask,
    credentials: Arc<Credentials>,
    file: FileHandle,
    mut receiver: futures::channel::mpsc::UnboundedReceiver<Box<dyn Work>>,
) {
    // Run with the correct credentials
    current_task
        .override_creds_async(credentials.clone(), async || {
            // Reopen file object to not share state with the given FileObject.
            let file = match file.name.open(current_task, OpenAccessCheck::skip(file.flags())) {
                Ok(file) => file,
                Err(e) => {
                    log_error!("Unable to reopen file: {e:?}");
                    return;
                }
            };
            while let Some(w) = receiver.next().await {
                w.run(current_task, &file).await;
            }
        })
        .await;
}

fn to_open_flags(flags: &impl ProtocolsExt) -> OpenFlags {
    let rights = flags.rights().unwrap_or_default();
    let mut open_flags = if rights.contains(fio::Operations::WRITE_BYTES) {
        if rights.contains(fio::Operations::READ_BYTES) {
            OpenFlags::RDWR
        } else {
            OpenFlags::WRONLY
        }
    } else {
        OpenFlags::RDONLY
    };

    if flags.create_directory() {
        open_flags |= OpenFlags::DIRECTORY;
    }

    match flags.creation_mode() {
        vfs::CreationMode::Always => open_flags |= OpenFlags::CREAT | OpenFlags::EXCL,
        vfs::CreationMode::AllowExisting => open_flags |= OpenFlags::CREAT,
        vfs::CreationMode::UnnamedTemporary => open_flags |= OpenFlags::TMPFILE,
        vfs::CreationMode::UnlinkableUnnamedTemporary => {
            open_flags |= OpenFlags::TMPFILE | OpenFlags::EXCL
        }
        vfs::CreationMode::Never => {}
    };

    if flags.is_truncate() {
        open_flags |= OpenFlags::TRUNC;
    }

    if flags.is_append() {
        open_flags |= OpenFlags::APPEND;
    }

    open_flags
}

/// A representation of `file` for the rust vfs.
///
/// This struct implements the following trait from the rust vfs library:
/// - directory::entry_container::Directory
/// - directory::entry_container::MutableDirectory
/// - file::File
/// - file::RawFileIoConnection
///
/// Each method is delegated back to the starnix vfs, using `task` as the current task. Blocking
/// methods are run from the kernel dynamic thread spawner so that the async dispatched do not
/// block on these.
/// All vfs operations should be done using `credentials`.
#[derive(Clone)]
struct StarnixNodeConnection {
    is_dir: bool,
    credentials: Arc<Credentials>,
    work_sender: futures::channel::mpsc::UnboundedSender<Box<dyn Work>>,
    stats: Arc<FileServerStats>,
}

fn lookup_parent(
    current_task: &CurrentTask,
    file: &FileObject,
    path: path::Path,
) -> Result<(NamespaceNode, FsString), Errno> {
    let (node, name) = current_task.lookup_parent(
        &mut LookupContext::default(),
        &file.name,
        path.as_str().into(),
    )?;
    Ok((node, name.to_owned()))
}

impl StarnixNodeConnection {
    fn new(
        kernel: &Kernel,
        file: FileHandle,
        credentials: Arc<Credentials>,
        stats: Arc<FileServerStats>,
    ) -> Arc<Self> {
        let (work_sender, receiver) = futures::channel::mpsc::unbounded();
        let is_dir = file.node().is_dir();
        let closure = {
            let credentials = credentials.clone();
            async move |current_task: &CurrentTask| {
                handle_file(current_task, credentials, file, receiver).await;
            }
        };
        let req = SpawnRequestBuilder::new().with_async_closure(closure).build();
        kernel.kthreads.spawner().spawn_from_request(req);
        Arc::new(Self { is_dir, credentials, work_sender, stats })
    }

    fn spawn_task<R, E, F>(&self, f: F) -> Result<R, Errno>
    where
        R: Send + 'static,
        E: Send + 'static,
        F: AsyncFnOnce(&CurrentTask, &FileHandle) -> Result<R, E> + Send + 'static,
        Errno: From<E>,
    {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.work_sender
            .unbounded_send(Box::new(WorkWrapper { f, sender: sender.into() }))
            .map_err(|_| errno!(EIO))?;
        Ok(receiver.recv().map_err(|_| errno!(EIO))??)
    }

    async fn spawn_task_async<R, E, F>(&self, f: F) -> Result<R, Errno>
    where
        R: Send + 'static,
        E: Send + 'static,
        F: AsyncFnOnce(&CurrentTask, &FileHandle) -> Result<R, E> + Send + 'static,
        Errno: From<E>,
    {
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.work_sender
            .unbounded_send(Box::new(WorkWrapper { f, sender: sender.into() }))
            .map_err(|_| errno!(EIO))?;
        Ok(receiver.await.map_err(|_| errno!(EIO))??)
    }

    fn is_dir(&self) -> bool {
        self.is_dir
    }

    fn lookup_parent(&self, path: path::Path) -> Result<(NamespaceNode, FsString), Errno> {
        self.spawn_task(async move |current_task, file| lookup_parent(current_task, file, path))
    }

    /// Reopen the current `StarnixNodeConnection` with the given `OpenFlags`. The new file will not share
    /// state. It is equivalent to opening the same file, not dup'ing the file descriptor.
    fn reopen(&self, flags: &impl ProtocolsExt) -> Result<Arc<Self>, Errno> {
        let credentials = self.credentials.clone();
        let flags = to_open_flags(flags);
        let stats = self.stats.clone();
        self.spawn_task(async move |current_task, file| {
            let file = file.name.open(current_task, flags)?;
            Ok(StarnixNodeConnection::new(&current_task.kernel(), file, credentials, stats))
        })
    }

    /// Implementation of `vfs::directory::entry_container::Directory::directory_read_dirents`.
    fn directory_read_dirents<'a>(
        &'a self,
        pos: &'a directory::traversal_position::TraversalPosition,
        sink: Box<dyn directory::dirents_sink::Sink>,
    ) -> Result<
        (
            directory::traversal_position::TraversalPosition,
            Box<dyn directory::dirents_sink::Sealed>,
        ),
        Errno,
    > {
        let pos = pos.clone();
        self.spawn_task(async move |current_task, file| {
            struct DirentSinkAdapter<'a> {
                sink: Option<directory::dirents_sink::AppendResult>,
                offset: &'a mut off_t,
            }
            impl<'a> DirentSinkAdapter<'a> {
                fn is_sealed(&self) -> bool {
                    matches!(self.sink, Some(directory::dirents_sink::AppendResult::Sealed(_)))
                }

                fn append(
                    &mut self,
                    entry: &directory::entry::EntryInfo,
                    name: &str,
                ) -> Result<(), Errno> {
                    // We must take `self.sink` here because the Fuchsia VFS `Sink::append` method
                    // (called on the inner sink below) consumes the sink by value.
                    let sink = self.sink.take();
                    match sink {
                        s @ Some(directory::dirents_sink::AppendResult::Sealed(_)) => {
                            self.sink = s;
                            error!(ENOSPC)
                        }
                        Some(directory::dirents_sink::AppendResult::Ok(sink)) => {
                            self.sink = Some(sink.append(entry, name));
                            if self.is_sealed() {
                                // The sink is sealed and the entry did not fit.
                                //
                                // This function is called by `readdir` to iterate through directory
                                // entries and stops when all entries have been read or if this
                                // returns an error. By returning `ENOSPC` here, the `readdir`
                                // loop will halt before it advances to the next entry. This ensures
                                // that the entry that failed to fit is not skipped, and will be
                                // the first one processed when the client resumes reading.
                                error!(ENOSPC)
                            } else {
                                Ok(())
                            }
                        }
                        None => error!(ENOTSUP),
                    }
                }
            }
            impl<'a> DirentSink for DirentSinkAdapter<'a> {
                fn add(
                    &mut self,
                    inode_num: ino_t,
                    offset: off_t,
                    entry_type: DirectoryEntryType,
                    name: &FsStr,
                ) -> Result<(), Errno> {
                    // Ignore ..
                    if name != ".." {
                        // Ignore entries with unknown types.
                        if let Some(dirent_type) =
                            fio::DirentType::from_primitive(entry_type.bits())
                        {
                            let entry_info =
                                directory::entry::EntryInfo::new(inode_num, dirent_type);
                            self.append(&entry_info, &String::from_utf8_lossy(name))?
                        }
                    }
                    *self.offset = offset;
                    Ok(())
                }
                fn offset(&self) -> off_t {
                    *self.offset
                }
            }
            let offset = match pos {
                directory::traversal_position::TraversalPosition::Start => 0,
                directory::traversal_position::TraversalPosition::Index(v) => v as i64,
                directory::traversal_position::TraversalPosition::End => {
                    return Ok((
                        directory::traversal_position::TraversalPosition::End,
                        sink.seal(),
                    ));
                }
                _ => return error!(EINVAL),
            };
            if file.offset.read() != offset {
                file.seek(current_task, SeekTarget::Set(offset))?;
            }
            let mut file_offset = file.offset.copy();
            let sink_result = {
                let mut dirent_sink = DirentSinkAdapter {
                    sink: Some(directory::dirents_sink::AppendResult::Ok(sink)),
                    offset: &mut *file_offset,
                };
                match file.readdir(current_task, &mut dirent_sink) {
                    Ok(()) => {}
                    Err(err) if err == ENOSPC => {
                        // We caught ENOSPC. We must distinguish between:
                        // 1. ENOSPC sent when sink is sealed: This is expected when the buffer
                        //    fills up. We ignore the error and return the partial results.
                        // 2. A genuine filesystem error (sink is NOT sealed): This is a real
                        //    failure, so we must propagate it.
                        if !dirent_sink.is_sealed() {
                            return Err(err);
                        }
                    }
                    Err(err) => return Err(err),
                }
                dirent_sink.sink
            };
            let ret = match sink_result {
                Some(directory::dirents_sink::AppendResult::Sealed(seal)) => Ok((
                    directory::traversal_position::TraversalPosition::Index(*file_offset as u64),
                    seal,
                )),
                Some(directory::dirents_sink::AppendResult::Ok(sink)) => {
                    Ok((directory::traversal_position::TraversalPosition::End, sink.seal()))
                }
                None => error!(ENOTSUP),
            };
            file_offset.update();
            ret
        })
    }

    /// Implementation of `vfs::directory::entry::DirectoryEntry::open`.
    fn directory_entry_open(
        self: Arc<Self>,
        scope: execution_scope::ExecutionScope,
        flags: impl ProtocolsExt,
        path: path::Path,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        if self.is_dir() {
            if path.is_dot() {
                // Reopen the current directory.
                let dir = self.reopen(&flags)?;
                object_request
                    .take()
                    .create_connection_sync::<MutableConnection<_>, _>(scope, dir, flags);
                return Ok(());
            }

            // Open a path under the current directory.
            let starnix_file = self.spawn_task({
                let credentials = self.credentials.clone();
                let stats = self.stats.clone();
                let create_directory = flags.creation_mode() != vfs::common::CreationMode::Never
                    && flags.create_directory();
                let open_flags = to_open_flags(&flags);
                async move |current_task, file| {
                    let (node, name) = lookup_parent(current_task, file, path)?;
                    let file = match current_task.open_namespace_node_at(
                        node.clone(),
                        name.as_ref(),
                        open_flags,
                        FileMode::ALLOW_ALL,
                        ResolveFlags::empty(),
                    ) {
                        Err(e) if e == errno!(EISDIR) && create_directory => {
                            let mode = current_task
                                .fs()
                                .apply_umask(FileMode::from_bits(0o777) | FileMode::IFDIR);
                            let name = node.create_node(
                                &current_task,
                                name.as_ref(),
                                mode,
                                DeviceId::NONE,
                            )?;
                            name.open(
                                &current_task,
                                OpenAccessCheck::skip(
                                    open_flags & !(OpenFlags::CREAT | OpenFlags::EXCL),
                                ),
                            )?
                        }
                        f => f?,
                    };
                    Ok(StarnixNodeConnection::new(&current_task.kernel(), file, credentials, stats))
                }
            })?;

            return starnix_file.directory_entry_open(
                scope,
                flags,
                path::Path::dot(),
                object_request,
            );
        }

        // Reopen the current file.
        if !path.is_dot() {
            return Err(zx::Status::NOT_DIR);
        }
        let file = self.reopen(&flags)?;
        object_request
            .take()
            .create_connection_sync::<file::RawIoConnection<_>, _>(scope, file, flags);
        Ok(())
    }

    fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> fio::NodeAttributes2 {
        self.spawn_task(async move |_, file| {
            let info = file.node().info();

            // This cast is necessary depending on the architecture.
            #[allow(clippy::unnecessary_cast)]
            let link_count = info.link_count as u64;

            let (protocols, abilities) = if info.mode.contains(FileMode::IFDIR) {
                (
                    fio::NodeProtocolKinds::DIRECTORY,
                    fio::Operations::GET_ATTRIBUTES
                        | fio::Operations::UPDATE_ATTRIBUTES
                        | fio::Operations::ENUMERATE
                        | fio::Operations::TRAVERSE
                        | fio::Operations::MODIFY_DIRECTORY,
                )
            } else {
                (
                    fio::NodeProtocolKinds::FILE,
                    fio::Operations::GET_ATTRIBUTES
                        | fio::Operations::UPDATE_ATTRIBUTES
                        | fio::Operations::READ_BYTES
                        | fio::Operations::WRITE_BYTES,
                )
            };

            Ok(attributes!(
                requested_attributes,
                Mutable {
                    creation_time: info.time_status_change.into_nanos() as u64,
                    modification_time: info.time_modify.into_nanos() as u64,
                    mode: info.mode.bits(),
                    uid: info.uid,
                    gid: info.gid,
                    rdev: info.rdev.bits(),
                },
                Immutable {
                    protocols: protocols,
                    abilities: abilities,
                    content_size: info.size as u64,
                    storage_size: info.storage_size() as u64,
                    link_count: link_count,
                    id: file.fs.dev_id.bits(),
                }
            ))
        })
        .expect("spawn_task")
    }

    fn update_attributes(&self, attributes: fio::MutableNodeAttributes) {
        let _ = self.spawn_task(async move |_, file| {
            file.node().update_info(|info| {
                if let Some(time) = attributes.creation_time {
                    info.time_status_change = UtcInstant::from_nanos(time as i64);
                }
                if let Some(time) = attributes.modification_time {
                    info.time_modify = UtcInstant::from_nanos(time as i64);
                }
                if let Some(mode) = attributes.mode {
                    info.mode = FileMode::from_bits(mode);
                }
                if let Some(uid) = attributes.uid {
                    info.uid = uid;
                }
                if let Some(gid) = attributes.gid {
                    info.gid = gid;
                }
                if let Some(rdev) = attributes.rdev {
                    info.rdev = DeviceId::from_bits(rdev);
                }
            });
            Ok(())
        });
    }
}

impl vfs::node::Node for StarnixNodeConnection {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        Ok(StarnixNodeConnection::get_attributes(self, requested_attributes))
    }
}

impl directory::entry::GetEntryInfo for StarnixNodeConnection {
    fn entry_info(&self) -> directory::entry::EntryInfo {
        let dirent_type =
            if self.is_dir() { fio::DirentType::Directory } else { fio::DirentType::File };
        directory::entry::EntryInfo::new(0, dirent_type)
    }
}

impl directory::entry_container::Directory for StarnixNodeConnection {
    fn open(
        self: Arc<Self>,
        scope: execution_scope::ExecutionScope,
        path: path::Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        self.directory_entry_open(scope, flags, path, object_request)
    }

    async fn read_dirents(
        &self,
        pos: &directory::traversal_position::TraversalPosition,
        sink: Box<dyn directory::dirents_sink::Sink>,
    ) -> Result<
        (
            directory::traversal_position::TraversalPosition,
            Box<dyn directory::dirents_sink::Sealed>,
        ),
        zx::Status,
    > {
        StarnixNodeConnection::directory_read_dirents(self, pos, sink).map_err(Errno::into)
    }
    fn register_watcher(
        self: Arc<Self>,
        _scope: execution_scope::ExecutionScope,
        _mask: fio::WatchMask,
        _watcher: directory::entry_container::DirectoryWatcher,
    ) -> Result<(), zx::Status> {
        track_stub!(TODO("https://fxbug.dev/322875605"), "register directory watcher");
        Ok(())
    }
    fn unregister_watcher(self: Arc<Self>, _key: usize) {}
}

impl directory::entry_container::MutableDirectory for StarnixNodeConnection {
    async fn update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> Result<(), zx::Status> {
        StarnixNodeConnection::update_attributes(self, attributes);
        Ok(())
    }
    async fn unlink(
        self: Arc<Self>,
        name: &str,
        must_be_directory: bool,
    ) -> Result<(), zx::Status> {
        let name = FsString::from(name.to_owned());
        self.spawn_task_async(async move |current_task, file| {
            let kind =
                if must_be_directory { UnlinkKind::Directory } else { UnlinkKind::NonDirectory };
            file.name.entry.unlink(
                current_task,
                &file.name.mount,
                name.as_ref(),
                kind,
                DirectoryMode::AllowAny,
            )
        })
        .await?;
        Ok(())
    }
    async fn sync(&self) -> Result<(), zx::Status> {
        Ok(())
    }
    fn rename(
        self: Arc<Self>,
        src_dir: Arc<dyn directory::entry_container::MutableDirectory>,
        src_name: path::Path,
        dst_name: path::Path,
    ) -> BoxFuture<'static, Result<(), zx::Status>> {
        let this = self.clone();
        Box::pin(async move {
            Ok(self
                .spawn_task_async(async move |current_task, file| {
                    let src_dir = src_dir
                        .into_any()
                        .downcast::<StarnixNodeConnection>()
                        .map_err(|_| errno!(EXDEV))?;
                    let (dst_node, dst_name) = lookup_parent(current_task, &file, dst_name)?;
                    let (src_node, src_name) = if Arc::ptr_eq(&src_dir, &this) {
                        lookup_parent(current_task, &file, src_name)?
                    } else {
                        src_dir.lookup_parent(src_name)?
                    };
                    NamespaceNode::rename(
                        current_task,
                        &src_node,
                        src_name.as_ref(),
                        &dst_node,
                        dst_name.as_ref(),
                        RenameFlags::empty(),
                    )
                })
                .await?)
        })
    }
}

impl file::File for StarnixNodeConnection {
    fn writable(&self) -> bool {
        true
    }
    async fn open_file(&self, _optionss: &file::FileOptions) -> Result<(), zx::Status> {
        Ok(())
    }
    async fn truncate(&self, length: u64) -> Result<(), zx::Status> {
        Ok(self
            .spawn_task_async(async move |current_task, file| {
                // `ftruncate` checks fewer permissions than `file.name.truncate`, which is what we
                // want.
                file.ftruncate(current_task, length)
            })
            .await?)
    }
    async fn get_backing_memory(&self, flags: fio::VmoFlags) -> Result<zx::Vmo, zx::Status> {
        Ok(self
            .spawn_task_async(async move |current_task, file| {
                (|| {
                    let mut prot_flags = ProtectionFlags::empty();
                    if flags.contains(fio::VmoFlags::READ) {
                        prot_flags |= ProtectionFlags::READ;
                    }
                    if flags.contains(fio::VmoFlags::WRITE) {
                        prot_flags |= ProtectionFlags::WRITE;
                    }
                    if flags.contains(fio::VmoFlags::EXECUTE) {
                        prot_flags |= ProtectionFlags::EXEC;
                    }
                    let memory = file.get_memory(current_task, None, prot_flags)?;
                    let vmo = memory.as_vmo().ok_or(zx::Status::NOT_SUPPORTED)?;
                    if flags.contains(fio::VmoFlags::PRIVATE_CLONE) {
                        let size = vmo.get_size()?;
                        vmo.create_child(zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, size)
                    } else {
                        vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
                    }
                })()
                .map_err(|e| from_status_like_fdio!(e))
            })
            .await?)
    }

    async fn get_size(&self) -> Result<u64, zx::Status> {
        Ok(self.spawn_task_async(async move |_, file| Ok(file.node().info().size as u64)).await?)
    }
    async fn update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> Result<(), zx::Status> {
        StarnixNodeConnection::update_attributes(self, attributes);
        Ok(())
    }
    async fn sync(&self, _mode: file::SyncMode) -> Result<(), zx::Status> {
        Ok(())
    }
}

impl file::RawFileIoConnection for StarnixNodeConnection {
    async fn read(&self, count: u64) -> Result<Vec<u8>, zx::Status> {
        self.stats.reads.fetch_add(1, Ordering::Relaxed);
        let data: Vec<u8> = self
            .spawn_task_async(async move |current_task, file| {
                let mut data = VecOutputBuffer::new(count as usize);
                file.read(current_task, &mut data)?;
                Ok(data.into())
            })
            .await?;
        self.stats.read_bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(data)
    }

    async fn read_at(&self, offset: u64, count: u64) -> Result<Vec<u8>, zx::Status> {
        self.stats.reads.fetch_add(1, Ordering::Relaxed);
        let data: Vec<u8> = self
            .spawn_task_async(async move |current_task, file| {
                let mut data = VecOutputBuffer::new(count as usize);
                file.read_at(current_task, offset as usize, &mut data)?;
                Ok(data.into())
            })
            .await?;
        self.stats.read_bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(data)
    }

    async fn write(&self, content: &[u8]) -> Result<u64, zx::Status> {
        self.stats.writes.fetch_add(1, Ordering::Relaxed);
        let mut data = VecInputBuffer::new(content);
        let written = self
            .spawn_task_async(async move |current_task, file| {
                let written = file.write(current_task, &mut data)?;
                Ok(written as u64)
            })
            .await?;
        self.stats.write_bytes.fetch_add(written, Ordering::Relaxed);
        Ok(written)
    }

    async fn write_at(&self, offset: u64, content: &[u8]) -> Result<u64, zx::Status> {
        self.stats.writes.fetch_add(1, Ordering::Relaxed);
        let mut data = VecInputBuffer::new(content);
        let written = self
            .spawn_task_async(async move |current_task, file| {
                let written = file.write_at(current_task, offset as usize, &mut data)?;
                Ok(written as u64)
            })
            .await?;
        self.stats.write_bytes.fetch_add(written as u64, Ordering::Relaxed);
        Ok(written)
    }

    async fn seek(&self, offset: i64, origin: fio::SeekOrigin) -> Result<u64, zx::Status> {
        let target = match origin {
            fio::SeekOrigin::Start => SeekTarget::Set(offset),
            fio::SeekOrigin::Current => SeekTarget::Cur(offset),
            fio::SeekOrigin::End => SeekTarget::End(offset),
        };
        Ok(self.spawn_task(async move |current_task, file| {
            let seek_result = file.seek(current_task, target)?;
            Ok(seek_result as u64)
        })?)
    }

    fn set_flags(&self, flags: fio::Flags) -> Result<(), zx::Status> {
        // Called on the connection via `fcntl(FSETFL, ...)`. fuchsia.io only supports `O_APPEND`
        // right now, and does not have equivalents for the following flags:
        //  - `O_ASYNC`
        //  - `O_DIRECT`
        //  - `O_NOATIME` (only allowed if caller's EUID is same as the file's UID)
        //  - `O_NONBLOCK`
        const SETTABLE_FLAGS_MASK: OpenFlags = OpenFlags::APPEND;
        let flags = if flags.contains(fio::Flags::FILE_APPEND) {
            OpenFlags::APPEND
        } else {
            OpenFlags::empty()
        };
        Ok(self.spawn_task(async move |_, file| {
            file.update_file_flags(flags, SETTABLE_FLAGS_MASK);
            Ok(())
        })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::tmpfs::TmpFs;
    use crate::testing::*;
    use crate::vfs::{FsString, Namespace};
    use starnix_uapi::auth::{Capabilities, Credentials};
    use std::collections::HashSet;
    use syncio::{Zxio, ZxioOpenOptions, zxio_node_attr_has_t};

    fn assert_directory_content(zxio: &Zxio, content: &[&[u8]]) {
        let expected = content.iter().map(|&x| FsString::from(x)).collect::<HashSet<_>>();
        let mut iterator = zxio.create_dirent_iterator().expect("iterator");
        iterator.rewind().expect("iterator");
        let found =
            iterator.map(|x| x.as_ref().expect("dirent").name.clone()).collect::<HashSet<_>>();
        assert_eq!(found, expected);
    }

    #[::fuchsia::test]
    async fn access_file_system() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs(&kernel);

            let file = &fs.root().open_anonymous(current_task, OpenFlags::RDWR).expect("open");
            let (root_handle, scope) =
                serve_file(current_task, file, Credentials::root()).expect("serve");

            // Capture information from the filesystem in the main thread. The filesystem must not be
            // transferred to the other thread.
            let fs_dev_id = fs.dev_id;
            std::thread::spawn(move || {
                let root_zxio = Zxio::create(root_handle.into_channel().into()).expect("create");

                assert_directory_content(&root_zxio, &[b"."]);
                // Check that one can reiterate from the start.
                assert_directory_content(&root_zxio, &[b"."]);

                let attrs = root_zxio
                    .attr_get(zxio_node_attr_has_t { id: true, ..Default::default() })
                    .expect("attr_get");
                assert_eq!(attrs.id, fs_dev_id.bits());

                let mut attrs = syncio::zxio_node_attributes_t::default();
                attrs.has.creation_time = true;
                attrs.has.modification_time = true;
                attrs.creation_time = 0;
                attrs.modification_time = 42;
                root_zxio.attr_set(&attrs).expect("attr_set");
                let attrs = root_zxio
                    .attr_get(zxio_node_attr_has_t {
                        creation_time: true,
                        modification_time: true,
                        ..Default::default()
                    })
                    .expect("attr_get");
                assert_eq!(attrs.creation_time, 0);
                assert_eq!(attrs.modification_time, 42);

                assert_eq!(
                    root_zxio
                        .open("foo", fio::PERM_READABLE | fio::PERM_WRITABLE, Default::default())
                        .expect_err("open"),
                    zx::Status::NOT_FOUND
                );
                let foo_zxio = root_zxio
                    .open(
                        "foo",
                        fio::PERM_READABLE
                            | fio::PERM_WRITABLE
                            | fio::Flags::FLAG_MAYBE_CREATE
                            | fio::Flags::PROTOCOL_FILE,
                        Default::default(),
                    )
                    .expect("zxio_open");
                assert_directory_content(&root_zxio, &[b".", b"foo"]);

                assert_eq!(foo_zxio.write(b"hello").expect("write"), 5);
                assert_eq!(foo_zxio.write_at(2, b"ch").expect("write_at"), 2);
                let mut buffer = [0; 7];
                assert_eq!(foo_zxio.read_at(2, &mut buffer).expect("read_at"), 3);
                assert_eq!(&buffer[..3], b"cho");
                assert_eq!(foo_zxio.seek(syncio::SeekOrigin::Start, 0).expect("seek"), 0);
                assert_eq!(foo_zxio.read(&mut buffer).expect("read"), 5);
                assert_eq!(&buffer[..5], b"hecho");

                let attrs = foo_zxio
                    .attr_get(zxio_node_attr_has_t { id: true, ..Default::default() })
                    .expect("attr_get");
                assert_eq!(attrs.id, fs_dev_id.bits());

                let mut attrs = syncio::zxio_node_attributes_t::default();
                attrs.has.creation_time = true;
                attrs.has.modification_time = true;
                attrs.creation_time = 0;
                attrs.modification_time = 42;
                foo_zxio.attr_set(&attrs).expect("attr_set");
                let attrs = foo_zxio
                    .attr_get(zxio_node_attr_has_t {
                        creation_time: true,
                        modification_time: true,
                        ..Default::default()
                    })
                    .expect("attr_get");
                assert_eq!(attrs.creation_time, 0);
                assert_eq!(attrs.modification_time, 42);

                assert_eq!(
                    root_zxio
                        .open(
                            "bar/baz",
                            fio::Flags::PROTOCOL_DIRECTORY
                                | fio::Flags::FLAG_MAYBE_CREATE
                                | fio::PERM_READABLE
                                | fio::PERM_WRITABLE,
                            Default::default(),
                        )
                        .expect_err("open"),
                    zx::Status::NOT_FOUND
                );

                let bar_zxio = root_zxio
                    .open(
                        "bar",
                        fio::Flags::PROTOCOL_DIRECTORY
                            | fio::Flags::FLAG_MAYBE_CREATE
                            | fio::PERM_READABLE
                            | fio::PERM_WRITABLE,
                        Default::default(),
                    )
                    .expect("open");
                let baz_zxio = root_zxio
                    .open(
                        "bar/baz",
                        fio::Flags::PROTOCOL_DIRECTORY
                            | fio::Flags::FLAG_MAYBE_CREATE
                            | fio::PERM_READABLE
                            | fio::PERM_WRITABLE,
                        Default::default(),
                    )
                    .expect("open");
                assert_directory_content(&root_zxio, &[b".", b"foo", b"bar"]);
                assert_directory_content(&bar_zxio, &[b".", b"baz"]);

                bar_zxio.rename("baz", &root_zxio, "quz").expect("rename");
                assert_directory_content(&bar_zxio, &[b"."]);
                assert_directory_content(&root_zxio, &[b".", b"foo", b"bar", b"quz"]);
                assert_directory_content(&baz_zxio, &[b"."]);
            })
            .join()
            .expect("join");
            scope.shutdown();
            scope.wait().await;
            // This ensures fs cannot be captures in the thread.
            std::mem::drop(fs);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn serve_file_strips_trunc() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs(&kernel);
            let ns = Namespace::new(fs);
            let root = ns.root();

            let file_node = root
                .create_node(
                    current_task,
                    b"test".into(),
                    FileMode::IFREG | FileMode::ALLOW_ALL,
                    DeviceId::NONE,
                )
                .expect("create_node");

            let file =
                file_node.open(current_task, OpenAccessCheck::skip(OpenFlags::RDWR)).expect("open");
            file.write(current_task, &mut VecInputBuffer::new(b"hello")).expect("write");

            // Reopen with O_TRUNC.
            let file_to_serve = current_task
                .open_namespace_node_at(
                    root,
                    b"test".into(),
                    OpenAccessCheck::skip(OpenFlags::RDWR | OpenFlags::TRUNC),
                    FileMode::default(),
                    ResolveFlags::default(),
                )
                .expect("open O_TRUNC");

            // Ensure it IS truncated by the open.
            assert_eq!(file_to_serve.node().fetch_and_refresh_info(current_task).unwrap().size, 0);

            // Write something so we can check if it gets truncated again.
            file_to_serve
                .write(current_task, &mut VecInputBuffer::new(b"world"))
                .expect("write world");
            assert_eq!(file_to_serve.node().info().size, 5);

            let (client_end, scope) =
                serve_file(current_task, &file_to_serve, Credentials::root()).expect("serve");

            fuchsia_async::unblock(|| {
                let zxio = Zxio::create(client_end.into_channel().into()).expect("create");
                let mut attr = syncio::zxio_node_attributes_t::default();
                attr.has.content_size = true;
                let attr = zxio.attr_get(attr.has).expect("attr_get");
                // If O_TRUNC was not stripped, the size would be 0 here.
                assert_eq!(attr.content_size, 5);
            })
            .await;

            scope.shutdown();
            scope.wait().await;
        })
        .await;
    }

    #[::fuchsia::test]
    async fn truncate_checks_fd_permissions() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs(&kernel);
            let ns = Namespace::new(fs);
            let root = ns.root();

            let file_node = root
                .create_node(
                    current_task,
                    "test".into(),
                    FileMode::IFREG | FileMode::IRWXU,
                    DeviceId::NONE,
                )
                .expect("create_node");

            let file =
                file_node.open(current_task, OpenAccessCheck::skip(OpenFlags::RDWR)).expect("open");
            file.write(current_task, &mut VecInputBuffer::new(b"hello")).expect("write");

            // Serve the file as different user.
            let (client_end, scope) = serve_file(
                current_task,
                &file,
                Arc::new(Credentials {
                    fsuid: 2000,
                    cap_effective: Capabilities::empty(),
                    ..Credentials::clone(&current_task.current_creds())
                }),
            )
            .expect("serve");

            fuchsia_async::unblock(move || {
                let zxio = Zxio::create(client_end.into_channel().into()).expect("create");
                // truncate should succeed because the FD is open for writing, even though the file
                // is being served with a different user.
                zxio.truncate(2).expect("truncate");

                let mut attr = syncio::zxio_node_attributes_t::default();
                attr.has.content_size = true;
                let attr = zxio.attr_get(attr.has).expect("attr_get");
                assert_eq!(attr.content_size, 2);
            })
            .await;

            scope.shutdown();
            scope.wait().await;
        })
        .await;
    }

    #[::fuchsia::test]
    async fn open() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs(&kernel);

            let file = &fs
                .root()
                .open_anonymous(current_task, OpenFlags::RDWR)
                .expect("open_anonymous failed");
            let (root_handle, scope) =
                serve_file(current_task, file, Credentials::root()).expect("serve_file failed");

            std::thread::spawn(move || {
                let root_zxio =
                    Zxio::create(root_handle.into_channel().into()).expect("zxio create failed");

                assert_directory_content(&root_zxio, &[b"."]);
                assert_eq!(
                    root_zxio
                        .open(
                            "foo",
                            fio::PERM_READABLE | fio::PERM_WRITABLE,
                            ZxioOpenOptions::default()
                        )
                        .expect_err("open3 passed unexpectedly"),
                    zx::Status::NOT_FOUND
                );
                root_zxio
                    .open(
                        "foo",
                        fio::Flags::PROTOCOL_FILE
                            | fio::PERM_READABLE
                            | fio::PERM_WRITABLE
                            | fio::Flags::FLAG_MUST_CREATE,
                        ZxioOpenOptions::default(),
                    )
                    .expect("open3 failed");
                assert_directory_content(&root_zxio, &[b".", b"foo"]);

                assert_eq!(
                    root_zxio
                        .open(
                            "bar/baz",
                            fio::Flags::PROTOCOL_DIRECTORY
                                | fio::PERM_READABLE
                                | fio::PERM_WRITABLE
                                | fio::Flags::FLAG_MUST_CREATE,
                            ZxioOpenOptions::default()
                        )
                        .expect_err("open3 passed unexpectedly"),
                    zx::Status::NOT_FOUND
                );
                let bar_zxio = root_zxio
                    .open(
                        "bar",
                        fio::Flags::PROTOCOL_DIRECTORY
                            | fio::PERM_READABLE
                            | fio::PERM_WRITABLE
                            | fio::Flags::FLAG_MUST_CREATE,
                        ZxioOpenOptions::default(),
                    )
                    .expect("open3 failed");
                root_zxio
                    .open(
                        "bar/baz",
                        fio::Flags::PROTOCOL_DIRECTORY
                            | fio::PERM_READABLE
                            | fio::PERM_WRITABLE
                            | fio::Flags::FLAG_MUST_CREATE,
                        ZxioOpenOptions::default(),
                    )
                    .expect("open3 failed");
                assert_directory_content(&root_zxio, &[b".", b"foo", b"bar"]);
                assert_directory_content(&bar_zxio, &[b".", b"baz"]);
            })
            .join()
            .expect("join");
            scope.shutdown();
            scope.wait().await;

            // This ensures fs cannot be captured in the thread.
            std::mem::drop(fs);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn use_credentials() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs(&kernel);

            let file = &fs
                .root()
                .open_anonymous(current_task, OpenFlags::RDWR)
                .expect("open_anonymous failed");
            // Create a file as root.
            let ns = Namespace::new(fs);
            ns.root()
                .open_create_node(
                    current_task,
                    "test".into(),
                    FileMode::from_bits(0o600) | FileMode::IFREG,
                    DeviceId::NONE,
                    OpenFlags::empty(),
                )
                .expect("open_create_node failed");

            let mut creds = Credentials::with_ids(0, 0);
            creds.fsuid = 1;
            creds.cap_effective = Capabilities::empty();

            let (root_handle, scope) =
                serve_file(current_task, file, creds.into()).expect("serve_file failed");

            std::thread::spawn(move || {
                let root_zxio =
                    Zxio::create(root_handle.into_channel().into()).expect("zxio create failed");

                assert_directory_content(&root_zxio, &[b".", b"test"]);
                assert_eq!(
                    root_zxio
                        .open(
                            "test",
                            fio::PERM_READABLE | fio::PERM_WRITABLE,
                            ZxioOpenOptions::default()
                        )
                        .expect_err("open3 passed unexpectedly"),
                    zx::Status::ACCESS_DENIED
                );
            })
            .join()
            .expect("join");
            scope.shutdown();
            scope.wait().await;

            // This ensures fs cannot be captured in the thread.
            std::mem::drop(ns);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn large_directory_listing() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs(&kernel);

            let file = &fs
                .root()
                .open_anonymous(current_task, OpenFlags::RDWR)
                .expect("open_anonymous failed");

            let ns = Namespace::new(fs);
            let mut expected_files = vec![b".".to_vec()];
            // Create 500 files. Through trial and error, this number was found to exceed the sink's
            // buffer capacity.
            for i in 0..500 {
                let name = format!("file_{:03}", i);
                ns.root()
                    .open_create_node(
                        current_task,
                        name.as_str().into(),
                        FileMode::from_bits(0o600) | FileMode::IFREG,
                        DeviceId::NONE,
                        OpenFlags::empty(),
                    )
                    .expect("open_create_node failed");
                expected_files.push(name.into_bytes());
            }

            let (root_handle, scope) =
                serve_file(current_task, file, Credentials::root()).expect("serve_file failed");

            std::thread::spawn(move || {
                let root_zxio =
                    Zxio::create(root_handle.into_channel().into()).expect("zxio create failed");

                let expected = expected_files
                    .iter()
                    .map(|x| FsString::from(x.clone()))
                    .collect::<HashSet<_>>();
                let mut iterator =
                    root_zxio.create_dirent_iterator().expect("create_dirent_iterator failed");
                iterator.rewind().expect("rewind failed");

                let mut found = HashSet::new();
                for res in iterator {
                    match res {
                        Ok(dirent) => {
                            found.insert(dirent.name.clone());
                        }
                        Err(status) => {
                            panic!("Iterator returned error: {:?}", status);
                        }
                    }
                }
                assert_eq!(found, expected);
            })
            .join()
            .expect("thread join failed");
            scope.shutdown();
            scope.wait().await;

            // This ensures fs cannot be captured in the thread.
            std::mem::drop(ns);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn readdir_propagates_genuine_nospc_error() {
        spawn_kernel_and_run(async |current_task| {
            use crate::vfs::{
                DirEntry, FileOps, FsNode, FsNodeHandle, FsNodeInfo, FsNodeOps,
                fs_node_impl_dir_readonly,
            };
            use crate::{
                fileops_impl_directory, fileops_impl_noop_sync, fileops_impl_unbounded_seek,
            };

            struct FaultyDirectory;
            impl FileOps for FaultyDirectory {
                fileops_impl_directory!();
                fileops_impl_noop_sync!();
                fileops_impl_unbounded_seek!();

                fn readdir(
                    &self,
                    _file: &FileObject,
                    _current_task: &CurrentTask,
                    _sink: &mut dyn DirentSink,
                ) -> Result<(), Errno> {
                    error!(ENOSPC)
                }
            }

            struct FaultyDirectoryNode;
            impl FsNodeOps for FaultyDirectoryNode {
                fs_node_impl_dir_readonly!();

                fn create_file_ops(
                    &self,
                    _node: &FsNode,
                    _current_task: &CurrentTask,
                    _flags: OpenFlags,
                ) -> Result<Box<dyn FileOps>, Errno> {
                    Ok(Box::new(FaultyDirectory))
                }

                fn lookup(
                    &self,
                    _node: &FsNode,
                    _current_task: &CurrentTask,
                    _name: &FsStr,
                ) -> Result<FsNodeHandle, Errno> {
                    error!(ENOENT)
                }
            }

            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs(&kernel);

            let ino = fs.allocate_ino();
            let info = FsNodeInfo::new(
                FileMode::from_bits(0o777) | FileMode::IFDIR,
                current_task.current_fscred(),
            );
            let node = fs.create_node(ino, FaultyDirectoryNode, info);
            let dir_entry = DirEntry::new(node, None, "faulty_dir".into());
            let name = NamespaceNode::new_anonymous(dir_entry);
            let file = FileObject::new(
                current_task,
                Box::new(FaultyDirectory),
                name,
                OpenFlags::DIRECTORY | OpenFlags::RDONLY,
            )
            .expect("FileObject::new failed");

            let (root_handle, scope) =
                serve_file(current_task, &file, Credentials::root()).expect("serve_file failed");

            std::thread::spawn(move || {
                let root_zxio =
                    Zxio::create(root_handle.into_channel().into()).expect("zxio create failed");

                let mut iterator =
                    root_zxio.create_dirent_iterator().expect("create_dirent_iterator failed");
                iterator.rewind().expect("rewind failed");

                let mut got_error = false;
                for res in iterator {
                    if let Err(status) = res {
                        assert_eq!(status, zx::Status::NO_SPACE);
                        got_error = true;
                        break;
                    }
                }
                assert!(got_error, "Expected iterator to fail with NO_SPACE");
            })
            .join()
            .expect("thread join failed");
            scope.shutdown();
            scope.wait().await;
        })
        .await;
    }
}
