// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl_fuchsia_storage_blobfs::OverwriteFormat;

#[fuchsia::test]
async fn packageless_update_checks_needs_overwrite() {
    let blob_content = "blob content".as_bytes();
    let blob_hash = fuchsia_merkle::root_from_slice(blob_content);

    // fuchsia.storage.blobfs/OverwriteConfiguration is only implemented by c++blobfs.
    let env = TestEnvBuilder::new()
        .cpp_blobfs()
        .ota_manifest(make_manifest([manifest::Blob {
            uncompressed_size: blob_content.len() as u64,
            fuchsia_merkle_root: blob_hash,
        }]))
        .blob(blob_hash, blob_content.to_vec())
        .build()
        .await;

    let overwrite_configuration = env.blobfs.overwrite_configuration_proxy().unwrap();

    let () =
        overwrite_configuration.set(OverwriteFormat::OverwriteToPadded).await.unwrap().unwrap();
    let () = env.blobfs.write_blob(blob_hash, blob_content).await.unwrap();

    let creator = env.blobfs.blob_creator_proxy().unwrap();
    assert!(!creator.needs_overwrite(&blob_hash).await.unwrap().unwrap());

    let () =
        overwrite_configuration.set(OverwriteFormat::OverwriteToCompact).await.unwrap().unwrap();
    assert!(creator.needs_overwrite(&blob_hash).await.unwrap().unwrap());

    env.run_packageless_update().await.unwrap();

    assert!(!creator.needs_overwrite(&blob_hash).await.unwrap().unwrap());
}
