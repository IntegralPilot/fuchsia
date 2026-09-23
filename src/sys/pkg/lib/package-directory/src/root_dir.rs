// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::meta_as_dir::MetaAsDir;
use crate::meta_subdir::MetaSubdir;
use crate::non_meta_subdir::NonMetaSubdir;
use crate::{Error, NonMetaStorageError, usize_to_u64_safe};
use fidl_fuchsia_io as fio;
use fuchsia_pkg::MetaContents;
use log::error;
use packed::PackedMap;
use std::sync::Arc;
use vfs::directory::entry::{EntryInfo, OpenRequest};
use vfs::directory::immutable::connection::ImmutableConnection;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::execution_scope::ExecutionScope;
use vfs::file::vmo::VmoFile;
use vfs::{ObjectRequestRef, ProtocolsExt as _, immutable_attributes};

/// The root directory of Fuchsia package.
#[derive(Debug)]
pub struct RootDir<S> {
    pub(crate) non_meta_storage: S,
    pub(crate) hash: fuchsia_hash::Hash,
    // The keys are object relative path expressions.
    pub(crate) meta_files: PackedMap<str, MetaFileLocation>,
    // The keys are object relative path expressions.
    pub(crate) non_meta_files: PackedMap<str, fuchsia_hash::Hash>,
    pub(crate) meta_far_vmo: zx::Vmo,
    dropper: Option<Box<dyn crate::OnRootDirDrop>>,
}

impl<S: crate::NonMetaStorage> RootDir<S> {
    /// Loads the package metadata given by `hash` from `non_meta_storage`, returning an object
    /// representing the package, backed by `non_meta_storage`.
    pub async fn new(non_meta_storage: S, hash: fuchsia_hash::Hash) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self::new_raw(non_meta_storage, hash, None).await?))
    }

    /// Loads the package metadata given by `hash` from `non_meta_storage`, returning an object
    /// representing the package, backed by `non_meta_storage`.
    /// Takes `dropper`, which will be dropped when the returned `RootDir` is dropped.
    pub async fn new_with_dropper(
        non_meta_storage: S,
        hash: fuchsia_hash::Hash,
        dropper: Box<dyn crate::OnRootDirDrop>,
    ) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self::new_raw(non_meta_storage, hash, Some(dropper)).await?))
    }

    /// Loads the package metadata given by `hash` from `non_meta_storage`, returning an object
    /// representing the package, backed by `non_meta_storage`.
    /// Takes `dropper`, which will be dropped when the returned `RootDir` is dropped.
    /// Like `new_with_dropper` except the returned `RootDir` is not in an `Arc`.
    pub async fn new_raw(
        non_meta_storage: S,
        hash: fuchsia_hash::Hash,
        dropper: Option<Box<dyn crate::OnRootDirDrop>>,
    ) -> Result<Self, Error> {
        let meta_far_vmo = non_meta_storage.get_blob_vmo(&hash).await.map_err(|e| {
            if e.is_not_found_error() { Error::MissingMetaFar } else { Error::OpenMetaFar(e) }
        })?;
        let (meta_files, non_meta_files) = load_package_metadata(&meta_far_vmo)?;

        Ok(RootDir { non_meta_storage, hash, meta_files, non_meta_files, meta_far_vmo, dropper })
    }

    /// Sets the dropper. If the dropper was already set, returns `dropper` in the error.
    pub fn set_dropper(
        &mut self,
        dropper: Box<dyn crate::OnRootDirDrop>,
    ) -> Result<(), Box<dyn crate::OnRootDirDrop>> {
        match self.dropper {
            Some(_) => Err(dropper),
            None => {
                self.dropper = Some(dropper);
                Ok(())
            }
        }
    }

    /// Returns the contents, if present, of the file at object relative path expression `path`.
    /// https://fuchsia.dev/fuchsia-src/concepts/process/namespaces?hl=en#object_relative_path_expressions
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>, ReadFileError> {
        if let Some(hash) = self.non_meta_files.get(path) {
            self.non_meta_storage.read_blob(hash).await.map_err(ReadFileError::ReadBlob)
        } else if let Some(location) = self.meta_files.get(path) {
            self.meta_far_vmo
                .read_to_vec(location.offset, location.length)
                .map_err(ReadFileError::ReadMetaFile)
        } else {
            Err(ReadFileError::NoFileAtPath { path: path.to_string() })
        }
    }

    /// Returns `true` iff there is a file at `path`, an object relative path expression.
    /// https://fuchsia.dev/fuchsia-src/concepts/process/namespaces?hl=en#object_relative_path_expressions
    pub fn has_file(&self, path: &str) -> bool {
        self.non_meta_files.contains_key(path) || self.meta_files.contains_key(path)
    }

    /// Returns the hash of the package.
    pub fn hash(&self) -> &fuchsia_hash::Hash {
        &self.hash
    }

    /// Returns an iterator of the hashes of files stored externally to the package meta.far.
    /// May return duplicates.
    pub fn external_file_hashes(&self) -> impl ExactSizeIterator<Item = &fuchsia_hash::Hash> {
        self.non_meta_files.values()
    }

    /// Returns the path of the package as indicated by the "meta/package" file.
    pub async fn path(&self) -> Result<fuchsia_pkg::PackagePath, PathError> {
        Ok(fuchsia_pkg::MetaPackage::deserialize(&self.read_file("meta/package").await?[..])?
            .into_path())
    }

    /// Returns the subpackages of the package.
    pub async fn subpackages(&self) -> Result<fuchsia_pkg::MetaSubpackages, SubpackagesError> {
        let contents = match self.read_file(fuchsia_pkg::MetaSubpackages::PATH).await {
            Ok(contents) => contents,
            Err(ReadFileError::NoFileAtPath { .. }) => {
                return Ok(fuchsia_pkg::MetaSubpackages::default());
            }
            Err(e) => Err(e)?,
        };

        Ok(fuchsia_pkg::MetaSubpackages::deserialize(&*contents)?)
    }

    /// Creates a file that contains the package's hash.
    fn create_meta_as_file(&self) -> Result<Arc<VmoFile>, zx::Status> {
        let file_contents = self.hash.to_string();
        let vmo = zx::Vmo::create(usize_to_u64_safe(file_contents.len()))?;
        let () = vmo.write(file_contents.as_bytes(), 0)?;
        Ok(VmoFile::new_with_inode(vmo, /*inode*/ 1))
    }

    /// Creates and returns a meta file if one exists at `path`.
    pub(crate) fn get_meta_file(&self, path: &str) -> Result<Option<Arc<VmoFile>>, zx::Status> {
        // The FAR spec requires 4 KiB alignment of content chunks [1], so offset will
        // always be page-aligned, because pages are required [2] to be a power of 2 and at
        // least 4 KiB.
        // [1] https://fuchsia.dev/fuchsia-src/concepts/source_code/archive_format#content_chunk
        // [2] https://fuchsia.dev/fuchsia-src/reference/syscalls/system_get_page_size
        // TODO(https://fxbug.dev/42162525) Need to manually zero the end of the VMO if
        // zx_system_get_page_size() > 4K.
        assert_eq!(zx::system_get_page_size(), 4096);

        let location = match self.meta_files.get(path) {
            Some(location) => location,
            None => return Ok(None),
        };
        let vmo = self
            .meta_far_vmo
            .create_child(
                zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE | zx::VmoChildOptions::NO_WRITE,
                location.offset,
                location.length,
            )
            .map_err(|e| {
                error!("Error creating child vmo for meta file {:?}", e);
                zx::Status::INTERNAL
            })?;

        Ok(Some(VmoFile::new_with_inode(vmo, /*inode*/ 1)))
    }

    /// Creates and returns a `MetaSubdir` if one exists at `path`. `path` must end in '/'.
    pub(crate) fn get_meta_subdir(self: &Arc<Self>, path: String) -> Option<Arc<MetaSubdir<S>>> {
        debug_assert!(path.ends_with("/"));
        if let Some((k, _)) = self.meta_files.range(path.as_str()..).next()
            && k.starts_with(&path)
        {
            return Some(MetaSubdir::new(self.clone(), path));
        }
        None
    }

    /// Creates and returns a `NonMetaSubdir` if one exists at `path`. `path` must end in '/'.
    pub(crate) fn get_non_meta_subdir(
        self: &Arc<Self>,
        path: String,
    ) -> Option<Arc<NonMetaSubdir<S>>> {
        debug_assert!(path.ends_with("/"));
        if let Some((k, _)) = self.non_meta_files.range(path.as_str()..).next()
            && k.starts_with(&path)
        {
            return Some(NonMetaSubdir::new(self.clone(), path));
        }
        None
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ReadFileError {
    #[error("reading blob")]
    ReadBlob(#[source] NonMetaStorageError),

    #[error("reading meta file")]
    ReadMetaFile(#[source] zx::Status),

    #[error("no file exists at path: {path:?}")]
    NoFileAtPath { path: String },
}

#[derive(thiserror::Error, Debug)]
pub enum SubpackagesError {
    #[error("reading manifest")]
    Read(#[from] ReadFileError),

    #[error("parsing manifest")]
    Parse(#[from] fuchsia_pkg::MetaSubpackagesError),
}

#[derive(thiserror::Error, Debug)]
pub enum PathError {
    #[error("reading meta/package")]
    Read(#[from] ReadFileError),

    #[error("parsing meta/package")]
    Parse(#[from] fuchsia_pkg::MetaPackageError),
}

impl<S: crate::NonMetaStorage> vfs::directory::entry::DirectoryEntry for RootDir<S> {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.open_dir(self)
    }
}

impl<S: crate::NonMetaStorage> vfs::directory::entry::GetEntryInfo for RootDir<S> {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl<S: crate::NonMetaStorage> vfs::node::Node for RootDir<S> {
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

impl<S: crate::NonMetaStorage> vfs::directory::entry_container::Directory for RootDir<S> {
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
        let canonical_path = path.as_ref().strip_suffix('/').unwrap_or_else(|| path.as_ref());

        if canonical_path == "meta" {
            // This branch is done here instead of in MetaAsDir so that Clone'ing MetaAsDir yields
            // MetaAsDir. See the MetaAsDir::open impl for more.

            // TODO(https://fxbug.dev/328485661): consider retrieving the merkle root by retrieving
            // the attribute instead of opening as a file to read the merkle root content.

            // To remain POSIX compliant, we must default to opening meta as a file unless the
            // directory protocol (i.e. O_DIRECTORY) is explicitly requested.
            let open_meta_as_dir = flags.is_dir_allowed() && !flags.is_file_allowed();
            if !open_meta_as_dir {
                if path.is_dir() {
                    return Err(zx::Status::NOT_DIR);
                }
                let file = self.create_meta_as_file().map_err(|e| {
                    error!("Error creating the meta file: {:?}", e);
                    zx::Status::INTERNAL
                })?;
                return vfs::file::serve(file, scope, &flags, object_request);
            }
            return MetaAsDir::new(self).open(scope, vfs::Path::dot(), flags, object_request);
        }

        if canonical_path.starts_with("meta/") {
            if let Some(file) = self.get_meta_file(canonical_path)? {
                if path.is_dir() {
                    return Err(zx::Status::NOT_DIR);
                }
                return vfs::file::serve(file, scope, &flags, object_request);
            }

            if let Some(subdir) = self.get_meta_subdir(canonical_path.to_string() + "/") {
                return subdir.open(scope, vfs::Path::dot(), flags, object_request);
            }
            return Err(zx::Status::NOT_FOUND);
        }

        if let Some(blob) = self.non_meta_files.get(canonical_path) {
            if path.is_dir() {
                return Err(zx::Status::NOT_DIR);
            }
            return self.non_meta_storage.open(blob, flags, scope, object_request);
        }

        if let Some(subdir) = self.get_non_meta_subdir(canonical_path.to_string() + "/") {
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
            // Add "meta/placeholder" file so the "meta" dir is included in the results
            &crate::get_dir_children(self.non_meta_files.keys().chain(["meta/placeholder"]), ""),
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

#[allow(clippy::type_complexity)]
fn load_package_metadata(
    meta_far_vmo: &zx::Vmo,
) -> Result<(PackedMap<str, MetaFileLocation>, PackedMap<str, fuchsia_hash::Hash>), Error> {
    let stream =
        zx::Stream::create(zx::StreamOptions::MODE_READ, meta_far_vmo, 0).map_err(|e| {
            Error::OpenMetaFar(NonMetaStorageError::ReadBlob(
                fuchsia_fs::file::ReadError::ReadError(e),
            ))
        })?;

    let mut reader = fuchsia_archive::Reader::new(stream).map_err(Error::ArchiveReader)?;
    let mut meta_files = {
        let reader_list = reader.list();
        PackedMap::<str, MetaFileLocation>::with_capacity(
            reader_list.len(),
            reader_list.map(|entry| entry.path().len()).sum(),
        )
    };
    let reader_list = reader.list();
    // FAR files guarantee that entries are sorted by path, so we can use
    // `append_or_update` directly to avoid `PackedMapBuilder`'s sorting overhead.
    for entry in reader_list {
        let path = std::str::from_utf8(entry.path())
            .map_err(|source| Error::NonUtf8MetaEntry { source, path: entry.path().to_owned() })?;
        if path.starts_with("meta/") {
            for (i, _) in path.match_indices('/').skip(1) {
                if meta_files.contains_key(&path[..i]) {
                    return Err(Error::FileDirectoryCollision { path: path[..i].to_string() });
                }
            }
            meta_files
                .append_or_update(
                    path,
                    MetaFileLocation { offset: entry.offset(), length: entry.length() },
                )
                .expect("FAR entries are sorted");
        }
    }

    let meta_contents_bytes =
        reader.read_file(b"meta/contents").map_err(Error::ReadMetaContents)?;

    let mut non_meta_files = MetaContents::deserialize(&meta_contents_bytes[..])
        .map_err(Error::DeserializeMetaContents)?
        .into_contents();

    // We'll be keeping these packed files around for a while, so shrink to fit
    // to avoid using too much memory.
    non_meta_files.shrink_to_fit();

    Ok((meta_files, non_meta_files))
}

/// Location of a meta file's contents within a meta.far
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MetaFileLocation {
    offset: u64,
    length: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy;
    use fuchsia_fs::directory::{DirEntry, DirentKind};
    use fuchsia_pkg_testing::PackageBuilder;
    use fuchsia_pkg_testing::blobfs::Fake as FakeBlobfs;
    use futures::TryStreamExt as _;
    use pretty_assertions::assert_eq;
    use std::io::Cursor;

    struct TestEnv {
        _blobfs_fake: FakeBlobfs,
        root_dir: Arc<RootDir<blobfs::Client>>,
    }

    impl TestEnv {
        async fn with_subpackages(
            subpackages_content: Option<&[u8]>,
        ) -> (Self, Arc<RootDir<blobfs::Client>>) {
            let mut pkg = PackageBuilder::new("base-package-0")
                .add_resource_at("resource", "blob-contents".as_bytes())
                .add_resource_at("dir/file", "bloblob".as_bytes())
                .add_resource_at("meta/file", "meta-contents0".as_bytes())
                .add_resource_at("meta/dir/file", "meta-contents1".as_bytes());
            if let Some(subpackages_content) = subpackages_content {
                pkg = pkg.add_resource_at(fuchsia_pkg::MetaSubpackages::PATH, subpackages_content);
            }
            let pkg = pkg.build().await.unwrap();
            let (metafar_blob, content_blobs) = pkg.contents();
            let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
            blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
            for (hash, bytes) in content_blobs {
                blobfs_fake.add_blob(hash, bytes);
            }

            let root_dir = RootDir::new(blobfs_client, metafar_blob.merkle).await.unwrap();
            (Self { _blobfs_fake: blobfs_fake, root_dir: root_dir.clone() }, root_dir)
        }

        async fn new() -> (Self, fio::DirectoryProxy) {
            let (env, root) = Self::with_subpackages(None).await;
            (env, vfs::directory::serve_read_only(root, ExecutionScope::new()))
        }
    }

    #[fuchsia::test]
    async fn new_missing_meta_far_error() {
        let (_blobfs_fake, blobfs_client) = FakeBlobfs::new();
        assert_matches!(
            RootDir::new(blobfs_client, [0; 32].into()).await,
            Err(Error::MissingMetaFar)
        );
    }

    #[fuchsia::test]
    async fn new_rejects_invalid_utf8() {
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        let mut meta_far = vec![];
        let () = fuchsia_archive::write(
            &mut meta_far,
            std::collections::BTreeMap::from_iter([(
                b"\xff",
                (0, Box::new("".as_bytes()) as Box<dyn std::io::Read>),
            )]),
        )
        .unwrap();
        let hash = fuchsia_merkle::root_from_slice(&meta_far);
        let () = blobfs_fake.add_blob(hash, meta_far);

        assert_matches!(
            RootDir::new(blobfs_client, hash).await,
            Err(Error::NonUtf8MetaEntry{path, ..})
                if path == vec![255]
        );
    }

    #[fuchsia::test]
    async fn new_initializes_maps() {
        let (_env, root_dir) = TestEnv::with_subpackages(None).await;

        let meta_files = PackedMap::from([
            ("meta/contents", MetaFileLocation { offset: 4096, length: 148 }),
            ("meta/package", MetaFileLocation { offset: 20480, length: 39 }),
            ("meta/file", MetaFileLocation { offset: 12288, length: 14 }),
            ("meta/dir/file", MetaFileLocation { offset: 8192, length: 14 }),
            ("meta/fuchsia.abi/abi-revision", MetaFileLocation { offset: 16384, length: 8 }),
        ]);
        assert_eq!(root_dir.meta_files, meta_files);

        let non_meta_files = PackedMap::from([
            (
                "resource",
                "bd905f783ceae4c5ba8319703d7505ab363733c2db04c52c8405603a02922b15"
                    .parse::<fuchsia_hash::Hash>()
                    .unwrap(),
            ),
            (
                "dir/file",
                "5f615dd575994fcbcc174974311d59de258d93cd523d5cb51f0e139b53c33201"
                    .parse::<fuchsia_hash::Hash>()
                    .unwrap(),
            ),
        ]);
        assert_eq!(root_dir.non_meta_files, non_meta_files);
    }

    #[fuchsia::test]
    async fn rejects_meta_file_collisions() {
        let pkg = PackageBuilder::new("base-package-0")
            .add_resource_at("meta/dir/file", "meta-contents0".as_bytes())
            .build()
            .await
            .unwrap();

        // Manually modify the meta.far to contain a "meta/dir" entry.
        let (metafar_blob, _) = pkg.contents();
        let mut metafar =
            fuchsia_archive::Reader::new(Cursor::new(&metafar_blob.contents)).unwrap();
        let mut entries = std::collections::BTreeMap::new();
        let farentries =
            metafar.list().map(|entry| (entry.path().to_vec(), entry.length())).collect::<Vec<_>>();
        for (path, length) in farentries {
            let contents = metafar.read_file(&path).unwrap();
            entries
                .insert(path, (length, Box::new(Cursor::new(contents)) as Box<dyn std::io::Read>));
        }
        let extra_contents = b"meta-contents1";
        entries.insert(
            b"meta/dir".to_vec(),
            (
                extra_contents.len() as u64,
                Box::new(Cursor::new(extra_contents)) as Box<dyn std::io::Read>,
            ),
        );

        let mut metafar: Vec<u8> = vec![];
        let () = fuchsia_archive::write(&mut metafar, entries).unwrap();
        let merkle = fuchsia_merkle::root_from_slice(&metafar);

        // Verify it fails to load with the expected error.
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(merkle, &metafar);

        match RootDir::new(blobfs_client, merkle).await {
            Ok(_) => panic!("this should not be reached!"),
            Err(Error::FileDirectoryCollision { path }) => {
                assert_eq!(path, "meta/dir".to_string());
            }
            Err(e) => panic!("Expected collision error, receieved {e:?}"),
        };
    }

    #[fuchsia::test]
    async fn read_file() {
        let (_env, root_dir) = TestEnv::with_subpackages(None).await;

        assert_eq!(root_dir.read_file("resource").await.unwrap().as_slice(), b"blob-contents");
        assert_eq!(root_dir.read_file("meta/file").await.unwrap().as_slice(), b"meta-contents0");
        assert_matches!(
            root_dir.read_file("missing").await.unwrap_err(),
            ReadFileError::NoFileAtPath{path} if path == "missing"
        );
    }

    #[fuchsia::test]
    async fn has_file() {
        let (_env, root_dir) = TestEnv::with_subpackages(None).await;

        assert!(root_dir.has_file("resource"));
        assert!(root_dir.has_file("meta/file"));
        assert_eq!(root_dir.has_file("missing"), false);
    }

    #[fuchsia::test]
    async fn external_file_hashes() {
        let (_env, root_dir) = TestEnv::with_subpackages(None).await;

        let mut actual = root_dir.external_file_hashes().copied().collect::<Vec<_>>();
        actual.sort();
        assert_eq!(
            actual,
            vec![
                "5f615dd575994fcbcc174974311d59de258d93cd523d5cb51f0e139b53c33201".parse().unwrap(),
                "bd905f783ceae4c5ba8319703d7505ab363733c2db04c52c8405603a02922b15".parse().unwrap()
            ]
        );
    }

    #[fuchsia::test]
    async fn path() {
        let (_env, root_dir) = TestEnv::with_subpackages(None).await;

        assert_eq!(
            root_dir.path().await.unwrap(),
            "base-package-0/0".parse::<fuchsia_pkg::PackagePath>().unwrap()
        );
    }

    #[fuchsia::test]
    async fn subpackages_present() {
        let subpackages = fuchsia_pkg::MetaSubpackages::from_iter([(
            fuchsia_url::RelativePackageUrl::parse("subpackage-name").unwrap(),
            "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap(),
        )]);
        let mut subpackages_bytes = vec![];
        let () = subpackages.serialize(&mut subpackages_bytes).unwrap();
        let (_env, root_dir) = TestEnv::with_subpackages(Some(&*subpackages_bytes)).await;

        assert_eq!(root_dir.subpackages().await.unwrap(), subpackages);
    }

    #[fuchsia::test]
    async fn subpackages_absent() {
        let (_env, root_dir) = TestEnv::with_subpackages(None).await;

        assert_eq!(root_dir.subpackages().await.unwrap(), fuchsia_pkg::MetaSubpackages::default());
    }

    #[fuchsia::test]
    async fn subpackages_error() {
        let (_env, root_dir) = TestEnv::with_subpackages(Some(b"invalid-json")).await;

        assert_matches!(root_dir.subpackages().await, Err(SubpackagesError::Parse(_)));
    }

    /// Ensure connections to a [`RootDir`] cannot be created as mutable (i.e. with
    /// [`fio::PERM_WRITABLE`]). This ensures that the VFS will disallow any attempts to create a
    /// new file/directory, modify the attributes of any nodes, open any files as writable.
    #[fuchsia::test]
    async fn root_dir_cannot_be_served_as_mutable() {
        let (_env, root_dir) = TestEnv::with_subpackages(None).await;
        let proxy = vfs::directory::serve(root_dir, ExecutionScope::new(), fio::PERM_WRITABLE);
        assert_matches!(
            proxy.take_event_stream().try_next().await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx::Status::NOT_SUPPORTED
        );
    }

    #[fuchsia::test]
    async fn root_dir_readdir() {
        let (_env, root_dir) = TestEnv::new().await;
        assert_eq!(
            fuchsia_fs::directory::readdir_inclusive(&root_dir).await.unwrap(),
            vec![
                DirEntry { name: ".".to_string(), kind: DirentKind::Directory },
                DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
                DirEntry { name: "meta".to_string(), kind: DirentKind::Directory },
                DirEntry { name: "resource".to_string(), kind: DirentKind::File }
            ]
        );
    }

    #[fuchsia::test]
    async fn root_dir_get_attributes() {
        let (_env, root_dir) = TestEnv::new().await;
        let (mutable_attributes, immutable_attributes) =
            root_dir.get_attributes(fio::NodeAttributesQuery::all()).await.unwrap().unwrap();
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
    async fn root_dir_watch_not_supported() {
        let (_env, root_dir) = TestEnv::new().await;
        let (_client, server) = fidl::endpoints::create_endpoints();
        assert_eq!(
            zx::Status::ok(root_dir.watch(fio::WatchMask::empty(), 0, server).await.unwrap()),
            Err(zx::Status::NOT_SUPPORTED)
        );
    }

    #[fuchsia::test]
    async fn root_dir_open_non_meta_file() {
        let (_env, root_dir) = TestEnv::new().await;
        let proxy = fuchsia_fs::directory::open_file(&root_dir, "resource", fio::PERM_READABLE)
            .await
            .unwrap();
        assert_eq!(fuchsia_fs::file::read(&proxy).await.unwrap(), b"blob-contents".to_vec());
    }

    #[fuchsia::test]
    async fn root_dir_open_non_meta_file_get_attributes() {
        let (env, root_dir) = TestEnv::new().await;
        let proxy = fuchsia_fs::directory::open_file(&root_dir, "resource", fio::PERM_READABLE)
            .await
            .unwrap();
        let expected_hash = env.root_dir.non_meta_files.get("resource").unwrap();
        let attrs =
            proxy.get_attributes(fio::NodeAttributesQuery::ROOT_HASH).await.unwrap().unwrap();
        assert_eq!(attrs.1.root_hash, Some(expected_hash.as_bytes().to_vec()));
    }

    #[fuchsia::test]
    async fn root_dir_open_meta_as_file() {
        let (env, root_dir) = TestEnv::new().await;
        let proxy =
            fuchsia_fs::directory::open_file(&root_dir, "meta", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            fuchsia_fs::file::read(&proxy).await.unwrap(),
            env.root_dir.hash.to_string().as_bytes()
        );
        // Ensure the connection is cloned correctly (i.e. we don't get meta-as-dir).
        let (cloned_proxy, server_end) = create_proxy::<fio::FileMarker>();
        proxy.clone(server_end.into_channel().into()).unwrap();
        assert_eq!(
            fuchsia_fs::file::read(&cloned_proxy).await.unwrap(),
            env.root_dir.hash.to_string().as_bytes()
        );
    }

    #[fuchsia::test]
    async fn root_dir_open_meta_as_dir() {
        let (_env, root_dir) = TestEnv::new().await;
        for path in ["meta", "meta/"] {
            let proxy = fuchsia_fs::directory::open_directory(&root_dir, path, fio::PERM_READABLE)
                .await
                .unwrap();
            assert_eq!(
                fuchsia_fs::directory::readdir(&proxy).await.unwrap(),
                vec![
                    DirEntry { name: "contents".to_string(), kind: DirentKind::File },
                    DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
                    DirEntry { name: "file".to_string(), kind: DirentKind::File },
                    DirEntry { name: "fuchsia.abi".to_string(), kind: DirentKind::Directory },
                    DirEntry { name: "package".to_string(), kind: DirentKind::File },
                ]
            );
            // Ensure the connection is cloned correctly (i.e. we don't get meta-as-file).
            let (cloned_proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
            proxy.clone(server_end.into_channel().into()).unwrap();
            assert_eq!(
                fuchsia_fs::directory::readdir(&cloned_proxy).await.unwrap(),
                vec![
                    DirEntry { name: "contents".to_string(), kind: DirentKind::File },
                    DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
                    DirEntry { name: "file".to_string(), kind: DirentKind::File },
                    DirEntry { name: "fuchsia.abi".to_string(), kind: DirentKind::Directory },
                    DirEntry { name: "package".to_string(), kind: DirentKind::File },
                ]
            );
        }
    }

    #[fuchsia::test]
    async fn root_dir_open_meta_as_node() {
        let (_env, root_dir) = TestEnv::new().await;
        for path in ["meta", "meta/"] {
            let proxy = fuchsia_fs::directory::open_node(
                &root_dir,
                path,
                fio::Flags::PROTOCOL_NODE
                    | fio::Flags::PROTOCOL_DIRECTORY
                    | fio::Flags::PERM_GET_ATTRIBUTES,
            )
            .await
            .unwrap();
            let (mutable_attributes, immutable_attributes) = proxy
                .get_attributes(
                    fio::NodeAttributesQuery::PROTOCOLS | fio::NodeAttributesQuery::ABILITIES,
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                fio::NodeAttributes2 { mutable_attributes, immutable_attributes },
                immutable_attributes!(
                    fio::NodeAttributesQuery::PROTOCOLS | fio::NodeAttributesQuery::ABILITIES,
                    Immutable {
                        protocols: fio::NodeProtocolKinds::DIRECTORY,
                        abilities: crate::DIRECTORY_ABILITIES
                    }
                )
            );
        }
        // We should also be able to open the meta file as a node.
        let proxy = fuchsia_fs::directory::open_node(
            &root_dir,
            "meta",
            fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES,
        )
        .await
        .unwrap();
        let (mutable_attributes, immutable_attributes) = proxy
            .get_attributes(
                fio::NodeAttributesQuery::PROTOCOLS | fio::NodeAttributesQuery::ABILITIES,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            fio::NodeAttributes2 { mutable_attributes, immutable_attributes },
            immutable_attributes!(
                fio::NodeAttributesQuery::PROTOCOLS | fio::NodeAttributesQuery::ABILITIES,
                Immutable {
                    protocols: fio::NodeProtocolKinds::FILE,
                    abilities: fio::Abilities::READ_BYTES | fio::Abilities::GET_ATTRIBUTES,
                }
            )
        );
    }

    #[fuchsia::test]
    async fn root_dir_open_meta_file() {
        let (_env, root_dir) = TestEnv::new().await;
        let proxy = fuchsia_fs::directory::open_file(&root_dir, "meta/file", fio::PERM_READABLE)
            .await
            .unwrap();
        assert_eq!(fuchsia_fs::file::read(&proxy).await.unwrap(), b"meta-contents0".to_vec());
    }

    #[fuchsia::test]
    async fn root_dir_open_meta_subdir() {
        let (_env, root_dir) = TestEnv::new().await;
        for path in ["meta/dir", "meta/dir/"] {
            let proxy = fuchsia_fs::directory::open_directory(&root_dir, path, fio::PERM_READABLE)
                .await
                .unwrap();
            assert_eq!(
                fuchsia_fs::directory::readdir(&proxy).await.unwrap(),
                vec![DirEntry { name: "file".to_string(), kind: DirentKind::File }]
            );
        }
    }

    #[fuchsia::test]
    async fn root_dir_open_non_meta_subdir() {
        let (_env, root_dir) = TestEnv::new().await;
        for path in ["dir", "dir/"] {
            let proxy = fuchsia_fs::directory::open_directory(&root_dir, path, fio::PERM_READABLE)
                .await
                .unwrap();
            assert_eq!(
                fuchsia_fs::directory::readdir(&proxy).await.unwrap(),
                vec![DirEntry { name: "file".to_string(), kind: DirentKind::File }]
            );
        }
    }
}
