// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{TestEnv, write_needed_blob};
use fidl_fuchsia_pkg::NeededBlobsMarker;
use fidl_fuchsia_pkg_ext as pkg;
use futures::StreamExt as _;

#[fuchsia::test]
async fn write_blobs_single() {
    let env = TestEnv::builder().fxblob().build().await;
    let (needed_blobs, needed_blobs_server_end) =
        fidl::endpoints::create_proxy::<NeededBlobsMarker>();
    let () = env.proxies.package_cache.write_blobs(needed_blobs_server_end).unwrap();

    let blob_content = b"blob_content";
    let hash = fuchsia_merkle::root_from_slice(blob_content);

    let () = write_needed_blob(&needed_blobs, hash, blob_content).await;
    assert!(env.blobfs.list_blobs().unwrap().contains(&hash));

    let () = env.stop().await;
}

#[fuchsia::test]
async fn write_blobs_existing() {
    let env = TestEnv::builder().fxblob().build().await;
    let (needed_blobs, needed_blobs_server_end) =
        fidl::endpoints::create_proxy::<NeededBlobsMarker>();
    let () = env.proxies.package_cache.write_blobs(needed_blobs_server_end).unwrap();

    let existing_blobs = env.blobfs.list_blobs().unwrap();
    assert!(!existing_blobs.is_empty());

    let () = futures::stream::iter(&existing_blobs)
        .for_each_concurrent(None, async |hash| {
            assert_eq!(
                needed_blobs
                    .open_blob(&pkg::BlobId::from(*hash).into(), false)
                    .await
                    .unwrap()
                    .unwrap(),
                None
            );
        })
        .await;
    assert_eq!(env.blobfs.list_blobs().unwrap(), existing_blobs);

    let () = env.stop().await;
}

#[fuchsia::test]
async fn write_blobs_concurrent() {
    let env = TestEnv::builder().fxblob().build().await;
    let (needed_blobs, needed_blobs_server_end) =
        fidl::endpoints::create_proxy::<NeededBlobsMarker>();
    let () = env.proxies.package_cache.write_blobs(needed_blobs_server_end).unwrap();

    let blobs: Vec<_> = (1..100)
        .map(|i| {
            let blob_content = vec![i as u8; i];
            let hash = fuchsia_merkle::root_from_slice(&blob_content);
            (hash, blob_content)
        })
        .collect();

    let () = futures::stream::iter(&blobs)
        .for_each_concurrent(None, async |(hash, blob_content)| {
            let () = write_needed_blob(&needed_blobs, *hash, blob_content).await;
        })
        .await;
    assert!(
        env.blobfs
            .list_blobs()
            .unwrap()
            .is_superset(&blobs.into_iter().map(|(hash, _)| hash).collect())
    );

    let () = env.stop().await;
}
