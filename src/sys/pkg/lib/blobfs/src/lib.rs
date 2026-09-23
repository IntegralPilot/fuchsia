// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Typesafe wrappers around the /blob filesystem.

use fidl::endpoints::ClientEnd;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fuchsia_hash::{Hash, ParseHashError};
use futures::{StreamExt as _, stream};
use log::{error, info};
use std::collections::HashSet;
use thiserror::Error;
use vfs::execution_scope::ExecutionScope;
use vfs::file::StreamIoConnection;
use vfs::{ObjectRequest, ObjectRequestRef, ProtocolsExt};
use zx::{self as zx, Status};

pub mod mock;
pub use mock::Mock;

#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum BlobStatusError {
    #[error("this client was not created with a blob creator so it cannot write blobs")]
    WritingNotConfigured,

    #[error("the fidl call returned an unexpected error")]
    NeedsOverwrite(#[source] Status),
}

/// Blobfs client errors.
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum BlobfsError {
    #[error("while opening blobfs dir")]
    OpenDir(#[from] fuchsia_fs::node::OpenError),

    #[error("while cloning the blobfs dir")]
    CloneDir(#[from] fuchsia_fs::node::CloneError),

    #[error("while listing blobfs dir")]
    ReadDir(#[source] fuchsia_fs::directory::EnumerateError),

    #[error("while deleting blob")]
    Unlink(#[source] Status),

    #[error("while sync'ing")]
    Sync(#[source] Status),

    #[error("while parsing blob merkle hash")]
    ParseHash(#[from] ParseHashError),

    #[error("FIDL error")]
    Fidl(#[from] fidl::Error),

    #[error("while connecting to fuchsia.fxfs/BlobCreator")]
    ConnectToBlobCreator(#[source] anyhow::Error),

    #[error("while connecting to fuchsia.fxfs/BlobReader")]
    ConnectToBlobReader(#[source] anyhow::Error),

    #[error("while setting the VmexResource")]
    InitVmexResource(#[source] anyhow::Error),

    #[error("directory operation requested but blobfs directory was not configured")]
    DirectoryNotConfigured,

    #[error("while checking NeedsOverwrite for blob status")]
    BlobStatus(BlobStatusError),
}

/// An error encountered while creating a blob
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum CreateError {
    #[error("the blob already exists or is being concurrently written")]
    AlreadyExists,

    #[error("while creating the blob")]
    Io(#[source] fuchsia_fs::node::OpenError),

    #[error("while converting the proxy into a client end")]
    ConvertToClientEnd,

    #[error("FIDL error")]
    Fidl(#[from] fidl::Error),

    #[error("while calling fuchsia.fxfs/BlobCreator.Create: {0:?}")]
    BlobCreator(ffxfs::CreateBlobError),

    #[error("this client was not created with a blob creator so it cannot write blobs")]
    WritingNotConfigured,
}

/// The response to a `BlobCreator.NeedsOverwrite` call, excepting unexpected internal errors.
pub enum BlobStatus {
    /// The blob is present and considered up to date.
    UpToDate,

    /// The blob is present, but should be overwritten.
    NeedsOverwrite,

    /// The blob is not present.
    Absent,
}

impl From<ffxfs::CreateBlobError> for CreateError {
    fn from(e: ffxfs::CreateBlobError) -> Self {
        match e {
            ffxfs::CreateBlobError::AlreadyExists => CreateError::AlreadyExists,
            e @ ffxfs::CreateBlobError::Internal => CreateError::BlobCreator(e),
        }
    }
}

/// A builder for [`Client`]
#[derive(Default)]
pub struct ClientBuilder {
    readable: bool,
    writable: bool,
    executable: bool,
    creator: bool,
}

impl ClientBuilder {
    /// Opens the /blob directory in the component's namespace with readable, writable, and/or
    /// executable flags. Connects to the fuchsia.fxfs.BlobCreator and BlobReader if requested.
    /// Connects to and initializes the VmexResource if `use_vmex` is set. Returns a `Client`.
    pub async fn build(self) -> Result<Client, BlobfsError> {
        let mut flags = fio::Flags::empty();
        if self.readable {
            flags |= fio::PERM_READABLE
        }
        if self.writable {
            flags |= fio::PERM_WRITABLE
        }
        if self.executable {
            flags |= fio::PERM_EXECUTABLE
        }

        let dir = if !flags.is_empty() {
            Some(fuchsia_fs::directory::open_in_namespace("/blob", flags)?)
        } else {
            None
        };

        if let Ok(client) = fuchsia_component::client::connect_to_protocol::<
            fidl_fuchsia_kernel::VmexResourceMarker,
        >() && let Ok(vmex) = client.get().await
        {
            info!("Got vmex resource");
            vmo_blob::init_vmex_resource(vmex).map_err(BlobfsError::InitVmexResource)?;
        }
        let reader = fuchsia_component::client::connect_to_protocol::<ffxfs::BlobReaderMarker>()
            .map_err(BlobfsError::ConnectToBlobReader)?;
        let creator = if self.writable || self.creator {
            Some(
                fuchsia_component::client::connect_to_protocol::<ffxfs::BlobCreatorMarker>()
                    .map_err(BlobfsError::ConnectToBlobCreator)?,
            )
        } else {
            None
        };

        Ok(Client { dir, creator, reader })
    }

    /// If set, [`Client`] will connect to /blob in the current component's namespace with
    /// [`fio::PERM_READABLE`].
    pub fn readable(self) -> Self {
        Self { readable: true, ..self }
    }

    /// If set, [`Client`] will connect to /blob in the current component's namespace with
    /// [`fio::PERM_WRITABLE`] which needed so that [`Client::delete_blob`] can unlink the file,
    /// and [`Client`] will connect to and use fuchsia.fxfs/BlobCreator for writes.
    pub fn writable(self) -> Self {
        Self { writable: true, ..self }
    }

    /// If set, [`Client`] will connect to /blob in the current component's namespace with
    /// [`fio::PERM_EXECUTABLE`].
    pub fn executable(self) -> Self {
        Self { executable: true, ..self }
    }

    /// If set, [`Client`] will connect to and use fuchsia.fxfs/BlobCreator for writes.
    /// This is independent of requiring writable directory access to `/blob`.
    pub fn creator(self) -> Self {
        Self { creator: true, ..self }
    }
}

impl Client {
    /// Create an empty `ClientBuilder`
    pub fn builder() -> ClientBuilder {
        Default::default()
    }
}
/// Blobfs client
#[derive(Debug, Clone)]
pub struct Client {
    dir: Option<fio::DirectoryProxy>,
    creator: Option<ffxfs::BlobCreatorProxy>,
    reader: ffxfs::BlobReaderProxy,
}

impl Client {
    /// Returns a client connected to the given blob directory, BlobCreatorProxy, and
    /// BlobReaderProxy. If `vmex` is passed in, sets the VmexResource, which is used to mark blobs
    /// as executable. If `creator` is not supplied, writes will fail.
    pub fn new(
        dir: fio::DirectoryProxy,
        creator: Option<ffxfs::BlobCreatorProxy>,
        reader: ffxfs::BlobReaderProxy,
        vmex: Option<zx::Resource>,
    ) -> Result<Self, anyhow::Error> {
        if let Some(vmex) = vmex {
            vmo_blob::init_vmex_resource(vmex)?;
        }
        Ok(Self { dir: Some(dir), creator, reader })
    }

    /// Creates a new client backed by the returned request stream. This constructor should not be
    /// used outside of tests.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub fn new_test() -> (
        Self,
        fio::DirectoryRequestStream,
        ffxfs::BlobReaderRequestStream,
        ffxfs::BlobCreatorRequestStream,
    ) {
        let (dir, dir_stream) = fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
        let (reader, reader_stream) =
            fidl::endpoints::create_proxy_and_stream::<ffxfs::BlobReaderMarker>();
        let (creator, creator_stream) =
            fidl::endpoints::create_proxy_and_stream::<ffxfs::BlobCreatorMarker>();

        (
            Self { dir: Some(dir), creator: Some(creator), reader },
            dir_stream,
            reader_stream,
            creator_stream,
        )
    }

    /// Creates a new client backed by the returned mock. This constructor should not be used
    /// outside of tests.
    ///
    /// # Panics
    ///
    /// Panics on error
    pub fn new_mock() -> (Self, mock::Mock) {
        let (dir, stream) = fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
        let (reader, reader_stream) =
            fidl::endpoints::create_proxy_and_stream::<ffxfs::BlobReaderMarker>();
        let (creator, creator_stream) =
            fidl::endpoints::create_proxy_and_stream::<ffxfs::BlobCreatorMarker>();

        (
            Self { dir: Some(dir), creator: Some(creator), reader },
            mock::Mock { stream, reader_stream, creator_stream },
        )
    }

    /// Returns the read-only VMO backing the blob.
    pub async fn get_blob_vmo(&self, hash: &Hash) -> Result<zx::Vmo, GetBlobVmoError> {
        self.reader
            .get_vmo(hash)
            .await
            .map_err(GetBlobVmoError::Fidl)?
            .map_err(|s| GetBlobVmoError::GetVmo(Status::err_from_raw(s)))
    }

    /// Open a blob for read using open3. `scope` will only be used if the client was configured to
    /// use fuchsia.fxfs.BlobReader.
    pub fn open_blob_for_read(
        &self,
        blob: &Hash,
        flags: fio::Flags,
        scope: ExecutionScope,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        if flags.rights().is_some_and(|rights| rights.contains(fio::Operations::WRITE_BYTES)) {
            return Err(zx::Status::ACCESS_DENIED);
        }
        if flags.creation_mode() != vfs::CreationMode::Never {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        // Errors below will be communicated via the `object_request` channel.
        let object_request = object_request.take();
        let () = open_blob_with_reader(self.reader.clone(), *blob, scope, flags, object_request);
        Ok(())
    }

    /// Returns the list of known blobs in blobfs.
    pub async fn list_known_blobs(&self) -> Result<HashSet<Hash>, BlobfsError> {
        // fuchsia.io.Directory.ReadDirents uses a per-connection index into the array of
        // directory entries. To prevent contention over this index by concurrent calls (either
        // from concurrent calls to list_known_blobs on this object, or on clones of this object,
        // or other clones of the DirectoryProxy this object was made from), create a new
        // connection which will have its own index.
        let dir = self.dir.as_ref().ok_or(BlobfsError::DirectoryNotConfigured)?;
        let private_connection = fuchsia_fs::directory::clone(dir)?;
        fuchsia_fs::directory::readdir(&private_connection)
            .await
            .map_err(BlobfsError::ReadDir)?
            .into_iter()
            .filter(|entry| entry.kind == fuchsia_fs::directory::DirentKind::File)
            .map(|entry| entry.name.parse().map_err(BlobfsError::ParseHash))
            .collect()
    }

    /// Delete the blob with the given merkle hash.
    pub async fn delete_blob(&self, blob: &Hash) -> Result<(), BlobfsError> {
        let dir = self.dir.as_ref().ok_or(BlobfsError::DirectoryNotConfigured)?;
        dir.unlink(&blob.to_string(), &fio::UnlinkOptions::default())
            .await?
            .map_err(|s| BlobfsError::Unlink(Status::err_from_raw(s)))
    }

    /// Open a new blob for write.
    pub async fn open_blob_for_write(
        &self,
        blob: &Hash,
        allow_existing: bool,
    ) -> Result<ClientEnd<ffxfs::BlobWriterMarker>, CreateError> {
        let Some(creator) = &self.creator else {
            return Err(CreateError::WritingNotConfigured);
        };
        Ok(creator.create(blob, allow_existing).await??)
    }

    /// Returns whether blobfs has a blob with the given hash and blobfs considers it up to date.
    pub async fn blob_present_and_up_to_date(&self, blob: &Hash) -> bool {
        // This call is only used when we're considering writing a blob, so we should have a
        // creator.
        matches!(
            self.creator.as_ref().expect("Missing BlobCreator access").needs_overwrite(blob).await,
            Ok(Ok(false))
        )
    }

    /// Looks up the current status of a blob using `BlobCreator.NeedsOverwrite`.
    pub async fn blob_status(&self, blob: &Hash) -> Result<BlobStatus, BlobfsError> {
        let Some(creator) = &self.creator else {
            return Err(BlobfsError::BlobStatus(BlobStatusError::WritingNotConfigured));
        };
        match creator.needs_overwrite(blob).await? {
            Ok(true) => Ok(BlobStatus::NeedsOverwrite),
            Ok(false) => Ok(BlobStatus::UpToDate),
            Err(status) if status == Status::NOT_FOUND.into_raw() => Ok(BlobStatus::Absent),
            Err(s) => Err(BlobfsError::BlobStatus(BlobStatusError::NeedsOverwrite(
                Status::err_from_raw(s),
            ))),
        }
    }

    /// Determines which blobs of `candidates` are missing from blobfs.
    /// TODO(https://fxbug.dev/338477132) This fn is used during resolves after a meta.far is
    /// fetched to determine which content blobs and subpackage meta.fars need to be fetched.
    /// On c++blobfs, opening a partially written blob keeps that blob alive, creating the
    /// following race condition:
    /// 1. blob is partially written by resolve A
    /// 2. blob is opened by this fn to check for presence by concurrent resolve B
    /// 3. resolve A encounters an error and retries the fetch, which attempts to open the blob for
    ///    write, which collides with the partially written blob from (1) that is being kept alive
    ///    by (2) and so fails
    pub async fn filter_to_missing_blobs(
        &self,
        candidates: impl IntoIterator<Item = Hash>,
    ) -> HashSet<Hash> {
        // Attempt to open each blob instead of using ReadDirents to catch more forms of filesystem
        // metadata corruption.
        // We don't use ReadDirents even as a pre-filter because emulator testing suggests
        // ReadDirents on an fxblob with 1,000 blobs takes as long as ~60 sequential has_blob calls
        // on missing blobs, and it's about 5x worse on c++blobfs (on which both ReadDirents is
        // slower and has_blob is faster). The minor speedup on packages with a great number of
        // missing blobs is not worth a rarely taken branch deep within package resolution.
        stream::iter(candidates)
            .map(move |blob| async move {
                if self.blob_present_and_up_to_date(&blob).await { None } else { Some(blob) }
            })
            // Emulator testing suggests both c++blobfs and fxblob show diminishing returns after
            // even three concurrent `has_blob` calls.
            .buffer_unordered(10)
            .filter_map(|blob| async move { blob })
            .collect()
            .await
    }

    /// Call fuchsia.io/Node.Sync on the blobfs directory.
    pub async fn sync(&self) -> Result<(), BlobfsError> {
        let dir = self.dir.as_ref().ok_or(BlobfsError::DirectoryNotConfigured)?;
        dir.sync().await?.map_err(zx::Status::err_from_raw).map_err(BlobfsError::Sync)
    }
}

/// Spawns a task on `scope` to attempt opening `blob` via `reader`. Creates a file connection to
/// the blob using [`vmo_blob::VmoBlob`]. Errors will be sent via `object_request` asynchronously.
fn open_blob_with_reader<P: ProtocolsExt + Send>(
    reader: ffxfs::BlobReaderProxy,
    blob_hash: Hash,
    scope: ExecutionScope,
    protocols: P,
    object_request: ObjectRequest,
) {
    scope.clone().spawn(object_request.handle_async(async move |object_request| {
        let get_vmo_result = reader.get_vmo(&blob_hash.into()).await.map_err(|fidl_error| {
            if let fidl::Error::ClientChannelClosed { epitaph, .. } = fidl_error {
                error!("Blob reader channel closed: {epitaph:?}");
                match epitaph.into() {
                    Err(status) => status,
                    Ok(()) => zx::Status::PEER_CLOSED,
                }
            } else {
                error!("Transport error on get_vmo: {:?}", fidl_error);
                zx::Status::INTERNAL
            }
        })?;
        let vmo = get_vmo_result.map_err(zx::Status::err_from_raw)?;
        let vmo_blob = vmo_blob::VmoBlob::new(vmo, blob_hash.into());
        object_request
            .create_connection::<StreamIoConnection<_>, _>(scope, vmo_blob, protocols)
            .await
    }));
}

#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum GetBlobVmoError {
    #[error("getting the vmo")]
    GetVmo(#[source] Status),

    #[error("opening the blob")]
    OpenBlob(#[source] fuchsia_fs::node::OpenError),

    #[error("making a fidl request")]
    Fidl(#[source] fidl::Error),
}

#[cfg(test)]
impl Client {
    /// Constructs a new [`Client`] connected to the provided [`BlobfsRamdisk`]. Tests in this
    /// crate should use this constructor rather than [`BlobfsRamdisk::client`], which returns
    /// the non-cfg(test) build of this crate's [`blobfs::Client`]. While tests could use the
    /// [`blobfs::Client`] returned by [`BlobfsRamdisk::client`], it will be a different type than
    /// [`super::Client`], and the tests could not access its private members or any cfg(test)
    /// specific functionality.
    ///
    /// # Panics
    ///
    /// Panics on error.
    pub fn for_ramdisk(blobfs: &blobfs_ramdisk::BlobfsRamdisk) -> Self {
        Self::new(
            blobfs.root_dir_proxy().unwrap(),
            Some(blobfs.blob_creator_proxy().unwrap()),
            blobfs.blob_reader_proxy().unwrap(),
            None,
        )
        .unwrap()
    }
}

#[cfg(test)]
#[allow(clippy::bool_assert_comparison)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use blobfs_ramdisk::BlobfsRamdisk;
    use fuchsia_async as fasync;
    use futures::stream::TryStreamExt as _;
    use std::sync::Arc;
    use test_case::test_case;

    #[test_case(blobfs_ramdisk::Implementation::CppBlobfs; "cpp_blobfs")]
    #[test_case(blobfs_ramdisk::Implementation::Fxblob; "fxblob")]
    #[fuchsia::test]
    async fn list_known_blobs_empty(blob_impl: blobfs_ramdisk::Implementation) {
        let blobfs = BlobfsRamdisk::builder().implementation(blob_impl).start().await.unwrap();
        let client = Client::for_ramdisk(&blobfs);

        assert_eq!(client.list_known_blobs().await.unwrap(), HashSet::new());
        blobfs.stop().await.unwrap();
    }

    #[test_case(blobfs_ramdisk::Implementation::CppBlobfs; "cpp_blobfs")]
    #[test_case(blobfs_ramdisk::Implementation::Fxblob; "fxblob")]
    #[fuchsia::test]
    async fn list_known_blobs(blob_impl: blobfs_ramdisk::Implementation) {
        let blobfs = BlobfsRamdisk::builder()
            .implementation(blob_impl)
            .with_blob(&b"blob 1"[..])
            .with_blob(&b"blob 2"[..])
            .start()
            .await
            .unwrap();
        let client = Client::for_ramdisk(&blobfs);

        let expected = blobfs.list_blobs().unwrap().into_iter().collect();
        assert_eq!(client.list_known_blobs().await.unwrap(), expected);
        blobfs.stop().await.unwrap();
    }

    #[test_case(blobfs_ramdisk::Implementation::CppBlobfs; "cpp_blobfs")]
    #[test_case(blobfs_ramdisk::Implementation::Fxblob; "fxblob")]
    #[fuchsia::test]
    async fn delete_blob_and_then_list(blob_impl: blobfs_ramdisk::Implementation) {
        let blobfs = BlobfsRamdisk::builder()
            .implementation(blob_impl)
            .with_blob(&b"blob 1"[..])
            .with_blob(&b"blob 2"[..])
            .start()
            .await
            .unwrap();
        let client = Client::for_ramdisk(&blobfs);

        let merkle = fuchsia_merkle::root_from_slice(b"blob 1");
        assert_matches!(client.delete_blob(&merkle).await, Ok(()));

        let expected = HashSet::from([fuchsia_merkle::root_from_slice(b"blob 2")]);
        assert_eq!(client.list_known_blobs().await.unwrap(), expected);
        blobfs.stop().await.unwrap();
    }

    #[test_case(blobfs_ramdisk::Implementation::CppBlobfs; "cpp_blobfs")]
    #[test_case(blobfs_ramdisk::Implementation::Fxblob; "fxblob")]
    #[fuchsia::test]
    async fn delete_nonexistent_blob(blob_impl: blobfs_ramdisk::Implementation) {
        let blobfs = BlobfsRamdisk::builder().implementation(blob_impl).start().await.unwrap();
        let client = Client::for_ramdisk(&blobfs);
        let blob_merkle = Hash::from([1; 32]);

        assert_matches!(
            client.delete_blob(&blob_merkle).await,
            Err(BlobfsError::Unlink(Status::NOT_FOUND))
        );
        blobfs.stop().await.unwrap();
    }

    #[fuchsia::test]
    async fn delete_blob_mock() {
        let (client, mut stream, _, _) = Client::new_test();
        let blob_merkle = Hash::from([1; 32]);
        fasync::Task::spawn(async move {
            match stream.try_next().await.unwrap().unwrap() {
                fio::DirectoryRequest::Unlink { name, responder, .. } => {
                    assert_eq!(name, blob_merkle.to_string());
                    responder.send(Ok(())).unwrap();
                }
                other => panic!("unexpected request: {other:?}"),
            }
        })
        .detach();

        assert_matches!(client.delete_blob(&blob_merkle).await, Ok(()));
    }

    #[test_case(blobfs_ramdisk::Implementation::CppBlobfs; "cpp_blobfs")]
    #[test_case(blobfs_ramdisk::Implementation::Fxblob; "fxblob")]
    #[fuchsia::test]
    async fn has_blob(blob_impl: blobfs_ramdisk::Implementation) {
        let blobfs = BlobfsRamdisk::builder()
            .implementation(blob_impl)
            .with_blob(&b"blob 1"[..])
            .start()
            .await
            .unwrap();
        let client = Client::for_ramdisk(&blobfs);

        assert!(
            client.blob_present_and_up_to_date(&fuchsia_merkle::root_from_slice(b"blob 1")).await
        );
        assert!(!client.blob_present_and_up_to_date(&Hash::from([1; 32])).await);

        blobfs.stop().await.unwrap();
    }

    #[test_case(blobfs_ramdisk::Implementation::CppBlobfs; "cpp_blobfs")]
    #[test_case(blobfs_ramdisk::Implementation::Fxblob; "fxblob")]
    #[fuchsia::test]
    async fn has_blob_return_false_if_blob_is_partially_written(
        blob_impl: blobfs_ramdisk::Implementation,
    ) {
        let blobfs = BlobfsRamdisk::builder().implementation(blob_impl).start().await.unwrap();
        let client = Client::for_ramdisk(&blobfs);

        let content = &[3; 1024];
        let hash = fuchsia_merkle::root_from_slice(content);
        let delivery_content =
            delivery_blob::Type1Blob::generate(content, delivery_blob::CompressionMode::Always);

        let writer = client.open_blob_for_write(&hash, false).await.unwrap().into_proxy();
        assert!(!client.blob_present_and_up_to_date(&hash).await);

        let n = delivery_content.len();
        let vmo = writer.get_vmo(n.try_into().unwrap()).await.unwrap().unwrap();
        assert!(!client.blob_present_and_up_to_date(&hash).await);

        let () = vmo.write(&delivery_content[0..n - 1], 0).unwrap();
        let () = writer.bytes_ready((n - 1).try_into().unwrap()).await.unwrap().unwrap();
        assert!(!client.blob_present_and_up_to_date(&hash).await);

        let () = vmo.write(&delivery_content[n - 1..], (n - 1).try_into().unwrap()).unwrap();
        let () = writer.bytes_ready(1.try_into().unwrap()).await.unwrap().unwrap();
        assert!(client.blob_present_and_up_to_date(&hash).await);

        blobfs.stop().await.unwrap();
    }

    async fn fully_write_blob(client: &Client, content: &[u8]) -> Hash {
        let hash = fuchsia_merkle::root_from_slice(content);
        let delivery_content =
            delivery_blob::Type1Blob::generate(content, delivery_blob::CompressionMode::Always);
        let writer = client.open_blob_for_write(&hash, false).await.unwrap().into_proxy();
        let vmo = writer
            .get_vmo(delivery_content.len().try_into().unwrap())
            .await
            .expect("a")
            .map_err(zx::Status::err_from_raw)
            .expect("b");
        let () = vmo.write(&delivery_content, 0).unwrap();
        let () =
            writer.bytes_ready(delivery_content.len().try_into().unwrap()).await.unwrap().unwrap();
        hash
    }

    #[test_case(blobfs_ramdisk::Implementation::CppBlobfs; "cpp_blobfs")]
    #[test_case(blobfs_ramdisk::Implementation::Fxblob; "fxblob")]
    #[fuchsia::test]
    async fn filter_to_missing_blobs(blob_impl: blobfs_ramdisk::Implementation) {
        let blobfs = BlobfsRamdisk::builder().implementation(blob_impl).start().await.unwrap();
        let client = Client::for_ramdisk(&blobfs);

        let missing_hash0 = Hash::from([0; 32]);
        let missing_hash1 = Hash::from([1; 32]);

        let present_blob0 = fully_write_blob(&client, &[2; 1024]).await;
        let present_blob1 = fully_write_blob(&client, &[3; 1024]).await;

        assert_eq!(
            client
                .filter_to_missing_blobs([
                    missing_hash0,
                    missing_hash1,
                    present_blob0,
                    present_blob1
                ])
                .await,
            HashSet::from([missing_hash0, missing_hash1])
        );

        blobfs.stop().await.unwrap();
    }

    #[fuchsia::test]
    async fn sync() {
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);
        let (client, mut stream, _, _) = Client::new_test();
        fasync::Task::spawn(async move {
            match stream.try_next().await.unwrap().unwrap() {
                fio::DirectoryRequest::Sync { responder } => {
                    counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    responder.send(Ok(())).unwrap();
                }
                other => panic!("unexpected request: {other:?}"),
            }
        })
        .detach();

        assert_matches!(client.sync().await, Ok(()));
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[fuchsia::test]
    async fn open_blob_for_write_maps_already_exists() {
        let (blob_creator, mut blob_creator_stream) =
            fidl::endpoints::create_proxy_and_stream::<ffxfs::BlobCreatorMarker>();
        let (blob_reader, _) = fidl::endpoints::create_proxy::<ffxfs::BlobReaderMarker>();

        let client = Client::new(
            fidl::endpoints::create_proxy::<fio::DirectoryMarker>().0,
            Some(blob_creator),
            blob_reader,
            None,
        )
        .unwrap();

        fuchsia_async::Task::spawn(async move {
            match blob_creator_stream.next().await.unwrap().unwrap() {
                ffxfs::BlobCreatorRequest::Create { hash, allow_existing, responder } => {
                    assert_eq!(hash, [0; 32]);
                    assert!(!allow_existing);
                    let () = responder.send(Err(ffxfs::CreateBlobError::AlreadyExists)).unwrap();
                }
                ffxfs::BlobCreatorRequest::NeedsOverwrite { .. } => {
                    unreachable!("This code path is not yet exercised.");
                }
            }
        })
        .detach();

        assert_matches!(
            client.open_blob_for_write(&[0; 32].into(), false).await,
            Err(CreateError::AlreadyExists)
        );
    }

    #[fuchsia::test]
    async fn concurrent_list_known_blobs_all_return_full_contents() {
        use futures::StreamExt;
        let blobfs = BlobfsRamdisk::builder().start().await.unwrap();
        let client = Client::for_ramdisk(&blobfs);

        // ReadDirents returns an 8,192 byte buffer, and each entry is 74 bytes [0] (including 64
        // bytes of filename), so use more than 110 entries to guarantee that listing all contents
        // requires multiple ReadDirents calls. This isn't necessary to cause conflict, because
        // each successful listing requires a call to Rewind as well, but it does make conflict
        // more likely.
        // [0] https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.io/directory.fidl;l=261;drc=9e84e19d3f42240c46d2b0c3c132c2f0b5a3343f
        for i in 0..256u16 {
            let _: Hash = fully_write_blob(&client, i.to_le_bytes().as_slice()).await;
        }

        let () = futures::stream::iter(0..100)
            .for_each_concurrent(None, |_| async {
                assert_eq!(client.list_known_blobs().await.unwrap().len(), 256);
            })
            .await;
    }
}
