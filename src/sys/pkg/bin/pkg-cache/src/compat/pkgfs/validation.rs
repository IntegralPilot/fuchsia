// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::BaseIndex;
use fidl_fuchsia_io as fio;
use log::{error, info};
use std::sync::Arc;
use vfs::directory::entry::{EntryInfo, OpenRequest};
use vfs::directory::immutable::connection::ImmutableConnection;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::execution_scope::ExecutionScope;
use vfs::path::Path as VfsPath;
use vfs::{ObjectRequestRef, ProtocolsExt as _, immutable_attributes};

/// The pkgfs /ctl/validation directory, except it contains only the "missing" file (e.g. does not
/// have the "present" file).
pub(crate) struct Validation {
    blobfs: blobfs::Client,
    base_packages: Arc<BaseIndex>,
}

impl Validation {
    pub(crate) fn new(blobfs: blobfs::Client, base_packages: Arc<BaseIndex>) -> Arc<Self> {
        Arc::new(Self { blobfs, base_packages })
    }

    // The contents of the "missing" file. The hex-encoded hashes of all the base blobs missing
    // from blobfs, separated and terminated by the newline character, '\n'.
    async fn make_missing_contents(&self) -> Vec<u8> {
        let base_blobs = self.base_packages.list_blobs();

        info!("checking if any of the {} base package blobs are missing", base_blobs.len());

        let mut missing = self
            .blobfs
            .filter_to_missing_blobs(base_blobs.iter().copied())
            .await
            .into_iter()
            .collect::<Vec<_>>();
        missing.sort();

        if missing.is_empty() {
            info!(total = base_blobs.len(); "all base package blobs were found");
        } else {
            error!(total = missing.len(); "base package blobs are missing");
        }

        #[allow(clippy::format_collect)]
        missing.into_iter().map(|hash| format!("{hash}\n")).collect::<String>().into_bytes()
    }
}

impl vfs::directory::entry::DirectoryEntry for Validation {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.open_dir(self)
    }
}

impl vfs::directory::entry::GetEntryInfo for Validation {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl vfs::node::Node for Validation {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::DIRECTORY,
                abilities: fio::Operations::GET_ATTRIBUTES
                    | fio::Operations::ENUMERATE
                    | fio::Operations::TRAVERSE,
                content_size: 1,
                storage_size: 1,
                id: 1,
            }
        ))
    }
}

impl vfs::directory::entry_container::Directory for Validation {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: VfsPath,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        if path.is_empty() {
            if flags.creation_mode() != vfs::CreationMode::Never {
                return Err(zx::Status::NOT_SUPPORTED);
            }

            if let Some(rights) = flags.rights()
                && (rights.intersects(fio::Operations::WRITE_BYTES)
                    | rights.intersects(fio::Operations::EXECUTE))
            {
                return Err(zx::Status::NOT_SUPPORTED);
            }

            // `ImmutableConnection` checks that only directory flags are specified.
            object_request
                .take()
                .create_connection_sync::<ImmutableConnection<_>, _>(scope, self, flags);
            return Ok(());
        }

        if path.as_ref() == "missing" {
            let object_request = object_request.take();
            scope.clone().spawn(async move {
                let missing_contents = self.make_missing_contents().await;
                object_request.handle(|object_request| {
                    vfs::file::serve(
                        vfs::file::vmo::read_only(missing_contents),
                        scope,
                        &flags,
                        object_request,
                    )
                });
            });
            return Ok(());
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
            &[(EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File), "missing".into())],
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
    use blobfs_ramdisk::BlobfsRamdisk;
    use futures::prelude::*;
    use std::collections::HashSet;
    use vfs::directory::entry::GetEntryInfo;
    use vfs::directory::entry_container::Directory;
    use vfs::node::Node;

    struct TestEnv {
        _blobfs: BlobfsRamdisk,
    }

    impl TestEnv {
        async fn new() -> (Self, Arc<Validation>) {
            Self::with_base_packages_and_blobfs_contents(
                Arc::new(BaseIndex::new_test_only(HashSet::new(), std::iter::empty())),
                std::iter::empty(),
            )
            .await
        }

        async fn with_base_packages_and_blobfs_contents(
            base_packages: Arc<BaseIndex>,
            blobfs_contents: impl IntoIterator<Item = (fuchsia_hash::Hash, Vec<u8>)>,
        ) -> (Self, Arc<Validation>) {
            let blobfs = BlobfsRamdisk::start().await.unwrap();
            for (hash, contents) in blobfs_contents.into_iter() {
                blobfs.add_blob_from(hash, contents.as_slice()).await.unwrap()
            }
            let validation = Validation::new(blobfs.client(), base_packages);
            (Self { _blobfs: blobfs }, validation)
        }
    }

    #[fuchsia::test]
    async fn directory_entry_open_self() {
        let (_env, validation) = TestEnv::new().await;
        let proxy = vfs::directory::serve(validation, ExecutionScope::new(), fio::PERM_READABLE);
        assert_eq!(
            fuchsia_fs::directory::readdir(&proxy).await.unwrap(),
            vec![fuchsia_fs::directory::DirEntry {
                name: "missing".to_string(),
                kind: fuchsia_fs::directory::DirentKind::File
            }]
        );
    }

    #[fuchsia::test]
    async fn directory_entry_open_rejects_invalid_flags() {
        let (_env, validation) = TestEnv::new().await;

        for invalid_flags in [
            fio::Flags::FLAG_MUST_CREATE,
            fio::Flags::FLAG_MAYBE_CREATE,
            fio::PERM_WRITABLE,
            fio::PERM_EXECUTABLE,
        ] {
            let proxy = vfs::directory::serve(
                validation.clone(),
                ExecutionScope::new(),
                fio::PERM_READABLE | invalid_flags,
            );
            assert_matches!(
                proxy.take_event_stream().try_next().await,
                Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                    if epitaph == zx::Status::NOT_SUPPORTED
            );
        }
    }

    #[fuchsia::test]
    async fn directory_entry_open_rejects_file_flags() {
        let (_env, validation) = TestEnv::new().await;

        // Requesting to open with `PROTOCOL_FILE` should return a `NOT_FILE` error.
        {
            let proxy = vfs::directory::serve(
                validation.clone(),
                ExecutionScope::new(),
                fio::Flags::PROTOCOL_FILE,
            );
            assert_matches!(
                proxy.take_event_stream().try_next().await,
                Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                    if epitaph == zx::Status::NOT_FILE
            );
        }

        // Opening with file flags is also invalid.
        for file_flags in [fio::Flags::FILE_APPEND, fio::Flags::FILE_TRUNCATE] {
            let proxy = vfs::directory::serve(
                validation.clone(),
                ExecutionScope::new(),
                fio::PERM_READABLE | file_flags,
            );
            assert_matches!(
                proxy.take_event_stream().try_next().await,
                Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                    if epitaph == zx::Status::INVALID_ARGS
            );
        }
    }

    #[fuchsia::test]
    async fn directory_entry_open_missing() {
        let (_env, validation) = TestEnv::with_base_packages_and_blobfs_contents(
            Arc::new(BaseIndex::new_test_only(HashSet::from([[0; 32].into()]), std::iter::empty())),
            std::iter::empty(),
        )
        .await;

        let proxy = vfs::serve_file(
            validation.clone(),
            VfsPath::validate_and_split("missing").unwrap(),
            ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        assert_eq!(
            fuchsia_fs::file::read(&proxy).await.unwrap(),
            b"0000000000000000000000000000000000000000000000000000000000000000\n".to_vec()
        );
    }

    #[fuchsia::test]
    async fn directory_entry_entry_info() {
        let (_env, validation) = TestEnv::new().await;

        assert_eq!(
            GetEntryInfo::entry_info(validation.as_ref()),
            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
        );
    }

    /// Implementation of vfs::directory::dirents_sink::Sink.
    /// Sink::append begins to fail (returns Sealed) after `max_entries` entries have been appended.
    #[derive(Clone)]
    struct FakeSink {
        max_entries: usize,
        entries: Vec<(String, EntryInfo)>,
        sealed: bool,
    }

    impl FakeSink {
        fn new(max_entries: usize) -> Self {
            FakeSink { max_entries, entries: Vec::with_capacity(max_entries), sealed: false }
        }

        fn from_sealed(sealed: Box<dyn vfs::directory::dirents_sink::Sealed>) -> Box<FakeSink> {
            sealed.into()
        }
    }

    impl From<Box<dyn vfs::directory::dirents_sink::Sealed>> for Box<FakeSink> {
        fn from(sealed: Box<dyn vfs::directory::dirents_sink::Sealed>) -> Self {
            sealed.open().downcast::<FakeSink>().unwrap()
        }
    }

    impl vfs::directory::dirents_sink::Sink for FakeSink {
        fn append(
            mut self: Box<Self>,
            entry: &EntryInfo,
            name: &str,
        ) -> vfs::directory::dirents_sink::AppendResult {
            assert!(!self.sealed);
            if self.entries.len() == self.max_entries {
                vfs::directory::dirents_sink::AppendResult::Sealed(self.seal())
            } else {
                self.entries.push((name.to_owned(), entry.clone()));
                vfs::directory::dirents_sink::AppendResult::Ok(self)
            }
        }

        fn seal(mut self: Box<Self>) -> Box<dyn vfs::directory::dirents_sink::Sealed> {
            self.sealed = true;
            self
        }
    }

    impl vfs::directory::dirents_sink::Sealed for FakeSink {
        fn open(self: Box<Self>) -> Box<dyn std::any::Any> {
            self
        }
    }

    #[fuchsia::test]
    async fn directory_read_dirents() {
        let (_env, validation) = TestEnv::new().await;

        let (pos, sealed) = validation
            .read_dirents(&TraversalPosition::Start, Box::new(FakeSink::new(3)))
            .await
            .expect("read_dirents failed");
        assert_eq!(
            FakeSink::from_sealed(sealed).entries,
            vec![
                (".".to_string(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)),
                ("missing".to_string(), EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)),
            ]
        );
        assert_eq!(pos, TraversalPosition::End);
    }

    #[fuchsia::test]
    async fn directory_register_watcher_not_supported() {
        let (_env, validation) = TestEnv::new().await;

        let (_client, server) = fidl::endpoints::create_endpoints();

        assert_eq!(
            Directory::register_watcher(
                validation,
                ExecutionScope::new(),
                fio::WatchMask::empty(),
                server.into(),
            ),
            Err(zx::Status::NOT_SUPPORTED)
        );
    }

    #[fuchsia::test]
    async fn directory_get_attributes() {
        let (_env, validation) = TestEnv::new().await;

        assert_eq!(
            validation.get_attributes(fio::NodeAttributesQuery::all()).await.unwrap(),
            immutable_attributes!(
                fio::NodeAttributesQuery::all(),
                Immutable {
                    protocols: fio::NodeProtocolKinds::DIRECTORY,
                    abilities: fio::Operations::GET_ATTRIBUTES
                        | fio::Operations::ENUMERATE
                        | fio::Operations::TRAVERSE,
                    content_size: 1,
                    storage_size: 1,
                    id: 1,
                }
            )
        );
    }

    #[fuchsia::test]
    async fn make_missing_contents_empty() {
        let (_env, validation) = TestEnv::new().await;

        assert_eq!(validation.make_missing_contents().await, Vec::<u8>::new());
    }

    #[fuchsia::test]
    async fn make_missing_contents_missing_blob() {
        let (_env, validation) = TestEnv::with_base_packages_and_blobfs_contents(
            Arc::new(BaseIndex::new_test_only(HashSet::from([[0; 32].into()]), std::iter::empty())),
            std::iter::empty(),
        )
        .await;

        assert_eq!(
            validation.make_missing_contents().await,
            b"0000000000000000000000000000000000000000000000000000000000000000\n".to_vec()
        );
    }

    #[fuchsia::test]
    async fn make_missing_contents_two_missing_blob() {
        let (_env, validation) = TestEnv::with_base_packages_and_blobfs_contents(
            Arc::new(BaseIndex::new_test_only(
                HashSet::from([[0; 32].into(), [1; 32].into()]),
                std::iter::empty(),
            )),
            std::iter::empty(),
        )
        .await;

        assert_eq!(
            validation.make_missing_contents().await,
            b"0000000000000000000000000000000000000000000000000000000000000000\n\
              0101010101010101010101010101010101010101010101010101010101010101\n"
                .to_vec(),
        );
    }

    #[fuchsia::test]
    async fn make_missing_contents_irrelevant_blobfs_blob() {
        let blob = vec![0u8, 1u8];
        let hash = fuchsia_merkle::root_from_slice(&blob);
        let (_env, validation) = TestEnv::with_base_packages_and_blobfs_contents(
            Arc::new(BaseIndex::new_test_only(HashSet::new(), std::iter::empty())),
            [(hash, blob)],
        )
        .await;

        assert_eq!(validation.make_missing_contents().await, Vec::<u8>::new());
    }

    #[fuchsia::test]
    async fn make_missing_contents_present_blob() {
        let blob = vec![0u8, 1u8];
        let hash = fuchsia_merkle::root_from_slice(&blob);
        let (_env, validation) = TestEnv::with_base_packages_and_blobfs_contents(
            Arc::new(BaseIndex::new_test_only(HashSet::from([hash]), std::iter::empty())),
            [(hash, blob)],
        )
        .await;

        assert_eq!(validation.make_missing_contents().await, Vec::<u8>::new());
    }

    #[fuchsia::test]
    async fn make_missing_contents_present_blob_missing_blob() {
        let blob = vec![0u8, 1u8];
        let hash = fuchsia_merkle::root_from_slice(&blob);
        let mut missing_hash = <[u8; 32]>::from(hash);
        missing_hash[0] = !missing_hash[0];
        let missing_hash = missing_hash.into();

        let (_env, validation) = TestEnv::with_base_packages_and_blobfs_contents(
            Arc::new(BaseIndex::new_test_only(
                HashSet::from([hash, missing_hash]),
                std::iter::empty(),
            )),
            [(hash, blob)],
        )
        .await;

        assert_eq!(
            validation.make_missing_contents().await,
            format!("{missing_hash}\n").into_bytes()
        );
    }
}
