// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::pager::ErofsPacketReceiver;
use crate::volume::ErofsVolume;
use erofs::FileNode;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use std::sync::Arc;
use vfs::ObjectRequestRef;
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::execution_scope::ExecutionScope;
use vfs::file::connection::GetVmo;
use vfs::file::{File, FileLike, FileOptions, SyncMode};
use vfs::node::Node;

/// An implementation of an EROFS file backed by a Zircon Pager VMO.
pub struct ErofsFile {
    volume: Arc<ErofsVolume>,
    node: FileNode,
    vmo: zx::Vmo,
    registration: fasync::ReceiverRegistration<ErofsPacketReceiver>,
}

impl ErofsFile {
    /// Creates a new pager-backed `ErofsFile` using `Arc::new_cyclic` to establish a weak
    /// self-reference within the registered `ErofsPacketReceiver`. This permits the file to be
    /// kept alive dynamically when being used by clients, and cleaned up when no longer in use.
    pub fn new(volume: Arc<ErofsVolume>, node: FileNode) -> Result<Arc<Self>, zx::Status> {
        let file = Arc::new_cyclic(|weak| {
            let (vmo, registration) = volume
                .pager()
                .create_vmo(weak.clone(), node.size())
                .expect("Failed to create pager VMO");
            Self { volume, node, vmo, registration }
        });
        Ok(file)
    }

    pub fn fs(&self) -> &erofs::ErofsFilesystem {
        self.volume.fs()
    }

    pub fn node(&self) -> &FileNode {
        &self.node
    }

    pub fn vmo(&self) -> &zx::Vmo {
        &self.vmo
    }

    pub(crate) fn register_zero_children_wait(&self) -> Result<(), zx::Status> {
        self.vmo.wait_async(
            fasync::EHandle::local().port(),
            self.registration.key(),
            zx::Signals::VMO_ZERO_CHILDREN,
            zx::WaitAsyncOpts::empty(),
        )
    }

    /// Instructs the pager to watch for the `VMO_ZERO_CHILDREN` signal.
    ///
    /// If the VMO is currently held weakly by the packet receiver, this method upgrades it to a
    /// `Strong` reference to prevent the file from being deallocated while clients have active
    /// mappings, and registers the signal wait on the VMO. Returns `Ok(true)` if a transition to
    /// `Strong` occurred.
    pub fn watch_for_zero_children(&self) -> Result<bool, zx::Status> {
        let mut file_holder = self.registration.receiver().file.lock().unwrap();
        match &*file_holder {
            crate::pager::FileHolder::Weak(weak) => {
                let strong = weak.upgrade().ok_or(zx::Status::BAD_STATE)?;

                // Start watching for VMO_ZERO_CHILDREN
                self.register_zero_children_wait()?;

                *file_holder = crate::pager::FileHolder::Strong(strong);
                Ok(true)
            }
            crate::pager::FileHolder::Strong(_) => Ok(false),
        }
    }
}

impl DirectoryEntry for ErofsFile {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.open_file(self)
    }
}

impl GetEntryInfo for ErofsFile {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.node.nid(), fio::DirentType::File)
    }
}

impl Node for ErofsFile {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        let mtime = self.node.mtime_ns();
        let content_size = self.node.size();
        let storage_size = self.node.storage_size(self.volume.fs().block_size());
        let selinux_context = self
            .volume
            .fs()
            .get_xattr(&self.node, fio::SELINUX_CONTEXT_NAME.as_bytes())
            .ok()
            .flatten()
            .map(|val| {
                if val.len() <= fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize {
                    fio::SelinuxContext::Data(val)
                } else {
                    fio::SelinuxContext::UseExtendedAttributes(fio::EmptyStruct {})
                }
            });
        Ok(vfs::attributes!(
            requested_attributes,
            Mutable {
                mode: self.node.mode() as u32,
                uid: self.node.uid(),
                gid: self.node.gid(),
                creation_time: mtime,
                modification_time: mtime,
                access_time: mtime,
                selinux_context: selinux_context,
            },
            Immutable {
                protocols: fio::NodeProtocolKinds::FILE,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::READ_BYTES,
                content_size: content_size,
                storage_size: storage_size,
                id: self.node.nid(),
                link_count: self.node.link_count() as u64,
                change_time: mtime,
            }
        ))
    }

    async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, zx::Status> {
        self.volume.fs().list_xattrs(&self.node).map_err(|e| e.to_status())
    }

    async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, zx::Status> {
        self.volume
            .fs()
            .get_xattr(&self.node, &name)
            .map_err(|e| e.to_status())?
            .ok_or(zx::Status::NOT_FOUND)
    }
}

impl GetVmo for ErofsFile {
    const PAGER_ON_FIDL_EXECUTOR: bool = true;

    fn get_vmo(&self) -> &zx::Vmo {
        &self.vmo
    }
}

impl File for ErofsFile {
    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn executable(&self) -> bool {
        false
    }

    async fn open_file(&self, _options: &FileOptions) -> Result<(), zx::Status> {
        Ok(())
    }

    async fn truncate(&self, _length: u64) -> Result<(), zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }

    async fn get_size(&self) -> Result<u64, zx::Status> {
        Ok(self.node.size())
    }

    async fn update_attributes(
        &self,
        _attributes: fio::MutableNodeAttributes,
    ) -> Result<(), zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }

    async fn get_backing_memory(&self, flags: fio::VmoFlags) -> Result<zx::Vmo, zx::Status> {
        let mut vmo_rights = vmo_flags_to_rights(flags)
            | zx::Rights::BASIC
            | zx::Rights::MAP
            | zx::Rights::GET_PROPERTY;

        let child_vmo = if flags.contains(fio::VmoFlags::PRIVATE_CLONE) {
            vmo_rights |= zx::Rights::SET_PROPERTY;
            let mut child_options = zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE;
            if flags.contains(fio::VmoFlags::WRITE) {
                child_options |= zx::VmoChildOptions::RESIZABLE;
                vmo_rights |= zx::Rights::RESIZE;
            }
            self.vmo.create_child(child_options, 0, self.node.size())?
        } else {
            self.vmo.create_child(zx::VmoChildOptions::REFERENCE, 0, 0)?
        };

        let child_vmo = child_vmo.replace_handle(vmo_rights)?;

        let _ = self.watch_for_zero_children()?;

        Ok(child_vmo)
    }

    async fn sync(&self, _mode: SyncMode) -> Result<(), zx::Status> {
        Ok(())
    }
}

impl FileLike for ErofsFile {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        options: FileOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        let request = object_request.take();
        let scope_clone = scope.clone();
        scope.spawn(request.handle_async(async move |object_request_ref| {
            vfs::file::StreamIoConnection::create(scope_clone, self, options, object_request_ref)
                .await
        }));
        Ok(())
    }
}

/// Maps VMO flags to their respective rights.
fn vmo_flags_to_rights(vmo_flags: fio::VmoFlags) -> zx::Rights {
    let mut rights = zx::Rights::NONE;
    if vmo_flags.contains(fio::VmoFlags::READ) {
        rights |= zx::Rights::READ;
    }
    if vmo_flags.contains(fio::VmoFlags::WRITE) {
        rights |= zx::Rights::WRITE;
    }
    if vmo_flags.contains(fio::VmoFlags::EXECUTE) {
        rights |= zx::Rights::EXECUTE;
    }
    rights
}
