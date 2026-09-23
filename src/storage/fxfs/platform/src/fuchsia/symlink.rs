// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::directory::FxDirectory;
use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::node::FxNode;
use crate::fuchsia::volume::FxVolume;
use anyhow::Error;
use fidl_fuchsia_io as fio;
use fxfs::errors::FxfsError;
use fxfs::object_handle::{ObjectHandle, ObjectProperties};
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::{
    DirType, HandleOptions, ObjectAttributes, ObjectDescriptor, ObjectKey, ObjectKind, ObjectValue,
    StoreObjectHandle,
};
use fxfs_macros::ToWeakNode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use vfs::attributes;
use vfs::directory::entry::{EntryInfo, GetEntryInfo};
use vfs::directory::entry_container::MutableDirectory;
use vfs::name::Name;
use vfs::node::Node;
use vfs::symlink::Symlink;

// We use the top-bit of `open_count` to indicate that the symlink needs to be purged.
const TO_BE_PURGED: u64 = 1 << 63;

#[derive(ToWeakNode)]
pub struct FxSymlink {
    handle: StoreObjectHandle<FxVolume>,
    open_count: AtomicU64,
}

impl FxSymlink {
    pub fn new(volume: Arc<FxVolume>, object_id: u64) -> Self {
        Self {
            handle: StoreObjectHandle::new(
                volume,
                object_id,
                /* permanent_keys: */ false,
                HandleOptions::default(),
                /* trace: */ false,
            ),
            open_count: AtomicU64::new(0),
        }
    }

    async fn get_properties(&self) -> Result<ObjectProperties, Error> {
        let store = self.handle.store();
        // We don't take a transaction lock here because the lifetime of the symlink object
        // (and its records in the LSM tree) is guaranteed for as long as this FxSymlink
        // handle is open. Even if the symlink is unlinked, it is kept in the graveyard
        // until the handle is closed, meaning its object ID and records are stable.
        let item = store
            .tree()
            .find(&ObjectKey::object(self.object_id()))
            .await?
            .ok_or(FxfsError::NotFound)?;
        match item.value {
            ObjectValue::Object {
                kind: ObjectKind::Symlink { refs, link },
                attributes:
                    ObjectAttributes {
                        creation_time,
                        modification_time,
                        posix_attributes,
                        access_time,
                        change_time,
                        ..
                    },
            } => Ok(ObjectProperties {
                refs,
                allocated_size: 0,
                // For POSIX compatibility we report the target length as file size.
                data_attribute_size: link.len() as u64,
                creation_time,
                modification_time,
                access_time,
                change_time,
                sub_dirs: 0,
                posix_attributes,
                dir_type: DirType::Normal,
            }),
            ObjectValue::Object {
                kind: ObjectKind::EncryptedSymlink { refs, link },
                attributes:
                    ObjectAttributes {
                        creation_time,
                        modification_time,
                        posix_attributes,
                        access_time,
                        change_time,
                        ..
                    },
            } => {
                let link_len =
                    store.read_encrypted_symlink(self.object_id(), link.into_vec()).await?.len();
                Ok(ObjectProperties {
                    refs,
                    allocated_size: 0,
                    // For POSIX compatibility we report the target length as file size.
                    data_attribute_size: link_len as u64,
                    creation_time,
                    modification_time,
                    access_time,
                    change_time,
                    sub_dirs: 0,
                    posix_attributes,
                    dir_type: DirType::Normal,
                })
            }
            ObjectValue::None => Err(FxfsError::NotFound.into()),
            _ => Err(FxfsError::NotFile.into()),
        }
    }
    pub fn to_be_purged(&self) -> bool {
        (self.open_count.load(Ordering::Relaxed) & TO_BE_PURGED) != 0
    }
}

impl Symlink for FxSymlink {
    async fn read_target(&self) -> Result<Vec<u8>, zx::Status> {
        self.handle.store().read_symlink(self.object_id()).await.map_err(map_to_status)
    }
}

impl GetEntryInfo for FxSymlink {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.object_id(), fio::DirentType::Symlink)
    }
}

impl Node for FxSymlink {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        let mut props = self.get_properties().await.map_err(map_to_status)?;

        if requested_attributes.contains(fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE) {
            self.handle
                .store()
                .update_access_time(self.object_id(), &mut props, || true)
                .await
                .map_err(map_to_status)?;
        }

        Ok(attributes!(
            requested_attributes,
            Mutable {
                creation_time: props.creation_time.as_nanos(),
                modification_time: props.modification_time.as_nanos(),
                access_time: props.access_time.as_nanos(),
                mode: props.posix_attributes.map(|a| a.mode),
                uid: props.posix_attributes.map(|a| a.uid),
                gid: props.posix_attributes.map(|a| a.gid),
                rdev: props.posix_attributes.map(|a| a.rdev),
                selinux_context: self
                    .handle
                    .get_inline_selinux_context()
                    .await
                    .map_err(map_to_status)?,
            },
            Immutable {
                protocols: fio::NodeProtocolKinds::SYMLINK,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::UPDATE_ATTRIBUTES,
                content_size: props.data_attribute_size,
                storage_size: props.allocated_size,
                link_count: props.refs,
                id: self.object_id(),
                verity_enabled: false,
            }
        ))
    }

    async fn link_into(
        self: Arc<Self>,
        destination_dir: Arc<dyn MutableDirectory>,
        name: Name,
    ) -> Result<(), zx::Status> {
        if self.to_be_purged() {
            return Err(zx::Status::NOT_FOUND);
        }
        let dir = destination_dir.into_any().downcast::<FxDirectory>().unwrap();
        let store = self.handle.store();
        let transaction = store
            .new_transaction(
                lock_keys![
                    LockKey::object(store.store_object_id(), self.object_id()),
                    LockKey::object(store.store_object_id(), dir.object_id()),
                ],
                Options::default(),
            )
            .await
            .map_err(map_to_status)?;
        dir.link_object(transaction, &name, self.object_id(), ObjectDescriptor::Symlink).await
    }

    fn query_filesystem(&self) -> Result<fio::FilesystemInfo, zx::Status> {
        Ok(self.handle.owner().filesystem_info_for_volume())
    }

    async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, zx::Status> {
        self.handle.list_extended_attributes().await.map_err(map_to_status)
    }

    async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, zx::Status> {
        self.handle.get_extended_attribute(name).await.map_err(map_to_status)
    }

    async fn set_extended_attribute(
        &self,
        name: Vec<u8>,
        value: Vec<u8>,
        mode: fio::SetExtendedAttributeMode,
    ) -> Result<(), zx::Status> {
        self.handle.set_extended_attribute(name, value, mode.into()).await.map_err(map_to_status)
    }

    async fn remove_extended_attribute(&self, name: Vec<u8>) -> Result<(), zx::Status> {
        self.handle.remove_extended_attribute(name).await.map_err(map_to_status)
    }

    fn will_clone(&self) {
        self.open_count_add_one();
    }

    fn close(self: Arc<Self>) {
        self.open_count_sub_one();
    }
}

impl FxNode for FxSymlink {
    fn object_id(&self) -> u64 {
        self.handle.object_id()
    }

    fn parent(&self) -> Option<Arc<FxDirectory>> {
        None
    }

    fn set_parent(&self, _parent: Arc<FxDirectory>) {}
    fn open_count_add_one(&self) {
        self.open_count.fetch_add(1, Ordering::Relaxed);
    }
    fn open_count_sub_one(self: Arc<Self>) {
        let old = self.open_count.fetch_sub(1, Ordering::Relaxed);
        assert!(old & !TO_BE_PURGED > 0);
        if old == 1 | TO_BE_PURGED {
            self.handle.owner().clone().spawn(async move {
                let store = self.handle.store();
                store
                    .filesystem()
                    .graveyard()
                    .queue_tombstone_object(store.store_object_id(), self.object_id());
            });
        }
    }

    fn object_descriptor(&self) -> ObjectDescriptor {
        ObjectDescriptor::Symlink
    }

    fn mark_to_be_purged(self: Arc<Self>) {
        let old = self.open_count.fetch_or(TO_BE_PURGED, Ordering::Relaxed);
        assert!(old & TO_BE_PURGED == 0);
        if old == 0 {
            let store = self.handle.store();
            store
                .filesystem()
                .graveyard()
                .queue_tombstone_object(store.store_object_id(), self.object_id());
        }
    }
}
