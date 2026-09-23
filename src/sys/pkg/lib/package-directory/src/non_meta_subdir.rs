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

pub(crate) struct NonMetaSubdir<S: crate::NonMetaStorage> {
    root_dir: Arc<RootDir<S>>,
    // The object relative path expression of the subdir relative to the package root with a
    // trailing slash appended.
    path: String,
}

impl<S: crate::NonMetaStorage> NonMetaSubdir<S> {
    pub(crate) fn new(root_dir: Arc<RootDir<S>>, path: String) -> Arc<Self> {
        Arc::new(NonMetaSubdir { root_dir, path })
    }
}

impl<S: crate::NonMetaStorage> vfs::directory::entry::GetEntryInfo for NonMetaSubdir<S> {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl<S: crate::NonMetaStorage> vfs::node::Node for NonMetaSubdir<S> {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::DIRECTORY,
                abilities: crate::DIRECTORY_ABILITIES,
                id: 1,
            }
        ))
    }
}

impl<S: crate::NonMetaStorage> vfs::directory::entry_container::Directory for NonMetaSubdir<S> {
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

        if let Some(blob) = self.root_dir.non_meta_files.get(file_path.as_str()) {
            if path.is_dir() {
                return Err(zx::Status::NOT_DIR);
            }
            return self.root_dir.non_meta_storage.open(blob, flags, scope, object_request);
        }

        if let Some(subdir) = self.root_dir.get_non_meta_subdir(file_path + "/") {
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
            &crate::get_dir_children(self.root_dir.non_meta_files.keys(), &self.path),
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
                .add_resource_at("dir0/dir1/file", "bloblob".as_bytes())
                .build()
                .await
                .unwrap();
            let (metafar_blob, content_blobs) = pkg.contents();
            let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
            blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
            for (hash, bytes) in content_blobs {
                blobfs_fake.add_blob(hash, bytes);
            }
            let root_dir = RootDir::new(blobfs_client, metafar_blob.merkle).await.unwrap();
            let sub_dir = NonMetaSubdir::new(root_dir, "dir0/".to_string());
            (
                Self { _blobfs_fake: blobfs_fake },
                vfs::directory::serve_read_only(
                    sub_dir,
                    vfs::execution_scope::ExecutionScope::new(),
                ),
            )
        }
    }

    /// Ensure connections to a [`NonMetaSubdir`] cannot be created as mutable (i.e. with
    /// [`fio::PERM_WRITABLE`]) This ensures that the VFS will disallow any attempts to create a new
    /// file/directory, modify the attributes of any nodes, or open any files as writable.
    #[fuchsia::test]
    async fn non_meta_subdir_cannot_be_served_as_mutable() {
        let pkg = PackageBuilder::new("pkg")
            .add_resource_at("dir0/dir1/file", "bloblob".as_bytes())
            .build()
            .await
            .unwrap();
        let (metafar_blob, content_blobs) = pkg.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
        for (hash, bytes) in content_blobs {
            blobfs_fake.add_blob(hash, bytes);
        }
        let root_dir = RootDir::new(blobfs_client, metafar_blob.merkle).await.unwrap();
        let sub_dir = NonMetaSubdir::new(root_dir, "dir0/".to_string());
        let proxy = vfs::directory::serve(sub_dir, ExecutionScope::new(), fio::PERM_WRITABLE);
        assert_matches!(
            proxy.take_event_stream().try_next().await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx::Status::NOT_SUPPORTED
        );
    }

    #[fuchsia::test]
    async fn non_meta_subdir_readdir() {
        let (_env, sub_dir) = TestEnv::new().await;
        assert_eq!(
            fuchsia_fs::directory::readdir_inclusive(&sub_dir).await.unwrap(),
            vec![
                DirEntry { name: ".".to_string(), kind: DirentKind::Directory },
                DirEntry { name: "dir1".to_string(), kind: DirentKind::Directory }
            ]
        );
    }

    #[fuchsia::test]
    async fn non_meta_subdir_get_attributes() {
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
                    id: 1,
                }
            )
        );
    }

    #[fuchsia::test]
    async fn non_meta_subdir_watch_not_supported() {
        let (_env, sub_dir) = TestEnv::new().await;
        let (_client, server) = fidl::endpoints::create_endpoints();
        assert_eq!(
            zx::Status::ok(sub_dir.watch(fio::WatchMask::empty(), 0, server).await.unwrap()),
            Err(zx::Status::NOT_SUPPORTED)
        );
    }

    #[fuchsia::test]
    async fn non_meta_subdir_open_directory() {
        let (_env, sub_dir) = TestEnv::new().await;
        for path in ["dir1", "dir1/"] {
            let proxy = fuchsia_fs::directory::open_directory(&sub_dir, path, fio::PERM_READABLE)
                .await
                .unwrap();
            assert_eq!(
                fuchsia_fs::directory::readdir(&proxy).await.unwrap(),
                vec![DirEntry { name: "file".to_string(), kind: DirentKind::File }]
            );
        }
    }

    #[fuchsia::test]
    async fn non_meta_subdir_open_file() {
        let (_env, sub_dir) = TestEnv::new().await;
        let proxy = fuchsia_fs::directory::open_file(&sub_dir, "dir1/file", fio::PERM_READABLE)
            .await
            .unwrap();
        assert_eq!(fuchsia_fs::file::read(&proxy).await.unwrap(), b"bloblob".to_vec())
    }
}
