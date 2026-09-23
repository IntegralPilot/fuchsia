// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use assert_matches::assert_matches;
use test_case::test_case;

#[test_case(Implementation::CppBlobfs; "blobfs")]
#[test_case(Implementation::Fxblob; "fxblob")]
#[fuchsia::test]
async fn open_missing_fails(implementation: Implementation) {
    let blobfs = BlobfsRamdisk::builder().implementation(implementation).start().await.unwrap();

    let bytes = vec![0u8; 64];
    let hash = fuchsia_merkle::root_from_slice(&bytes);
    let reader = blobfs.blob_reader_proxy().unwrap();
    assert_eq!(reader.get_vmo(&hash.into()).await.unwrap(), Err(zx::Status::NOT_FOUND.into_raw()));
    blobfs.stop().await.unwrap();
}

#[test_case(Implementation::CppBlobfs; "blobfs")]
#[test_case(Implementation::Fxblob; "fxblob")]
#[fuchsia::test]
async fn corrupt_create_fails_on_last_byte_write(implementation: Implementation) {
    let blobfs = BlobfsRamdisk::builder().implementation(implementation).start().await.unwrap();
    let creator = blobfs.blob_creator_proxy().unwrap();

    // 8,194 bytes so that the partial write exceeds 8,192 bytes.
    let mut bytes = vec![0u8; 8194];
    let hash = fuchsia_merkle::root_from_slice(&bytes);
    // Corrupt the last byte.
    bytes[8193] ^= 1;
    let compressed = Type1Blob::generate(&bytes, CompressionMode::Never);
    let compressed_len: u64 = compressed.len().try_into().unwrap();

    let writer = creator.create(&hash, false).await.unwrap().unwrap().into_proxy();
    let vmo = writer.get_vmo(compressed_len).await.unwrap().unwrap();
    let () = vmo.write(&compressed, 0).unwrap();
    let () = writer.bytes_ready(compressed_len - 1).await.unwrap().unwrap();
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::new());

    assert_matches!(
        writer.bytes_ready(1).await.unwrap().map_err(zx::Status::err_from_raw),
        Err(zx::Status::IO_DATA_INTEGRITY)
    );

    blobfs.stop().await.unwrap();
}

#[fuchsia::test]
async fn cppblobfs_concurrent_create_fails() {
    let blobfs = BlobfsRamdisk::builder().cpp_blobfs().start().await.unwrap();
    let creator = blobfs.blob_creator_proxy().unwrap();

    let bytes = [0u8; 1];
    let hash = fuchsia_merkle::root_from_slice(&bytes[..]);

    let _writer0 = creator.create(&hash, false).await.unwrap().unwrap().into_proxy();
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::new());

    assert_matches!(
        creator.create(&hash, false).await.unwrap(),
        Err(ffxfs::CreateBlobError::AlreadyExists)
    );
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::new());

    blobfs.stop().await.unwrap();
}

#[fuchsia::test]
async fn fxblob_concurrent_creation_succeeds() {
    let blobfs = BlobfsRamdisk::builder().fxblob().start().await.unwrap();
    let creator = blobfs.blob_creator_proxy().unwrap();

    let bytes = vec![0u8; 100];
    let hash = fuchsia_merkle::root_from_slice(&bytes);
    let compressed = Type1Blob::generate(&bytes, CompressionMode::Never);
    let compressed_len: u64 = compressed.len().try_into().unwrap();

    // Create and write all but the last byte.
    let writer0 = creator.create(&hash, false).await.unwrap().unwrap().into_proxy();
    let vmo0 = writer0.get_vmo(compressed_len).await.unwrap().unwrap();
    let () = vmo0.write(&compressed, 0).unwrap();
    let () = writer0.bytes_ready(compressed_len - 1).await.unwrap().unwrap();
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::new());

    // Fully creating on another writer still works.
    let writer1 = creator.create(&hash, false).await.unwrap().unwrap().into_proxy();
    let vmo1 = writer1.get_vmo(compressed_len).await.unwrap().unwrap();
    let () = vmo1.write(&compressed, 0).unwrap();
    let () = writer1.bytes_ready(compressed_len).await.unwrap().unwrap();
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::from([hash]));

    // Finishing the initial create by writing the final byte also works.
    let () = writer0.bytes_ready(1).await.unwrap().unwrap();
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::from([hash]));

    blobfs.stop().await.unwrap();
}

#[test_case(Implementation::CppBlobfs; "blobfs")]
#[test_case(Implementation::Fxblob; "fxblob")]
#[fuchsia::test]
async fn create_already_present_returns_already_exists(implementation: Implementation) {
    let blobfs = BlobfsRamdisk::builder().implementation(implementation).start().await.unwrap();
    let creator = blobfs.blob_creator_proxy().unwrap();

    let bytes = vec![0u8; 1];
    let hash = fuchsia_merkle::root_from_slice(&bytes);
    let compressed = Type1Blob::generate(&bytes, CompressionMode::Never);
    let compressed_len: u64 = compressed.len().try_into().unwrap();

    let writer0 = creator.create(&hash, false).await.unwrap().unwrap().into_proxy();
    let vmo0 = writer0.get_vmo(compressed_len).await.unwrap().unwrap();
    let () = vmo0.write(&compressed, 0).unwrap();
    let () = writer0.bytes_ready(compressed_len).await.unwrap().unwrap();
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::from([hash]));

    assert_matches!(
        creator.create(&hash, false).await,
        Ok(Err(ffxfs::CreateBlobError::AlreadyExists))
    );

    blobfs.stop().await.unwrap();
}

// ReadDirents on /blob should only return blobs if they are fully written and do not have
// outstanding deletion requests.
#[test_case(Implementation::CppBlobfs; "blobfs")]
#[test_case(Implementation::Fxblob; "fxblob")]
#[fuchsia::test]
async fn readdirents_only_returns_valid_blobs(implementation: Implementation) {
    let blobfs_server =
        BlobfsRamdisk::builder().implementation(implementation).start().await.unwrap();
    let creator = blobfs_server.blob_creator_proxy().unwrap();
    let bytes = vec![0u8; 2];
    let hash = fuchsia_merkle::root_from_slice(&bytes);
    let compressed = Type1Blob::generate(&bytes, CompressionMode::Never);
    let compressed_len: u64 = compressed.len().try_into().unwrap();

    // Blob doesn't appear until it is fully written.
    assert_eq!(blobfs_server.list_blobs().unwrap(), BTreeSet::new());

    let writer0 = creator.create(&hash, false).await.unwrap().unwrap().into_proxy();
    assert_eq!(blobfs_server.list_blobs().unwrap(), BTreeSet::new());

    let vmo0 = writer0.get_vmo(compressed_len).await.unwrap().unwrap();
    assert_eq!(blobfs_server.list_blobs().unwrap(), BTreeSet::new());

    let () = vmo0.write(&compressed, 0).unwrap();
    let () = writer0.bytes_ready(compressed_len - 1).await.unwrap().unwrap();
    assert_eq!(blobfs_server.list_blobs().unwrap(), BTreeSet::new());

    let () = writer0.bytes_ready(1).await.unwrap().unwrap();
    assert_eq!(blobfs_server.list_blobs().unwrap(), BTreeSet::from([hash]));

    // Blob disappears once a deletion request has been received, even if an outstanding connection
    // is keeping it alive.
    let reader = blobfs_server.blob_reader_proxy().unwrap();
    let _vmo1: zx::Vmo = reader.get_vmo(&hash.into()).await.unwrap().unwrap();

    let () = blobfs_server.client().delete_blob(&hash).await.unwrap();
    assert_eq!(blobfs_server.list_blobs().unwrap(), BTreeSet::new());

    let () = blobfs_server.stop().await.unwrap();
}

#[test_case(Implementation::CppBlobfs; "blobfs")]
#[test_case(Implementation::Fxblob; "fxblob")]
#[fuchsia::test]
async fn create_partial_write_drop_create(implementation: Implementation) {
    let blobfs = BlobfsRamdisk::builder().implementation(implementation).start().await.unwrap();
    let creator = blobfs.blob_creator_proxy().unwrap();

    let bytes = vec![0u8; 100];
    let hash = fuchsia_merkle::root_from_slice(&bytes);
    let compressed = Type1Blob::generate(&bytes, CompressionMode::Never);
    let compressed_len: u64 = compressed.len().try_into().unwrap();

    // Create and write all but the last byte.
    let writer0 = creator.create(&hash, false).await.unwrap().unwrap().into_proxy();
    let vmo0 = writer0.get_vmo(compressed_len).await.unwrap().unwrap();
    let () = vmo0.write(&compressed, 0).unwrap();
    let () = writer0.bytes_ready(compressed_len - 1).await.unwrap().unwrap();
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::new());

    // Dropping the writer should allow immediate re-creation, even for implementations that do not
    // support concurrent creation (of course implementations that support concurrent creation, i.e.
    // fxblob, would pass this test even without dropping the writer).
    // This is because dropping the writer proxy synchronously closes the client end's zircon
    // channel, and cppblobfs (which does not support concurrent creation) checks the client ends of
    // outstanding creation attempts when it starts handling a new creation attempt, and will cancel
    // any attempts with closed client ends before continuing.
    drop(writer0);
    let writer1 = creator.create(&hash, false).await.unwrap().unwrap().into_proxy();
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::new());
    let vmo1 = writer1.get_vmo(compressed_len).await.unwrap().unwrap();
    let () = vmo1.write(&compressed, 0).unwrap();
    let () = writer1.bytes_ready(compressed_len).await.unwrap().unwrap();
    assert_eq!(blobfs.list_blobs().unwrap(), BTreeSet::from([hash]));

    blobfs.stop().await.unwrap();
}
