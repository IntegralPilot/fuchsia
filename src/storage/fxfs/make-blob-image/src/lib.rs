// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use anyhow::{Context, Error, anyhow};
use delivery_blob::Type1Blob;
pub use delivery_blob::compression::CompressionAlgorithm;
use delivery_blob::compression::{ChunkedArchive, ChunkedArchiveOptions};
use fuchsia_async as fasync;
use fuchsia_merkle::{Hash, MerkleRootBuilder};
use futures::{SinkExt as _, StreamExt as _, TryStreamExt as _, try_join};
use fxfs::blob_metadata::{
    BlobFormat, BlobMetadata, BlobMetadataLeafHashCollector, FxfsBlobMetadataExt,
};
use fxfs::errors::FxfsError;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem};
use fxfs::object_handle::{ObjectHandle, WriteBytes};
use fxfs::object_store::directory::Directory;
use fxfs::object_store::journal::RESERVED_SPACE;
use fxfs::object_store::journal::super_block::SuperBlockInstance;
use fxfs::object_store::transaction::{LockKey, lock_keys};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{
    DataObjectHandle, DirectWriter, HandleOptions, NewChildStoreOptions, ObjectStore, StoreOptions,
};
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sparse::unsparse;
use std::fs;
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use storage_device::DeviceHolder;
use storage_device::file_backed_device::FileBackedDevice;
use storage_units::BlockSize;

pub const BLOB_VOLUME_NAME: &str = "blob";

const BLOCK_SIZE: BlockSize = BlockSize::SIZE_4KIB;

const READ_BUFFER_BLOCKS: u64 = 512;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct BlobsJsonOutputEntry {
    source_path: String,
    merkle: String,
    bytes: usize,
    size: u64,
    file_size: usize,
    compressed_file_size: u64,
    merkle_tree_size: usize,
    // For consistency with the legacy blobfs tooling, we still use the name `blobfs`.
    used_space_in_blobfs: u64,
}

type BlobsJsonOutput = Vec<BlobsJsonOutputEntry>;

/// Generates an Fxfs image containing a blob volume with the blobs specified in `manifest_path`.
/// Creates the block image at `output_image_path` and writes a blobs.json file to
/// `json_output_path`.
/// If `target_size` bytes is set, the raw image will be set to exactly this size (and an error is
/// returned if the contents exceed that size).  If unset (or 0), the image will be truncated to
/// twice the size of its contents, which is a heuristic that gives us roughly enough space for
/// normal usage of the image.
/// If `sparse_output_image_path` is set, an image will also be emitted in the Android sparse
/// format, which is suitable for flashing via fastboot.  The sparse image's logical size and
/// contents are identical to the raw image, but its actual size will likely be smaller.
pub async fn make_blob_image(
    output_image_path: &str,
    sparse_output_image_path: Option<&str>,
    blobs: Vec<(Hash, PathBuf)>,
    json_output_path: &str,
    target_size: Option<u64>,
    compression_algorithm: Option<CompressionAlgorithm>,
) -> Result<(), Error> {
    let output_image = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(output_image_path)?;

    let mut target_size = target_size.unwrap_or_default();

    if target_size > 0 && target_size < BLOCK_SIZE {
        return Err(anyhow!("Size {} is too small", target_size));
    }
    if !BLOCK_SIZE.is_aligned(target_size) {
        return Err(anyhow!("Invalid size {} is not block-aligned", target_size));
    }
    let block_count = if target_size != 0 {
        // Truncate the image to the target size now.
        output_image.set_len(target_size).context("Failed to resize image")?;
        target_size / BLOCK_SIZE
    } else {
        // Arbitrarily use 4GiB for the initial block device size, but don't truncate the file yet,
        // so it becomes exactly as large as needed to contain the contents.  We'll truncate it down
        // to 2x contents later.
        // 4G just needs to be large enough to fit pretty much any image.
        const FOUR_GIGS: u64 = 4 * 1024 * 1024 * 1024;
        FOUR_GIGS / BLOCK_SIZE
    };

    let device = DeviceHolder::new(FileBackedDevice::new_with_block_count(
        output_image,
        BLOCK_SIZE.get() as u32,
        block_count,
    ));
    let fxblob = FxBlobBuilder::new(device).await?;
    let blobs_json = install_blobs(&fxblob, blobs, compression_algorithm).await.map_err(|e| {
        if target_size != 0 && FxfsError::NoSpace.matches(&e) {
            e.context(format!(
                "Configured image size {} is too small to fit the base system image.",
                target_size
            ))
        } else {
            e
        }
    })?;
    let actual_size = fxblob.finalize().await?.1;

    if target_size == 0 {
        // Apply a default heuristic of 2x the actual image size.  This is necessary to use the
        // Fxfs image, since if it's completely full it can't be modified.
        target_size = (actual_size + RESERVED_SPACE) * 2;
    }

    if let Some(sparse_path) = sparse_output_image_path {
        create_sparse_image(sparse_path, output_image_path, actual_size, target_size, BLOCK_SIZE)
            .context("Failed to create sparse image")?;
    }

    if target_size != actual_size {
        debug_assert!(target_size > actual_size);
        let output_image =
            std::fs::OpenOptions::new().read(true).write(true).open(output_image_path)?;
        output_image.set_len(target_size).context("Failed to resize image")?;
    }

    let mut json_output = BufWriter::new(
        std::fs::File::create(json_output_path).context("Failed to create JSON output file")?,
    );
    serde_json::to_writer_pretty(&mut json_output, &blobs_json)
        .context("Failed to serialize to JSON output")?;

    Ok(())
}

fn create_sparse_image(
    sparse_output_image_path: &str,
    image_path: &str,
    actual_size: u64,
    target_size: u64,
    block_size: BlockSize,
) -> Result<(), Error> {
    let image = std::fs::OpenOptions::new()
        .read(true)
        .open(image_path)
        .with_context(|| format!("Failed to open {:?}", image_path))?;
    let mut output = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(sparse_output_image_path)
        .with_context(|| format!("Failed to create {:?}", sparse_output_image_path))?;
    sparse::builder::SparseImageBuilder::new()
        .set_block_size(block_size.get() as u32)
        .add_source(sparse::builder::DataSource::Reader {
            reader: Box::new(image),
            size: actual_size,
        })
        .add_source(sparse::builder::DataSource::Skip(target_size - actual_size))
        .build(&mut output)
        .map_err(anyhow::Error::from)
}

/// Builder used to construct a new Fxblob instance ready for flashing to a device.
pub struct FxBlobBuilder {
    blob_directory: Directory<ObjectStore>,
    filesystem: OpenFxFilesystem,
}

impl FxBlobBuilder {
    /// Creates a new [`FxBlobBuilder`] backed by the given `device`.
    pub async fn new(device: DeviceHolder) -> Result<Self, Error> {
        let filesystem = FxFilesystemBuilder::new()
            .format(true)
            .trim_config(None)
            .image_builder_mode(Some(SuperBlockInstance::A))
            .open(device)
            .await
            .context("Failed to format filesystem")?;
        filesystem.enable_allocations();
        let root_volume = root_volume(filesystem.clone()).await?;
        let vol = root_volume
            .new_volume(BLOB_VOLUME_NAME, NewChildStoreOptions::default())
            .await
            .context("Failed to create volume")?;
        let blob_directory = Directory::open(&vol, vol.root_directory_object_id())
            .await
            .context("Unable to open root blob directory")?;
        Ok(Self { blob_directory, filesystem })
    }

    /// Finalizes building the FxBlob instance this builder represents. The filesystem will not be
    /// usable unless this is called. Returns the filesystem's DeviceHolder and the last offset in
    /// bytes which was used on the device.
    pub async fn finalize(self) -> Result<(DeviceHolder, u64), Error> {
        self.filesystem.close().await?;
        let actual_size = self.filesystem.allocator().maximum_offset();
        Ok((self.filesystem.take_device().await, actual_size))
    }

    /// Installs the given `blob` into the filesystem, returning a handle to the new object.
    pub async fn install_blob(
        &self,
        blob: &BlobToInstall,
    ) -> Result<DataObjectHandle<ObjectStore>, Error> {
        let handle;
        let keys = lock_keys![LockKey::object(
            self.blob_directory.store().store_object_id(),
            self.blob_directory.object_id(),
        )];
        let mut transaction = self
            .blob_directory
            .store()
            .new_transaction(keys, Default::default())
            .await
            .context("new transaction")?;
        handle = self
            .blob_directory
            .create_child_file_with_options(
                &mut transaction,
                &blob.hash.to_string(),
                // Checksums are redundant for blobs, which are already content-verified.
                HandleOptions { skip_checksums: true, ..Default::default() },
            )
            .await
            .context("create child file")?;
        transaction.commit().await.context("transaction commit")?;

        // Write the blob data directly into the object handle.
        {
            let mut writer = DirectWriter::new(&handle, Default::default()).await;
            match &blob.data {
                BlobData::Uncompressed(data) => {
                    writer.write_bytes(data).await.context("write blob contents")?;
                }
                BlobData::CompressedZstd(archive) | BlobData::CompressedLz4(archive) => {
                    for chunk in archive.chunks() {
                        writer
                            .write_bytes(&chunk.compressed_data)
                            .await
                            .context("write blob contents")?;
                    }
                }
            }
            writer.complete().await.context("flush blob contents")?;
        }

        // Write the metadata to the object handle.
        blob.metadata.write_to(&handle).await.context("write blob metadata")?;

        Ok(handle)
    }

    /// Helper function to quickly create a blob to install from in-memory data. Mainly for testing.
    pub fn generate_blob(
        &self,
        data: Vec<u8>,
        compression_algorithm: Option<CompressionAlgorithm>,
    ) -> Result<BlobToInstall, Error> {
        BlobToInstall::new(data, self.filesystem.block_size(), compression_algorithm)
    }
}

enum BlobData {
    Uncompressed(Vec<u8>),
    CompressedZstd(ChunkedArchive),
    CompressedLz4(ChunkedArchive),
}

fn compressed_offsets(chunked_archive: &ChunkedArchive) -> Vec<u64> {
    let mut offsets = Vec::with_capacity(chunked_archive.chunks().len());
    let mut offset: u64 = 0;
    for chunk in chunked_archive.chunks() {
        offsets.push(offset);
        offset += chunk.compressed_data.len() as u64;
    }
    offsets
}

/// Represents a blob ready to be installed into an FxBlob instance.
pub struct BlobToInstall {
    /// The validated Merkle root of this blob.
    hash: Hash,
    /// On-disk representation of the blob data (either compressed or uncompressed).
    data: BlobData,
    /// Uncompressed size of the blob's data.
    uncompressed_size: usize,
    /// Holds the merkle leaves and compressed offsets.
    metadata: BlobMetadata,
    /// Path, if any, corresponding to the on-disk location of the source for this blob. Only set
    /// if created via [`Self::new_from_file`].
    source: Option<PathBuf>,
}

impl BlobToInstall {
    /// Create a new blob ready for installation with [`FxBlobBuilder::install_blob`].
    pub fn new(
        data: Vec<u8>,
        fs_block_size: BlockSize,
        compression_algorithm: Option<CompressionAlgorithm>,
    ) -> Result<Self, Error> {
        let (hash, hashes) =
            MerkleRootBuilder::new(BlobMetadataLeafHashCollector::new()).complete(&data);

        let uncompressed_size = data.len();
        let data = if let Some(compression_algorithm) = compression_algorithm {
            maybe_compress(data, fs_block_size, compression_algorithm)
        } else {
            BlobData::Uncompressed(data)
        };
        let metadata = match &data {
            BlobData::Uncompressed(_) => {
                BlobMetadata { merkle_leaves: hashes, format: BlobFormat::Uncompressed }
            }
            BlobData::CompressedZstd(chunked_archive) => BlobMetadata {
                merkle_leaves: hashes,
                format: BlobFormat::ChunkedZstd {
                    uncompressed_size: uncompressed_size as u64,
                    chunk_size: chunked_archive.chunk_size() as u64,
                    compressed_offsets: compressed_offsets(&chunked_archive),
                },
            },
            BlobData::CompressedLz4(chunked_archive) => BlobMetadata {
                merkle_leaves: hashes,
                format: BlobFormat::ChunkedLz4 {
                    uncompressed_size: uncompressed_size as u64,
                    chunk_size: chunked_archive.chunk_size() as u64,
                    compressed_offsets: compressed_offsets(&chunked_archive),
                },
            },
        };
        Ok(BlobToInstall { hash, data, uncompressed_size, metadata, source: None })
    }

    /// Create a new blob ready for installation with [`FxBlobBuilder::install_blob`] from an
    /// existing file on disk.
    pub fn new_from_file(
        path: PathBuf,
        fs_block_size: BlockSize,
        compression_algorithm: Option<CompressionAlgorithm>,
    ) -> Result<Self, Error> {
        let mut data = Vec::new();
        std::fs::File::open(&path)
            .with_context(|| format!("Unable to open `{:?}'", path))?
            .read_to_end(&mut data)
            .with_context(|| format!("Unable to read contents of `{:?}'", path))?;
        let blob = Self::new(data, fs_block_size, compression_algorithm)?;
        Ok(Self { source: Some(path), ..blob })
    }

    pub fn hash(&self) -> Hash {
        self.hash.clone()
    }
}

async fn install_blobs(
    fxblob: &FxBlobBuilder,
    blobs: Vec<(Hash, PathBuf)>,
    compression_algorithm: Option<CompressionAlgorithm>,
) -> Result<BlobsJsonOutput, Error> {
    let num_blobs = blobs.len();
    let fs_block_size = fxblob.filesystem.block_size();
    // We don't need any backpressure as the channel guarantees at least one slot per sender.
    let (tx, rx) = futures::channel::mpsc::channel::<BlobToInstall>(0);
    // Generate each blob in parallel using a thread pool.
    let num_threads: usize = std::thread::available_parallelism().unwrap().into();
    let thread_pool = ThreadPoolBuilder::new().num_threads(num_threads).build().unwrap();
    let generate = fasync::unblock(move || {
        thread_pool.install(|| {
            blobs.par_iter().try_for_each(|(hash, path)| {
                let blob = BlobToInstall::new_from_file(
                    path.clone(),
                    fs_block_size,
                    compression_algorithm,
                )?;
                if &blob.hash != hash {
                    let calculated_hash = &blob.hash;
                    let path = path.display();
                    return Err(anyhow!(
                        "Hash mismatch for {path}: calculated={calculated_hash}, expected={hash}"
                    ));
                }
                futures::executor::block_on(tx.clone().send(blob))
                    .context("send blob to install task")
            })
        })?;
        Ok(())
    });
    // We can buffer up to this many blobs after processing.
    const MAX_INSTALL_CONCURRENCY: usize = 10;
    let install = rx
        .map(|blob| install_blob_with_json_output(fxblob, blob))
        .buffer_unordered(MAX_INSTALL_CONCURRENCY)
        .try_collect::<BlobsJsonOutput>();
    let (installed_blobs, _) = try_join!(install, generate)?;
    assert_eq!(installed_blobs.len(), num_blobs);
    Ok(installed_blobs)
}

async fn install_blob_with_json_output(
    fxblob: &FxBlobBuilder,
    blob: BlobToInstall,
) -> Result<BlobsJsonOutputEntry, Error> {
    let handle = fxblob.install_blob(&blob).await?;
    let properties = handle.get_properties().await.context("get properties")?;
    let source_path = blob
        .source
        .expect("missing source path")
        .to_str()
        .context("blob path to utf8")?
        .to_string();
    Ok(BlobsJsonOutputEntry {
        source_path,
        merkle: blob.hash.to_string(),
        bytes: blob.uncompressed_size,
        size: properties.allocated_size,
        file_size: blob.uncompressed_size,
        compressed_file_size: properties.data_attribute_size,
        merkle_tree_size: blob.metadata.serialized_size().context("blob metadata size")?,
        used_space_in_blobfs: properties.allocated_size,
    })
}

fn maybe_compress(
    buf: Vec<u8>,
    filesystem_block_size: BlockSize,
    compression_algorithm: CompressionAlgorithm,
) -> BlobData {
    if buf.len() as u64 <= filesystem_block_size {
        return BlobData::Uncompressed(buf); // No savings, return original data.
    }
    let chunked_archive_options = match compression_algorithm {
        CompressionAlgorithm::Zstd => {
            // TODO(https://fxbug.dev/450626615) Use chunked-compression V3.
            Type1Blob::CHUNKED_ARCHIVE_OPTIONS
        }
        CompressionAlgorithm::Lz4 => ChunkedArchiveOptions::V3 { compression_algorithm },
    };
    let archive =
        ChunkedArchive::new(&buf, chunked_archive_options).expect("failed to compress data");
    if filesystem_block_size.align_up(archive.compressed_data_size() as u64).unwrap()
        >= buf.len() as u64
    {
        BlobData::Uncompressed(buf) // Compression expanded the file, return original data.
    } else {
        match compression_algorithm {
            CompressionAlgorithm::Zstd => BlobData::CompressedZstd(archive),
            CompressionAlgorithm::Lz4 => BlobData::CompressedLz4(archive),
        }
    }
}

/// Extract blobs from the Fxfs image in the product bundle to the output directory.
pub async fn extract_blobs(image: PathBuf, out_dir: PathBuf) -> anyhow::Result<()> {
    if out_dir.exists() {
        fs::remove_dir_all(&out_dir).context("Failed to remove output directory")?;
    }
    fs::create_dir_all(&out_dir)?;

    // TODO (https://fxbug.dev/483735826):
    // Update the fxfs crate so that you can hand it a sparse image and
    // it will be able to parse that and iterate over the contents
    let mut source = fs::File::open(&image)?;
    let mut non_sparse_image = tempfile::NamedTempFile::new_in(&out_dir)?;
    unsparse(&mut source, non_sparse_image.as_file_mut()).map_err(anyhow::Error::from)?;

    let device = DeviceHolder::new(FileBackedDevice::new(
        non_sparse_image.reopen()?,
        BLOCK_SIZE.get() as u32,
    ));
    let fs = FxFilesystemBuilder::new().read_only(true).open(device).await?;
    let vol =
        root_volume(fs.clone()).await?.volume(BLOB_VOLUME_NAME, StoreOptions::default()).await?;
    let root_dir = Directory::open(&vol, vol.root_directory_object_id()).await?;
    let layer_set = root_dir.store().tree().layer_set();
    let mut merger = layer_set.merger();
    let mut iter = root_dir.iter(&mut merger).await?;
    let blob_extraction_futures = futures::stream::FuturesUnordered::new();

    while let Some((name, object_id, descriptor)) = iter.get() {
        if *descriptor == fxfs::object_store::ObjectDescriptor::File {
            let handle = fxfs::object_store::ObjectStore::open_object(
                root_dir.owner(),
                object_id,
                fxfs::object_store::HandleOptions::default(),
                None,
            )
            .await?;

            let mut components = std::path::Path::new(name).components();
            if !matches!(components.next(), Some(std::path::Component::Normal(..))) {
                return Err(anyhow!("Invalid blob name: {}", name));
            }
            if components.next().is_some() {
                return Err(anyhow!("Invalid blob name: {}", name));
            }
            let out_path = out_dir.join(name);
            let mut file = std::fs::File::create(&out_path)?;
            let mut read_buf = Vec::new();
            let mut offset = 0;
            let mut buf =
                handle.allocate_buffer((handle.block_size() * READ_BUFFER_BLOCKS) as usize).await;
            loop {
                let bytes = handle.read(offset, buf.as_mut()).await?;
                if bytes == 0 {
                    break;
                }
                offset += bytes as u64;
                buf.subslice(..bytes).append_to(&mut read_buf);
            }

            let metadata = BlobMetadata::read_from(&handle).await?;
            blob_extraction_futures.push(fasync::unblock(move || -> Result<(), Error> {
                match metadata.format {
                    BlobFormat::ChunkedZstd {
                        uncompressed_size,
                        compressed_offsets,
                        chunk_size,
                    } => decompress_blob(
                        &read_buf,
                        uncompressed_size,
                        compressed_offsets,
                        chunk_size,
                        CompressionAlgorithm::Zstd,
                        &mut file,
                    ),
                    BlobFormat::ChunkedLz4 {
                        uncompressed_size,
                        compressed_offsets,
                        chunk_size,
                    } => decompress_blob(
                        &read_buf,
                        uncompressed_size,
                        compressed_offsets,
                        chunk_size,
                        CompressionAlgorithm::Lz4,
                        &mut file,
                    ),
                    BlobFormat::Uncompressed => {
                        file.write_all(&read_buf)?;
                        Ok(())
                    }
                }
            }));
        }
        iter.advance().await?;
    }
    blob_extraction_futures.try_collect::<()>().await?;
    Ok(())
}

fn decompress_blob(
    blob_data: &[u8],
    uncompressed_size: u64,
    compressed_offsets: Vec<u64>,
    chunk_size: u64,
    compression_algorithm: CompressionAlgorithm,
    out: &mut std::fs::File,
) -> Result<(), Error> {
    let mut decompressor = compression_algorithm.decompressor();
    let mut buf = vec![0; chunk_size as usize];
    let mut total_decompressed_size = 0;
    for i in 0..compressed_offsets.len() {
        let start_offset = compressed_offsets[i] as usize;
        let end_offset = if i + 1 == compressed_offsets.len() {
            blob_data.len()
        } else {
            compressed_offsets[i + 1] as usize
        };
        let decompressed_size =
            decompressor.decompress_into(&blob_data[start_offset..end_offset], &mut buf, i)?;
        total_decompressed_size += decompressed_size;
        out.write_all(&buf[..decompressed_size])?;
    }
    if total_decompressed_size != uncompressed_size as usize {
        Err(anyhow!(
            "Decompressed size does not match expected size {} {}",
            total_decompressed_size,
            uncompressed_size
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BlobsJsonOutput, BlobsJsonOutputEntry, extract_blobs, make_blob_image};
    use assert_matches::assert_matches;
    use delivery_blob::compression::CompressionAlgorithm;
    use fxfs::blob_metadata::FxfsBlobMetadataExt;
    use fxfs::filesystem::FxFilesystem;
    use fxfs::object_store::StoreOptions;
    use fxfs::object_store::directory::Directory;
    use fxfs::object_store::volume::root_volume;
    use sparse::reader::SparseReader;
    use std::fs::File;
    use std::io::{Seek as _, SeekFrom, Write};
    use std::path::Path;
    use std::str::from_utf8;
    use storage_device::DeviceHolder;
    use storage_device::file_backed_device::FileBackedDevice;
    use tempfile::TempDir;

    #[fuchsia::test(threads = 10)]
    async fn test_extract_blobs_zstd() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        let input_blob_path = dir.join("input.txt");
        let image_path = dir.join("fxfs1.blk");
        let sparse_path = dir.join("fxfs1.sparse.blk");
        let out_dir = dir.join("extracted_out");

        let data = "C".repeat(128 * 1024);
        std::fs::write(&input_blob_path, &data).unwrap();

        let merkle_hash = fuchsia_merkle::root_from_slice(data.as_bytes());

        make_blob_image(
            image_path.to_str().unwrap(),
            Some(sparse_path.to_str().unwrap()),
            vec![(merkle_hash, input_blob_path.clone())],
            dir.join("blobs1.json").to_str().unwrap(),
            None,
            Some(CompressionAlgorithm::Zstd),
        )
        .await
        .expect("make_blob_image failed");

        extract_blobs(sparse_path, out_dir.clone())
            .await
            .expect("Extraction failed inside extract_blobs");

        let mut extracted_files = std::fs::read_dir(&out_dir).expect("out_dir should exist");
        let first_entry = extracted_files
            .next()
            .expect("No files were extracted!")
            .expect("Failed to read directory entry");

        let extracted_blob_path = first_entry.path();
        let final_len = std::fs::metadata(&extracted_blob_path).unwrap().len();

        assert_eq!(
            final_len,
            data.len() as u64,
            "Decompressed data size does not match original size",
        );
    }

    #[fuchsia::test(threads = 10)]
    async fn test_extract_blobs_lz4() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        let input_blob_path = dir.join("input.txt");
        let image_path = dir.join("fxfs1.blk");
        let sparse_path = dir.join("fxfs1.sparse.blk");
        let out_dir = dir.join("extracted_out");

        let data = "C".repeat(128 * 1024);
        std::fs::write(&input_blob_path, &data).unwrap();

        let merkle_hash = fuchsia_merkle::root_from_slice(data.as_bytes());

        make_blob_image(
            image_path.to_str().unwrap(),
            Some(sparse_path.to_str().unwrap()),
            vec![(merkle_hash, input_blob_path.clone())],
            dir.join("blobs1.json").to_str().unwrap(),
            None,
            Some(CompressionAlgorithm::Lz4),
        )
        .await
        .expect("make_blob_image failed");

        extract_blobs(sparse_path, out_dir.clone())
            .await
            .expect("Extraction failed inside extract_blobs");

        let mut extracted_files = std::fs::read_dir(&out_dir).expect("out_dir should exist");
        let first_entry = extracted_files
            .next()
            .expect("No files were extracted!")
            .expect("Failed to read directory entry");

        let extracted_blob_path = first_entry.path();
        let final_len = std::fs::metadata(&extracted_blob_path).unwrap().len();

        assert_eq!(
            final_len,
            data.len() as u64,
            "Decompressed data size does not match original size",
        );
    }

    #[fuchsia::test(threads = 10)]
    async fn test_make_blob_image() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let blobs_in = {
            let write_data = |path, data: &str| {
                let mut file = File::create(&path).unwrap();
                write!(file, "{}", data).unwrap();
                let root = fuchsia_merkle::root_from_slice(data);
                (root, path)
            };
            vec![
                write_data(dir.join("stuff1.txt"), "Goodbye, stranger!"),
                write_data(dir.join("stuff2.txt"), "It's been nice!"),
                write_data(dir.join("stuff3.txt"), from_utf8(&['a' as u8; 65_537]).unwrap()),
            ]
        };

        let dir = tmp.path();
        let output_path = dir.join("fxfs.blk");
        let sparse_path = dir.join("fxfs.sparse.blk");
        let blobs_json_path = dir.join("blobs.json");
        make_blob_image(
            output_path.as_os_str().to_str().unwrap(),
            Some(sparse_path.as_os_str().to_str().unwrap()),
            blobs_in,
            blobs_json_path.as_os_str().to_str().unwrap(),
            /*target_size=*/ None,
            Some(CompressionAlgorithm::Zstd),
        )
        .await
        .expect("make_blob_image failed");

        // Check that the blob manifest contains the entries we expect.
        let mut blobs_json = std::fs::OpenOptions::new()
            .read(true)
            .open(blobs_json_path)
            .expect("Failed to open blob manifest");
        let mut blobs: BlobsJsonOutput =
            serde_json::from_reader(&mut blobs_json).expect("Failed to serialize to JSON output");

        assert_eq!(blobs.len(), 3);
        blobs.sort_by_key(|entry| entry.source_path.clone());

        assert_eq!(Path::new(blobs[0].source_path.as_str()), dir.join("stuff1.txt"));
        assert_matches!(
            &blobs[0],
            BlobsJsonOutputEntry {
                merkle,
                bytes: 18,
                size: 4096,
                file_size: 18,
                merkle_tree_size: 0,
                used_space_in_blobfs: 4096,
                ..
            } if merkle == "9a24fe2fb8da617f39d303750bbe23f4e03a8b5f4d52bc90b2e5e9e44daddb3a"
        );
        assert_eq!(Path::new(blobs[1].source_path.as_str()), dir.join("stuff2.txt"));
        assert_matches!(
            &blobs[1],
            BlobsJsonOutputEntry {
                merkle,
                bytes: 15,
                size: 4096,
                file_size: 15,
                merkle_tree_size: 0,
                used_space_in_blobfs: 4096,
                ..
            } if merkle == "deebe5d5a0a42a51a293b511d0368e6f2b4da522ee0f05c6ae728c77d904f916"
        );
        assert_eq!(Path::new(blobs[2].source_path.as_str()), dir.join("stuff3.txt"));
        assert_matches!(
            &blobs[2],
            BlobsJsonOutputEntry {
                merkle,
                bytes: 65537,
                // This is technically sensitive to compression, but a string of 'a' should
                // always compress down to a single block.
                size: 8192,
                file_size: 65537,
                merkle_tree_size: 308,
                used_space_in_blobfs: 8192,
                ..
            } if merkle == "1194c76d2d3b61f29df97a85ede7b2fd2b293b452f53072356e3c5c939c8131d"
        );

        let unsparsed_image = {
            let sparse_image = std::fs::OpenOptions::new().read(true).open(sparse_path).unwrap();
            let mut reader = SparseReader::new(sparse_image).expect("Failed to parse sparse image");

            let unsparsed_image_path = dir.join("fxfs.unsparsed.blk");
            let mut unsparsed_image = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(unsparsed_image_path)
                .unwrap();

            std::io::copy(&mut reader, &mut unsparsed_image).expect("Failed to unsparse");
            unsparsed_image.seek(SeekFrom::Start(0)).unwrap();
            unsparsed_image
        };

        let orig_image = std::fs::OpenOptions::new()
            .read(true)
            .open(output_path.clone())
            .expect("Failed to open image");

        assert_eq!(unsparsed_image.metadata().unwrap().len(), orig_image.metadata().unwrap().len());

        // Verify the images created are valid Fxfs images and contains the blobs we expect.
        for image in [orig_image, unsparsed_image] {
            let device = DeviceHolder::new(FileBackedDevice::new(image, 4096));
            let filesystem = FxFilesystem::open(device).await.unwrap();
            let root_volume = root_volume(filesystem.clone()).await.expect("Opening root volume");
            let vol =
                root_volume.volume("blob", StoreOptions::default()).await.expect("Opening volume");
            let directory = Directory::open(&vol, vol.root_directory_object_id())
                .await
                .expect("Opening root dir");
            let entries = {
                let layer_set = directory.store().tree().layer_set();
                let mut merger = layer_set.merger();
                let mut iter = directory.iter(&mut merger).await.expect("iter failed");
                let mut entries = vec![];
                while let Some((name, _, _)) = iter.get() {
                    entries.push(name.to_string());
                    iter.advance().await.expect("advance failed");
                }
                entries
            };
            assert_eq!(
                &entries[..],
                &[
                    "1194c76d2d3b61f29df97a85ede7b2fd2b293b452f53072356e3c5c939c8131d",
                    "9a24fe2fb8da617f39d303750bbe23f4e03a8b5f4d52bc90b2e5e9e44daddb3a",
                    "deebe5d5a0a42a51a293b511d0368e6f2b4da522ee0f05c6ae728c77d904f916",
                ]
            );
        }
    }

    #[fuchsia::test(threads = 10)]
    async fn test_make_uncompressed_blob_image() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let path = dir.join("large_blob.txt");
        let mut file = File::create(&path).unwrap();
        let data = vec![0xabu8; 32 * 1024 * 1024];
        file.write_all(&data).unwrap();
        let root = fuchsia_merkle::root_from_slice(&data);
        let blobs_in = vec![(root, path)];

        let compressed_path = dir.join("fxfs-compressed.blk");
        let blobs_json_path = dir.join("blobs.json");
        make_blob_image(
            compressed_path.as_os_str().to_str().unwrap(),
            None,
            blobs_in.clone(),
            blobs_json_path.as_os_str().to_str().unwrap(),
            /*target_size=*/ None,
            Some(CompressionAlgorithm::Zstd),
        )
        .await
        .expect("make_blob_image failed");

        let uncompressed_path = dir.join("fxfs-uncompressed.blk");
        make_blob_image(
            uncompressed_path.as_os_str().to_str().unwrap(),
            None,
            blobs_in,
            blobs_json_path.as_os_str().to_str().unwrap(),
            /*target_size=*/ None,
            /*compression_algorithm=*/ None,
        )
        .await
        .expect("make_blob_image failed");

        assert!(
            std::fs::metadata(compressed_path).unwrap().len()
                < std::fs::metadata(uncompressed_path).unwrap().len()
        )
    }

    #[fuchsia::test(threads = 10)]
    async fn test_make_blob_image_with_target_size() {
        const TARGET_SIZE: u64 = 200 * 1024 * 1024;
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let path = dir.join("large_blob.txt");
        let mut file = File::create(&path).unwrap();
        let data = vec![0xabu8; 8 * 1024 * 1024];
        file.write_all(&data).unwrap();
        let root = fuchsia_merkle::root_from_slice(&data);
        let blobs_in = vec![(root, path)];

        let image_path = dir.join("fxfs.blk");
        let sparse_image_path = dir.join("fxfs.sparse.blk");
        let blobs_json_path = dir.join("blobs.json");
        make_blob_image(
            image_path.as_os_str().to_str().unwrap(),
            Some(sparse_image_path.as_os_str().to_str().unwrap()),
            blobs_in.clone(),
            blobs_json_path.as_os_str().to_str().unwrap(),
            /*target_size=*/ Some(200 * 1024 * 1024),
            Some(CompressionAlgorithm::Zstd),
        )
        .await
        .expect("make_blob_image failed");

        // The fxfs image is small but gets padded with zeros up to the target size. The zeros
        // should be replaced with a don't care chunk in the sparse format making it much smaller.
        let image_size = std::fs::metadata(image_path).unwrap().len();
        let sparse_image_size = std::fs::metadata(sparse_image_path).unwrap().len();
        assert_eq!(image_size, TARGET_SIZE);
        assert!(sparse_image_size < TARGET_SIZE, "Sparse image size: {sparse_image_size}");
    }

    #[fuchsia::test(threads = 10)]
    async fn test_extract_blobs_path_traversal() {
        use super::{
            BLOB_VOLUME_NAME, BLOCK_SIZE, BlobFormat, BlobMetadata, DirectWriter,
            FxFilesystemBuilder, HandleOptions, LockKey, NewChildStoreOptions, SuperBlockInstance,
            create_sparse_image,
        };
        use fxfs::object_handle::WriteBytes;
        use fxfs::object_store::transaction::lock_keys;

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let image_size = 10 * 1024 * 1024;

        let image_path = dir.join("malicious.blk");

        // Create a minimal Fxfs image with a malicious filename.
        let output_image = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&image_path)
            .unwrap();
        output_image.set_len(image_size).unwrap();

        let device =
            DeviceHolder::new(FileBackedDevice::new(output_image, BLOCK_SIZE.get() as u32));
        let fs = FxFilesystemBuilder::new()
            .format(true)
            .trim_config(None)
            .image_builder_mode(Some(SuperBlockInstance::A))
            .open(device)
            .await
            .unwrap();
        fs.enable_allocations();
        let root_volume = root_volume(fs.clone()).await.unwrap();
        let vol = root_volume
            .new_volume(BLOB_VOLUME_NAME, NewChildStoreOptions::default())
            .await
            .unwrap();
        let blob_directory = Directory::open(&vol, vol.root_directory_object_id()).await.unwrap();

        // Create a file with a malicious name.
        let malicious_name = "../prevent_escaped_write.txt";
        let keys = lock_keys![LockKey::object(
            blob_directory.store().store_object_id(),
            blob_directory.object_id(),
        )];
        let mut transaction =
            blob_directory.store().new_transaction(keys, Default::default()).await.unwrap();
        let handle = blob_directory
            .create_child_file_with_options(
                &mut transaction,
                malicious_name,
                HandleOptions { skip_checksums: true, ..Default::default() },
            )
            .await
            .unwrap();
        transaction.commit().await.unwrap();

        // Write some placeholder data.
        {
            let mut writer = DirectWriter::new(&handle, Default::default()).await;
            writer.write_bytes(b"malicious data").await.unwrap();
            writer.complete().await.unwrap();
        }
        // Write uncompressed metadata (simplest).
        let metadata = BlobMetadata { merkle_leaves: vec![], format: BlobFormat::Uncompressed };
        metadata.write_to(&handle).await.unwrap();

        fs.close().await.unwrap();

        let sparse_path = dir.join("malicious.sparse.blk");
        create_sparse_image(
            sparse_path.to_str().unwrap(),
            image_path.to_str().unwrap(),
            image_size,
            image_size,
            BLOCK_SIZE,
        )
        .unwrap();

        // Now try to extract it. It should fail.
        let extract_dir = dir.join("normal_out");
        std::fs::create_dir(&extract_dir).unwrap();

        let err = extract_blobs(sparse_path, extract_dir.clone()).await.unwrap_err();
        assert_eq!(err.to_string(), "Invalid blob name: ../prevent_escaped_write.txt");

        // Ensure the file was NOT created outside the output directory.
        let escaped_path = dir.join("prevent_escaped_write.txt");
        assert!(!escaped_path.exists());
    }
}
