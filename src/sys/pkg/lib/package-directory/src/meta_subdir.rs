// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::root_dir::RootDir;
use fidl_fuchsia_io as fio;
use std::sync::Arc;
use vfs::directory::entry::EntryInfo;
use vfs::directory::immutable::connection::ImmutableConnection;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::execution_scope::ExecutionScope;
use vfs::{ObjectRequestRef, immutable_attributes};

pub(crate) struct MetaSubdir<S: crate::NonMetaStorage> {
    root_dir: Arc<RootDir<S>>,
    // The object relative path expression of the subdir relative to the package root with a
    // trailing slash appended.
    path: String,
}

impl<S: crate::NonMetaStorage> MetaSubdir<S> {
    pub(crate) fn new(root_dir: Arc<RootDir<S>>, path: String) -> Arc<Self> {
        Arc::new(MetaSubdir { root_dir, path })
    }
}

impl<S: crate::NonMetaStorage> vfs::directory::entry::GetEntryInfo for MetaSubdir<S> {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl<S: crate::NonMetaStorage> vfs::node::Node for MetaSubdir<S> {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        let size = crate::usize_to_u64_safe(self.root_dir.meta_files.element_len());
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::DIRECTORY,
                abilities: crate::DIRECTORY_ABILITIES,
                content_size: size,
                storage_size: size,
                id: 1,
            }
        ))
    }
}

impl<S: crate::NonMetaStorage> vfs::directory::entry_container::Directory for MetaSubdir<S> {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: vfs::Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        if !flags.difference(crate::ALLOWED_FLAGS).is_empty() {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        // Disallow creating an executable connection to this node or any children.
        if flags.contains(fio::Flags::PERM_EXECUTE) {
            return Err(zx::Status::NOT_SUPPORTED);
        }

        // Handle case where the request is for this directory itself (e.g. ".").
        if path.is_empty() {
            // `ImmutableConnection` checks that only directory flags are specified.
            object_request
                .take()
                .create_connection_sync::<ImmutableConnection<_>, _>(scope, self, flags);
            return Ok(());
        }

        // `path` is relative, and may include a trailing slash.
        let file_path = format!(
            "{}{}",
            self.path,
            path.as_ref().strip_suffix('/').unwrap_or_else(|| path.as_ref())
        );

        if let Some(file) = self.root_dir.get_meta_file(&file_path)? {
            if path.is_dir() {
                return Err(zx::Status::NOT_DIR);
            }
            return vfs::file::serve(file, scope, &flags, object_request);
        }

        if let Some(subdir) = self.root_dir.get_meta_subdir(file_path + "/") {
            return subdir.open(scope, vfs::Path::dot(), flags, object_request);
        }

        Err(zx::Status::NOT_FOUND)
    }

    async fn read_dirents(
        &self,
        pos: &TraversalPosition,
        sink: Box<dyn vfs::directory::dirents_sink::Sink + 'static>,
    ) -> Result<
        (TraversalPosition, Box<dyn vfs::directory::dirents_sink::Sealed + 'static>),
        zx::Status,
    > {
        vfs::directory::read_dirents::read_dirents(
            &crate::get_dir_children(self.root_dir.meta_files.keys(), &self.path),
            pos,
            sink,
        )
    }

    fn register_watcher(
        self: Arc<Self>,
        _: ExecutionScope,
        _: fio::WatchMask,
        _: vfs::directory::entry_container::DirectoryWatcher,
    ) -> Result<(), zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }

    // `register_watcher` is unsupported so no need to do anything here.
    fn unregister_watcher(self: Arc<Self>, _: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_fs::directory::{DirEntry, DirentKind};
    use fuchsia_pkg_testing::PackageBuilder;
    use fuchsia_pkg_testing::blobfs::Fake as FakeBlobfs;
    use futures::prelude::*;

    struct TestEnv {
        _blobfs_fake: FakeBlobfs,
    }

    impl TestEnv {
        async fn new() -> (Self, fio::DirectoryProxy) {
            let pkg = PackageBuilder::new("pkg")
                .add_resource_at("meta/dir/dir/file", &b"contents"[..])
                .build()
                .await
                .unwrap();
            let (metafar_blob, _) = pkg.contents();
            let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
            blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
            let root_dir = RootDir::new(blobfs_client, metafar_blob.merkle).await.unwrap();
            let sub_dir = MetaSubdir::new(root_dir, "meta/dir/".to_string());
            (
                Self { _blobfs_fake: blobfs_fake },
                vfs::directory::serve_read_only(
                    sub_dir,
                    vfs::execution_scope::ExecutionScope::new(),
                ),
            )
        }
    }

    /// Ensure connections to a [`MetaSubdir`] cannot be created as mutable (i.e. with
    /// [`fio::PERM_WRITABLE`]) or executable ([`fio::PERM_EXECUTABLE`]). This ensures that the VFS
    /// will disallow any attempts to create a new file/directory, modify the attributes of any
    /// nodes, or open any files as writable/executable.
    #[fuchsia::test]
    async fn meta_subdir_cannot_be_served_as_mutable() {
        let pkg = PackageBuilder::new("pkg")
            .add_resource_at("meta/dir/dir/file", &b"contents"[..])
            .build()
            .await
            .unwrap();
        let (metafar_blob, _) = pkg.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
        let root_dir = RootDir::new(blobfs_client, metafar_blob.merkle).await.unwrap();
        let sub_dir = MetaSubdir::new(root_dir, "meta/dir/".to_string());
        for flags in [fio::PERM_WRITABLE, fio::PERM_EXECUTABLE] {
            let proxy = vfs::directory::serve(sub_dir.clone(), ExecutionScope::new(), flags);
            assert_matches!(
                proxy.take_event_stream().try_next().await,
                Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                    if epitaph == zx::Status::NOT_SUPPORTED
            );
        }
    }

    #[fuchsia::test]
    async fn meta_subdir_readdir() {
        let (_env, sub_dir) = TestEnv::new().await;
        assert_eq!(
            fuchsia_fs::directory::readdir_inclusive(&sub_dir).await.unwrap(),
            vec![
                DirEntry { name: ".".to_string(), kind: DirentKind::Directory },
                DirEntry { name: "dir".to_string(), kind: DirentKind::Directory }
            ]
        );
    }

    #[fuchsia::test]
    async fn meta_subdir_get_attributes() {
        let (_env, sub_dir) = TestEnv::new().await;
        let (mutable_attributes, immutable_attributes) =
            sub_dir.get_attributes(fio::NodeAttributesQuery::all()).await.unwrap().unwrap();
        assert_eq!(
            fio::NodeAttributes2 { mutable_attributes, immutable_attributes },
            immutable_attributes!(
                fio::NodeAttributesQuery::all(),
                Immutable {
                    protocols: fio::NodeProtocolKinds::DIRECTORY,
                    abilities: crate::DIRECTORY_ABILITIES,
                    content_size: 4,
                    storage_size: 4,
                    id: 1,
                }
            )
        );
    }

    #[fuchsia::test]
    async fn meta_subdir_watch_not_supported() {
        let (_env, sub_dir) = TestEnv::new().await;
        let (_client, server) = fidl::endpoints::create_endpoints();
        assert_eq!(
            zx::Status::ok(sub_dir.watch(fio::WatchMask::empty(), 0, server).await.unwrap()),
            Err(zx::Status::NOT_SUPPORTED)
        );
    }

    #[fuchsia::test]
    async fn meta_subdir_open_file() {
        let (_env, sub_dir) = TestEnv::new().await;
        let proxy = fuchsia_fs::directory::open_file(&sub_dir, "dir/file", fio::PERM_READABLE)
            .await
            .unwrap();
        assert_eq!(fuchsia_fs::file::read(&proxy).await.unwrap(), b"contents".to_vec());
    }

    #[fuchsia::test]
    async fn meta_subdir_open_directory() {
        let (_env, sub_dir) = TestEnv::new().await;
        for path in ["dir", "dir/"] {
            let proxy = fuchsia_fs::directory::open_directory(&sub_dir, path, fio::PERM_READABLE)
                .await
                .unwrap();
            assert_eq!(
                fuchsia_fs::directory::readdir(&proxy).await.unwrap(),
                vec![fuchsia_fs::directory::DirEntry {
                    name: "file".to_string(),
                    kind: fuchsia_fs::directory::DirentKind::File
                }]
            );
        }
    }
}
