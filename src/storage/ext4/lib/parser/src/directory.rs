// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ext4_lib::parser::XattrMap;
use fidl_fuchsia_io as fio;
use fuchsia_sync::Mutex;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::future::ready;
use std::iter;
use std::sync::Arc;
use vfs::directory::dirents_sink;
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::directory::entry_container::{Directory, DirectoryWatcher};
use vfs::directory::immutable::connection::ImmutableConnection;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::directory::watchers::Watchers;
use vfs::directory::watchers::event_producers::{SingleNameEventProducer, StaticVecEventProducer};
use vfs::execution_scope::ExecutionScope;
use vfs::name::Name;
use vfs::node::Node;
use vfs::path::Path;
use vfs::{CreationMode, ObjectRequestRef, ProtocolsExt, immutable_attributes};
use zx::Status;

use crate::node::ExtNode;
use crate::types::ExtAttributes;

/// An ext4 filesystem directory node.
#[derive(Debug)]
pub struct ExtDirectory {
    inode: u64,
    xattrs: XattrMap,
    attributes: ExtAttributes,
    data: Mutex<ExtDirectoryData>,
}

struct ExtDirectoryData {
    children: BTreeMap<Name, ExtNode>,
    watchers: Watchers,
}

impl std::fmt::Debug for ExtDirectoryData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtDirectoryData").field("children", &self.children).finish()
    }
}

impl ExtDirectory {
    /// Creates a new [`ExtDirectory`] with the given `inode` and `attributes`.
    pub fn new(inode: u64, attributes: ExtAttributes, xattrs: XattrMap) -> Arc<Self> {
        Arc::new(Self {
            inode,
            xattrs,
            attributes,
            data: Mutex::new(ExtDirectoryData {
                children: BTreeMap::new(),
                watchers: Watchers::new(),
            }),
        })
    }

    /// Inserts a child identified by `name`.
    pub fn insert_child(
        &self,
        name: impl Into<String>,
        child: impl Into<ExtNode>,
    ) -> Result<ExtNode, Status> {
        let name = Name::try_from(name.into())?;
        let mut data = self.data.lock();
        match data.children.entry(name) {
            Entry::Vacant(slot) => {
                let name = slot.key().clone();
                let child = slot.insert(child.into()).clone();
                data.watchers.send_event(&mut SingleNameEventProducer::added(&name));
                Ok(child)
            }
            Entry::Occupied(_) => Err(Status::ALREADY_EXISTS),
        }
    }

    fn lookup_child(
        self: &Arc<Self>,
        mut path: Path,
        flags: fio::Flags,
    ) -> Result<ExtNode, Status> {
        let mut current_entry = ExtNode::Dir(self.clone());

        while !path.is_empty() {
            let child_flags =
                if path.is_single_component() { flags } else { fio::Flags::PROTOCOL_DIRECTORY };

            match current_entry {
                ExtNode::Dir(dir) => {
                    let name = Name::try_from(path.next().unwrap().to_string())?;
                    current_entry = dir.open_child(&name, child_flags)?;
                }
                ExtNode::File(_) | ExtNode::Symlink(_) => {
                    return Err(Status::NOT_DIR);
                }
            }
        }

        Ok(current_entry)
    }

    fn open_child(self: &Arc<Self>, name: &Name, flags: fio::Flags) -> Result<ExtNode, Status> {
        if flags.create_unnamed_temporary_in_directory_path() {
            return Err(Status::NOT_SUPPORTED);
        }

        let data = self.data.lock();

        if let Some(child) = data.children.get(name) {
            if flags.creation_mode() == CreationMode::Always {
                return Err(Status::ALREADY_EXISTS);
            }
            return Ok(child.clone());
        }

        // This filesystem is immutable. If the child cannot be found, do not attempt to create it,
        // even if requested via flags.
        Err(Status::NOT_FOUND)
    }
}

impl GetEntryInfo for ExtDirectory {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.inode, fio::DirentType::Directory)
    }
}

impl DirectoryEntry for ExtDirectory {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_dir(self)
    }
}

impl Node for ExtDirectory {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        Ok(self.attributes.overlay_node_attributes(
            requested_attributes,
            immutable_attributes!(
                requested_attributes,
                Immutable {
                    protocols: fio::NodeProtocolKinds::DIRECTORY,
                    abilities: fio::Operations::GET_ATTRIBUTES
                        | fio::Operations::ENUMERATE
                        | fio::Operations::TRAVERSE,
                    id: self.inode,
                }
            ),
        ))
    }

    fn list_extended_attributes(
        &self,
    ) -> impl Future<Output = Result<Vec<Vec<u8>>, Status>> + Send {
        ready(Ok(self.xattrs.keys().map(Clone::clone).collect()))
    }

    fn get_extended_attribute(
        &self,
        name: Vec<u8>,
    ) -> impl Future<Output = Result<Vec<u8>, Status>> + Send {
        ready(self.xattrs.get(&name).map(Clone::clone).ok_or(Status::NOT_FOUND))
    }
}

impl Directory for ExtDirectory {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        match self.lookup_child(path, flags)? {
            ExtNode::Dir(dir) => {
                object_request
                    .take()
                    .create_connection_sync::<ImmutableConnection<_>, _>(scope, dir, flags);
                Ok(())
            }
            ExtNode::File(file) => {
                file.open_entry(OpenRequest::new(scope, flags, Path::dot(), object_request))
            }
            ExtNode::Symlink(symlink) => {
                symlink.open_entry(OpenRequest::new(scope, flags, Path::dot(), object_request))
            }
        }
    }

    async fn read_dirents(
        &self,
        pos: &TraversalPosition,
        sink: Box<dyn dirents_sink::Sink>,
    ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), Status> {
        use dirents_sink::AppendResult;

        let data = self.data.lock();
        let (mut sink, entries_iter) = match pos {
            TraversalPosition::Start => {
                match sink.append(&EntryInfo::new(self.inode, fio::DirentType::Directory), ".") {
                    AppendResult::Ok(sink) => (sink, data.children.range::<Name, _>(..)),
                    AppendResult::Sealed(sealed) => {
                        let new_pos = match data.children.keys().next() {
                            None => TraversalPosition::End,
                            Some(first_name) => TraversalPosition::Name(first_name.clone().into()),
                        };
                        return Ok((new_pos, sealed));
                    }
                }
            }

            TraversalPosition::Name(next_name) => {
                // The only way to get a `TraversalPosition::Name` is if we returned it in the
                // `AppendResult::Sealed` code path above. Therefore, the conversion from
                // `next_name` to `Name` will never fail in practice.
                let next: Name = next_name.to_owned().try_into().unwrap();
                (sink, data.children.range::<Name, _>(next..))
            }

            TraversalPosition::End => return Ok((TraversalPosition::End, sink.seal())),

            _ => unreachable!(),
        };

        for (name, entry) in entries_iter {
            match sink.append(&entry.as_entry().entry_info(), &name) {
                AppendResult::Ok(new_sink) => sink = new_sink,
                AppendResult::Sealed(sealed) => {
                    return Ok((TraversalPosition::Name(name.clone().into()), sealed));
                }
            }
        }

        Ok((TraversalPosition::End, sink.seal()))
    }

    fn register_watcher(
        self: Arc<Self>,
        scope: ExecutionScope,
        mask: fio::WatchMask,
        watcher: DirectoryWatcher,
    ) -> Result<(), Status> {
        let mut data = self.data.lock();

        // Creating the watcher controller mutably borrows `data`. Extract the existing children
        // first, if requested.
        let existing_children = if mask.contains(fio::WatchMask::EXISTING) {
            iter::once(".".to_owned())
                .chain(data.children.keys().map(|x| x.to_owned().into()))
                .collect()
        } else {
            vec![]
        };

        let controller = data.watchers.add(scope, self.clone(), mask, watcher);
        if !existing_children.is_empty() {
            controller.send_event(&mut StaticVecEventProducer::existing(existing_children));
        }
        controller.send_event(&mut SingleNameEventProducer::idle());
        Ok(())
    }

    fn unregister_watcher(self: Arc<Self>, key: usize) {
        let mut data = self.data.lock();
        data.watchers.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use fidl_fuchsia_io::ExtendedAttributeValue;

    use super::*;

    #[test]
    fn insert_child_success() {
        let dir = ExtDirectory::new(0, ExtAttributes::default(), XattrMap::default());
        dir.insert_child(
            "path_without_separators",
            ExtDirectory::new(1, ExtAttributes::default(), XattrMap::default()),
        )
        .expect("insert_child with valid filename should succeed");
    }

    #[test]
    fn insert_child_error_duplicate() {
        let dir = ExtDirectory::new(0, ExtAttributes::default(), XattrMap::default());
        dir.insert_child("a", ExtDirectory::new(1, ExtAttributes::default(), XattrMap::default()))
            .expect("insert_child with valid filename should succeed");

        let status = dir
            .insert_child("a", ExtDirectory::new(1, ExtAttributes::default(), XattrMap::default()))
            .expect_err("insert_child with duplicate filename should fail");
        assert_eq!(status, Status::ALREADY_EXISTS);
    }

    #[test]
    fn insert_child_error_name_with_path_separator() {
        let dir = ExtDirectory::new(0, ExtAttributes::default(), XattrMap::default());
        let status = dir
            .insert_child(
                "path/with/separators",
                ExtDirectory::new(1, ExtAttributes::default(), XattrMap::default()),
            )
            .expect_err("insert_child with path separator in filename should fail");
        assert_eq!(status, Status::INVALID_ARGS);
    }

    #[test]
    fn insert_child_error_name_too_long() {
        let dir = ExtDirectory::new(0, ExtAttributes::default(), XattrMap::default());
        let status = dir
            .insert_child(
                "a".repeat(1000),
                ExtDirectory::new(1, ExtAttributes::default(), XattrMap::default()),
            )
            .expect_err("insert_child whose name is too long should fail");
        assert_eq!(status, Status::BAD_PATH);
    }

    #[fuchsia::test]
    async fn test_get_dac_attributes() {
        let directory = ExtDirectory::new(
            123,
            ExtAttributes { mode: 0x8124, uid: 456, gid: 789 },
            XattrMap::default(),
        );
        let proxy = vfs::directory::serve_read_only(directory, ExecutionScope::new());

        let attributes_query = fio::NodeAttributesQuery::ID
            | fio::NodeAttributesQuery::MODE
            | fio::NodeAttributesQuery::UID
            | fio::NodeAttributesQuery::GID;
        let (mutable_attributes, immutable_attributes) = proxy
            .get_attributes(attributes_query)
            .await
            .expect("get_attributes FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("get_attributes error");
        assert_eq!(immutable_attributes.id.expect("missing id attribute"), 123);
        assert_eq!(mutable_attributes.mode.expect("missing mode attribute"), 0x8124);
        assert_eq!(mutable_attributes.uid.expect("missing uid attribute"), 456);
        assert_eq!(mutable_attributes.gid.expect("missing gid attribute"), 789);

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");
    }

    #[fuchsia::test]
    async fn test_get_extended_attributes() {
        let xattrs =
            [(b"attr".into(), b"value".into()), (b"attr2".into(), b"value2".into())].into();
        let directory = ExtDirectory::new(123, ExtAttributes::default(), xattrs);
        let proxy = vfs::directory::serve_read_only(directory, ExecutionScope::new());

        let value = proxy
            .get_extended_attribute(b"attr2")
            .await
            .expect("get_extended_attribute FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("get_extended_attribute error");
        assert_eq!(value, ExtendedAttributeValue::Bytes(b"value2".into()));

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");
    }
}
