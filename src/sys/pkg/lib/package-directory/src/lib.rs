// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use std::collections::HashSet;
use std::convert::TryInto as _;
use std::future::Future;
use vfs::common::send_on_open_with_error;
use vfs::directory::entry::EntryInfo;
use vfs::directory::entry_container::Directory;
use vfs::{ObjectRequest, ObjectRequestRef};

mod meta_as_dir;
mod meta_subdir;
mod non_meta_subdir;
mod root_dir;
mod root_dir_cache;

pub use root_dir::{PathError, ReadFileError, RootDir, SubpackagesError};
pub use root_dir_cache::RootDirCache;
pub use vfs::execution_scope::ExecutionScope;

pub(crate) const DIRECTORY_ABILITIES: fio::Abilities =
    fio::Abilities::GET_ATTRIBUTES.union(fio::Abilities::ENUMERATE).union(fio::Abilities::TRAVERSE);

pub(crate) const ALLOWED_FLAGS: fio::Flags = fio::Flags::empty()
    .union(fio::MASK_KNOWN_PROTOCOLS)
    .union(fio::PERM_READABLE)
    .union(fio::PERM_EXECUTABLE)
    .union(fio::Flags::PERM_INHERIT_EXECUTE)
    .union(fio::Flags::FLAG_SEND_REPRESENTATION);

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("the meta.far was not found")]
    MissingMetaFar,

    #[error("while opening the meta.far")]
    OpenMetaFar(#[source] NonMetaStorageError),

    #[error("while instantiating a fuchsia archive reader")]
    ArchiveReader(#[source] fuchsia_archive::Error),

    #[error("meta.far has a path that is not valid utf-8: {path:?}")]
    NonUtf8MetaEntry {
        #[source]
        source: std::str::Utf8Error,
        path: Vec<u8>,
    },

    #[error("while reading meta/contents")]
    ReadMetaContents(#[source] fuchsia_archive::Error),

    #[error("while deserializing meta/contents")]
    DeserializeMetaContents(#[source] fuchsia_pkg::MetaContentsError),

    #[error("collision between a file and a directory at path: '{:?}'", path)]
    FileDirectoryCollision { path: String },

    #[error("the supplied RootDir already has a dropper set")]
    DropperAlreadySet,
}

impl From<&Error> for zx::Status {
    fn from(e: &Error) -> Self {
        use Error::*;
        match e {
            MissingMetaFar => zx::Status::NOT_FOUND,
            OpenMetaFar(e) => e.into(),
            DropperAlreadySet => zx::Status::INTERNAL,
            ArchiveReader(fuchsia_archive::Error::Read(_)) => zx::Status::IO,
            ArchiveReader(_) | ReadMetaContents(_) | DeserializeMetaContents(_) => {
                zx::Status::INVALID_ARGS
            }
            FileDirectoryCollision { .. } | NonUtf8MetaEntry { .. } => zx::Status::INVALID_ARGS,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum NonMetaStorageError {
    #[error("while reading blob")]
    ReadBlob(#[source] fuchsia_fs::file::ReadError),

    #[error("while opening blob")]
    OpenBlob(#[source] fuchsia_fs::node::OpenError),

    #[error("while making FIDL call")]
    Fidl(#[source] fidl::Error),

    #[error("while calling GetBackingMemory")]
    GetVmo(#[source] zx::Status),
}

impl NonMetaStorageError {
    pub fn is_not_found_error(&self) -> bool {
        match self {
            NonMetaStorageError::ReadBlob(e) => e.is_not_found_error(),
            NonMetaStorageError::OpenBlob(e) => e.is_not_found_error(),
            NonMetaStorageError::GetVmo(status) => *status == zx::Status::NOT_FOUND,
            _ => false,
        }
    }
}

impl From<&NonMetaStorageError> for zx::Status {
    fn from(e: &NonMetaStorageError) -> Self {
        if e.is_not_found_error() { zx::Status::NOT_FOUND } else { zx::Status::INTERNAL }
    }
}

/// The storage that provides the non-meta files (accessed by hash) of a package-directory (e.g.
/// blobfs).
pub trait NonMetaStorage: Send + Sync + Sized + 'static {
    /// Open a non-meta file by hash. `scope` may complete while there are still open connections.
    fn open(
        &self,
        _blob: &fuchsia_hash::Hash,
        _flags: fio::Flags,
        _scope: ExecutionScope,
        _object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status>;

    /// Get a read-only VMO for the blob.
    fn get_blob_vmo(
        &self,
        hash: &fuchsia_hash::Hash,
    ) -> impl Future<Output = Result<zx::Vmo, NonMetaStorageError>> + Send;

    /// Reads the contents of a blob.
    fn read_blob(
        &self,
        hash: &fuchsia_hash::Hash,
    ) -> impl Future<Output = Result<Vec<u8>, NonMetaStorageError>> + Send;
}

impl NonMetaStorage for blobfs::Client {
    fn open(
        &self,
        blob: &fuchsia_hash::Hash,
        flags: fio::Flags,
        scope: ExecutionScope,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        self.open_blob_for_read(blob, flags, scope, object_request)
    }

    async fn get_blob_vmo(
        &self,
        hash: &fuchsia_hash::Hash,
    ) -> Result<zx::Vmo, NonMetaStorageError> {
        self.get_blob_vmo(hash).await.map_err(|e| match e {
            blobfs::GetBlobVmoError::OpenBlob(e) => NonMetaStorageError::OpenBlob(e),
            blobfs::GetBlobVmoError::GetVmo(e) => NonMetaStorageError::GetVmo(e),
            blobfs::GetBlobVmoError::Fidl(e) => NonMetaStorageError::Fidl(e),
        })
    }

    async fn read_blob(&self, hash: &fuchsia_hash::Hash) -> Result<Vec<u8>, NonMetaStorageError> {
        let vmo = NonMetaStorage::get_blob_vmo(self, hash).await?;
        let content_size = vmo.get_content_size().map_err(|e| {
            NonMetaStorageError::ReadBlob(fuchsia_fs::file::ReadError::ReadError(e))
        })?;
        vmo.read_to_vec::<u8>(0, content_size)
            .map_err(|e| NonMetaStorageError::ReadBlob(fuchsia_fs::file::ReadError::ReadError(e)))
    }
}

/// Assumes the directory is a flat container and the files are named after their hashes.
impl NonMetaStorage for fio::DirectoryProxy {
    fn open(
        &self,
        blob: &fuchsia_hash::Hash,
        flags: fio::Flags,
        _scope: ExecutionScope,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        // If the FIDL call passes, errors will be communicated via the `object_request` channel.
        self.open(
            blob.to_string().as_str(),
            flags,
            &object_request.options(),
            object_request.take().into_channel(),
        )
        .map_err(|_fidl_error| zx::Status::PEER_CLOSED)
    }

    async fn get_blob_vmo(
        &self,
        hash: &fuchsia_hash::Hash,
    ) -> Result<zx::Vmo, NonMetaStorageError> {
        let proxy = fuchsia_fs::directory::open_file(self, &hash.to_string(), fio::PERM_READABLE)
            .await
            .map_err(NonMetaStorageError::OpenBlob)?;
        proxy
            .get_backing_memory(fio::VmoFlags::PRIVATE_CLONE | fio::VmoFlags::READ)
            .await
            .map_err(NonMetaStorageError::Fidl)?
            .map_err(|e| NonMetaStorageError::GetVmo(zx::Status::err_from_raw(e)))
    }

    async fn read_blob(&self, hash: &fuchsia_hash::Hash) -> Result<Vec<u8>, NonMetaStorageError> {
        fuchsia_fs::directory::read_file(self, &hash.to_string())
            .await
            .map_err(NonMetaStorageError::ReadBlob)
    }
}

/// Serves a package directory for the package with hash `meta_far` on `server_end`.
/// The connection rights are set by `flags`, used the same as the `flags` parameter of
///   fuchsia.io/Directory.Open.
pub fn serve(
    scope: vfs::execution_scope::ExecutionScope,
    non_meta_storage: impl NonMetaStorage,
    meta_far: fuchsia_hash::Hash,
    flags: fio::Flags,
    server_end: ServerEnd<fio::DirectoryMarker>,
) -> impl futures::Future<Output = Result<(), Error>> {
    serve_path(
        scope,
        non_meta_storage,
        meta_far,
        flags,
        vfs::Path::dot(),
        server_end.into_channel().into(),
    )
}

/// Serves a sub-`path` of a package directory for the package with hash `meta_far` on `server_end`.
///
/// The connection rights are set by `flags`, used the same as the `flags` parameter of
///   fuchsia.io/Directory.Open.
/// On error while loading the package metadata, closes the provided server end, sending an OnOpen
///   response with an error status if requested.
pub async fn serve_path(
    scope: vfs::execution_scope::ExecutionScope,
    non_meta_storage: impl NonMetaStorage,
    meta_far: fuchsia_hash::Hash,
    flags: fio::Flags,
    path: vfs::Path,
    server_end: ServerEnd<fio::NodeMarker>,
) -> Result<(), Error> {
    let root_dir = match RootDir::new(non_meta_storage, meta_far).await {
        Ok(d) => d,
        Err(e) => {
            let () = send_on_open_with_error(
                flags.contains(fio::Flags::FLAG_SEND_REPRESENTATION),
                server_end,
                (&e).into(),
            );
            return Err(e);
        }
    };

    ObjectRequest::new(flags, &fio::Options::default(), server_end.into_channel())
        .handle(|request| root_dir.open(scope, path, flags, request));
    Ok(())
}

fn usize_to_u64_safe(u: usize) -> u64 {
    let ret: u64 = u.try_into().unwrap();
    static_assertions::assert_eq_size_val!(u, ret);
    ret
}

/// RootDir takes an optional `OnRootDirDrop` value that will be dropped when the RootDir is
/// dropped.
///
/// This is useful because the VFS functions operate on `Arc<RootDir>`s (and create clones of the
/// `Arc`s in response to e.g. `Directory::open` calls), so this allows clients to perform actions
/// when the last clone of the `Arc<RootDir>` is dropped (which is frequently when the last
/// fuchsia.io connection closes).
///
/// The `ExecutionScope` used to serve the connection could also be used to notice when all the
/// `Arc<RootDir>`s are dropped, but only if the `Arc<RootDir>`s are only used by VFS. Tracking
/// when the `RootDir` itself is dropped allows non VFS uses of the `Arc<RootDir>`s.
pub trait OnRootDirDrop: Send + Sync + std::fmt::Debug {}
impl<T> OnRootDirDrop for T where T: Send + Sync + std::fmt::Debug {}

/// Takes a directory hierarchy and a directory in the hierarchy and returns all the directory's
/// children in alphabetical order.
///   `materialized_tree`: object relative path expressions of every file in a directory hierarchy
///   `dir`: the empty string (signifies the root dir) or a path to a subdir (must be an object
///          relative path expression plus a trailing slash)
/// Returns an empty vec if `dir` isn't in `materialized_tree`.
fn get_dir_children<'a>(
    materialized_tree: impl IntoIterator<Item = &'a str>,
    dir: &str,
) -> Vec<(EntryInfo, String)> {
    let mut added_entries = HashSet::new();
    let mut res = vec![];

    for path in materialized_tree {
        if let Some(path) = path.strip_prefix(dir) {
            match path.split_once('/') {
                None => {
                    // TODO(https://fxbug.dev/42161818) Replace .contains/.insert with .get_or_insert_owned when non-experimental.
                    if !added_entries.contains(path) {
                        res.push((
                            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File),
                            path.to_string(),
                        ));
                        added_entries.insert(path.to_string());
                    }
                }
                Some((first, _)) => {
                    if !added_entries.contains(first) {
                        res.push((
                            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory),
                            first.to_string(),
                        ));
                        added_entries.insert(first.to_string());
                    }
                }
            }
        }
    }

    // TODO(https://fxbug.dev/42162840) Remove this sort
    res.sort_by(|a, b| a.1.cmp(&b.1));
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_hash::Hash;
    use fuchsia_pkg_testing::PackageBuilder;
    use fuchsia_pkg_testing::blobfs::Fake as FakeBlobfs;
    use futures::StreamExt;
    use vfs::directory::helper::DirectlyMutable;

    #[fuchsia::test]
    async fn serve() {
        let (proxy, server_end) = fidl::endpoints::create_proxy();
        let package = PackageBuilder::new("just-meta-far").build().await.expect("created pkg");
        let (metafar_blob, _) = package.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);

        crate::serve(
            vfs::execution_scope::ExecutionScope::new(),
            blobfs_client,
            metafar_blob.merkle,
            fio::PERM_READABLE,
            server_end,
        )
        .await
        .unwrap();

        assert_eq!(
            fuchsia_fs::directory::readdir(&proxy).await.unwrap(),
            vec![fuchsia_fs::directory::DirEntry {
                name: "meta".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory
            }]
        );
    }

    #[fuchsia::test]
    async fn serve_path_open_root() {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let package = PackageBuilder::new("just-meta-far").build().await.expect("created pkg");
        let (metafar_blob, _) = package.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);

        crate::serve_path(
            vfs::execution_scope::ExecutionScope::new(),
            blobfs_client,
            metafar_blob.merkle,
            fio::PERM_READABLE,
            vfs::Path::validate_and_split(".").unwrap(),
            server_end.into_channel().into(),
        )
        .await
        .unwrap();

        assert_eq!(
            fuchsia_fs::directory::readdir(&proxy).await.unwrap(),
            vec![fuchsia_fs::directory::DirEntry {
                name: "meta".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory
            }]
        );
    }

    #[fuchsia::test]
    async fn serve_path_open_meta() {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::FileMarker>();
        let package = PackageBuilder::new("just-meta-far").build().await.expect("created pkg");
        let (metafar_blob, _) = package.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);

        crate::serve_path(
            vfs::execution_scope::ExecutionScope::new(),
            blobfs_client,
            metafar_blob.merkle,
            fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
            vfs::Path::validate_and_split("meta").unwrap(),
            server_end.into_channel().into(),
        )
        .await
        .unwrap();

        assert_eq!(
            fuchsia_fs::file::read_to_string(&proxy).await.unwrap(),
            metafar_blob.merkle.to_string(),
        );
    }

    #[fuchsia::test]
    async fn serve_path_open_missing_path_in_package() {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
        let package = PackageBuilder::new("just-meta-far").build().await.expect("created pkg");
        let (metafar_blob, _) = package.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);

        assert_matches!(
            crate::serve_path(
                vfs::execution_scope::ExecutionScope::new(),
                blobfs_client,
                metafar_blob.merkle,
                fio::PERM_READABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
                vfs::Path::validate_and_split("not-present").unwrap(),
                server_end.into_channel().into(),
            )
            .await,
            // serve_path succeeds in opening the package, but the forwarded open will discover
            // that the requested path does not exist.
            Ok(())
        );

        assert_eq!(node_into_on_open_status(proxy).await, Some(zx::Status::NOT_FOUND));
    }

    #[fuchsia::test]
    async fn serve_path_open_missing_package() {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
        let (_blobfs_fake, blobfs_client) = FakeBlobfs::new();

        assert_matches!(
            crate::serve_path(
                vfs::execution_scope::ExecutionScope::new(),
                blobfs_client,
                Hash::from([0u8; 32]),
                fio::PERM_READABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
                vfs::Path::validate_and_split(".").unwrap(),
                server_end.into_channel().into(),
            )
            .await,
            Err(Error::MissingMetaFar)
        );

        assert_eq!(node_into_on_open_status(proxy).await, Some(zx::Status::NOT_FOUND));
    }

    async fn node_into_on_open_status(node: fio::NodeProxy) -> Option<zx::Status> {
        // Handle either an io1 OnOpen Status or an io2 epitaph status, though only one will be
        // sent, determined by the open() API used.
        let mut events = node.take_event_stream();
        match events.next().await? {
            Ok(fio::NodeEvent::OnOpen_ { s: status, .. }) => zx::Status::try_from_raw(status),
            Ok(fio::NodeEvent::OnRepresentation { .. }) => None,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. }) => match epitaph.into() {
                Err(s) => Some(s),
                Ok(()) => Some(zx::Status::PEER_CLOSED),
            },
            other => panic!("unexpected stream event or error: {other:?}"),
        }
    }

    fn file() -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)
    }

    fn dir() -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }

    #[test]
    fn get_dir_children_root() {
        assert_eq!(get_dir_children([], ""), vec![]);
        assert_eq!(get_dir_children(["a"], ""), vec![(file(), "a".to_string())]);
        assert_eq!(
            get_dir_children(["a", "b"], ""),
            vec![(file(), "a".to_string()), (file(), "b".to_string())]
        );
        assert_eq!(
            get_dir_children(["b", "a"], ""),
            vec![(file(), "a".to_string()), (file(), "b".to_string())]
        );
        assert_eq!(get_dir_children(["a", "a"], ""), vec![(file(), "a".to_string())]);
        assert_eq!(get_dir_children(["a/b"], ""), vec![(dir(), "a".to_string())]);
        assert_eq!(
            get_dir_children(["a/b", "c"], ""),
            vec![(dir(), "a".to_string()), (file(), "c".to_string())]
        );
        assert_eq!(get_dir_children(["a/b/c"], ""), vec![(dir(), "a".to_string())]);
    }

    #[test]
    fn get_dir_children_subdir() {
        assert_eq!(get_dir_children([], "a/"), vec![]);
        assert_eq!(get_dir_children(["a"], "a/"), vec![]);
        assert_eq!(get_dir_children(["a", "b"], "a/"), vec![]);
        assert_eq!(get_dir_children(["a/b"], "a/"), vec![(file(), "b".to_string())]);
        assert_eq!(
            get_dir_children(["a/b", "a/c"], "a/"),
            vec![(file(), "b".to_string()), (file(), "c".to_string())]
        );
        assert_eq!(
            get_dir_children(["a/c", "a/b"], "a/"),
            vec![(file(), "b".to_string()), (file(), "c".to_string())]
        );
        assert_eq!(get_dir_children(["a/b", "a/b"], "a/"), vec![(file(), "b".to_string())]);
        assert_eq!(get_dir_children(["a/b/c"], "a/"), vec![(dir(), "b".to_string())]);
        assert_eq!(
            get_dir_children(["a/b/c", "a/d"], "a/"),
            vec![(dir(), "b".to_string()), (file(), "d".to_string())]
        );
        assert_eq!(get_dir_children(["a/b/c/d"], "a/"), vec![(dir(), "b".to_string())]);
    }

    const BLOB_CONTENTS: &[u8] = b"blob-contents";

    fn blob_contents_hash() -> Hash {
        fuchsia_merkle::root_from_slice(BLOB_CONTENTS)
    }

    #[fuchsia::test]
    async fn bootfs_get_vmo_blob() {
        let directory = vfs::directory::immutable::simple();
        directory
            .add_entry(blob_contents_hash().to_string(), vfs::file::read_only(BLOB_CONTENTS))
            .unwrap();
        let proxy = vfs::directory::serve_read_only(directory, ExecutionScope::new());

        let vmo = proxy.get_blob_vmo(&blob_contents_hash()).await.unwrap();
        assert_eq!(vmo.read_to_vec::<u8>(0, BLOB_CONTENTS.len() as u64).unwrap(), BLOB_CONTENTS);
    }

    #[fuchsia::test]
    async fn bootfs_read_blob() {
        let directory = vfs::directory::immutable::simple();
        directory
            .add_entry(blob_contents_hash().to_string(), vfs::file::read_only(BLOB_CONTENTS))
            .unwrap();
        let proxy = vfs::directory::serve_read_only(directory, ExecutionScope::new());

        assert_eq!(proxy.read_blob(&blob_contents_hash()).await.unwrap(), BLOB_CONTENTS);
    }

    #[fuchsia::test]
    async fn bootfs_get_vmo_blob_missing_blob() {
        let directory = vfs::directory::immutable::simple();
        let proxy = vfs::directory::serve_read_only(directory, ExecutionScope::new());

        let result = proxy.get_blob_vmo(&blob_contents_hash()).await;
        assert_matches!(result, Err(NonMetaStorageError::OpenBlob(e)) if e.is_not_found_error());
    }

    #[fuchsia::test]
    async fn bootfs_read_blob_missing_blob() {
        let directory = vfs::directory::immutable::simple();
        let proxy = vfs::directory::serve_read_only(directory, ExecutionScope::new());

        let result = proxy.read_blob(&blob_contents_hash()).await;
        assert_matches!(result, Err(NonMetaStorageError::ReadBlob(e)) if e.is_not_found_error());
    }

    #[fuchsia::test]
    async fn blobfs_get_vmo_blob() {
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(blob_contents_hash(), BLOB_CONTENTS);

        let vmo =
            NonMetaStorage::get_blob_vmo(&blobfs_client, &blob_contents_hash()).await.unwrap();
        assert_eq!(vmo.read_to_vec::<u8>(0, BLOB_CONTENTS.len() as u64).unwrap(), BLOB_CONTENTS);
    }

    #[fuchsia::test]
    async fn blobfs_read_blob() {
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(blob_contents_hash(), BLOB_CONTENTS);

        assert_eq!(blobfs_client.read_blob(&blob_contents_hash()).await.unwrap(), BLOB_CONTENTS);
    }

    #[fuchsia::test]
    async fn blobfs_get_vmo_blob_missing_blob() {
        let (_blobfs_fake, blobfs_client) = FakeBlobfs::new();

        let result = NonMetaStorage::get_blob_vmo(&blobfs_client, &blob_contents_hash()).await;
        assert_matches!(result, Err(e) if e.is_not_found_error());
    }

    #[fuchsia::test]
    async fn blobfs_read_blob_missing_blob() {
        let (_blobfs_fake, blobfs_client) = FakeBlobfs::new();

        let result = blobfs_client.read_blob(&blob_contents_hash()).await;
        assert_matches!(result, Err(e) if e.is_not_found_error());
    }
}
