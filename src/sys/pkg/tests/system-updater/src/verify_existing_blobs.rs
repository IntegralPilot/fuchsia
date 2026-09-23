// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use pretty_assertions::assert_eq;

#[fuchsia::test]
async fn verifies_existing_blobs_if_enabled() {
    let blob_content = b"valid blob content";
    let blob_hash = fuchsia_merkle::root_from_slice(blob_content);
    let blob = manifest::Blob {
        uncompressed_size: blob_content.len() as u64,
        fuchsia_merkle_root: blob_hash,
    };

    let (sender, mut receiver) = futures::channel::mpsc::unbounded();
    let mock_blob_reader = move |mut stream: ffxfs::BlobReaderRequestStream| {
        let expected_blob_hash = *blob_hash;
        let sender = sender.clone();
        fasync::Task::spawn(async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    ffxfs::BlobReaderRequest::GetVmo { blob_hash, responder } => {
                        assert_eq!(blob_hash, expected_blob_hash);
                        sender.unbounded_send(()).unwrap();
                        let vmo = zx::Vmo::create(blob_content.len() as u64).unwrap();
                        vmo.write(blob_content, 0).unwrap();
                        responder.send(Ok(vmo)).unwrap();
                    }
                }
            }
        })
        .detach();
    };

    let env = TestEnv::builder()
        .verify_existing_blobs(true)
        .ota_manifest(make_manifest(vec![blob]))
        .mock_blob_reader(mock_blob_reader)
        .build()
        .await;
    env.write_to_blobfs(blob_hash, blob_content).await;

    env.run_packageless_update().await.unwrap();

    assert_matches!(receiver.next().await, Some(()));

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![empty_merkle().into(), blob_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![blob_hash.into()]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

// TODO(https://fxbug.dev/469472560): test corrupted blobs with real blobfs
#[fuchsia::test]
async fn re_fetches_corrupt_blob() {
    let blob_content = b"valid blob content";
    let blob_hash = fuchsia_merkle::root_from_slice(blob_content);
    let blob = manifest::Blob {
        uncompressed_size: blob_content.len() as u64,
        fuchsia_merkle_root: blob_hash,
    };

    let mock_blob_reader = move |mut stream: ffxfs::BlobReaderRequestStream| {
        let expected_blob_hash = *blob_hash;
        fasync::Task::spawn(async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    ffxfs::BlobReaderRequest::GetVmo { blob_hash, responder } => {
                        assert_eq!(blob_hash, expected_blob_hash);
                        responder.send(Err(zx::Status::IO_DATA_INTEGRITY.into_raw())).unwrap();
                    }
                }
            }
        })
        .detach();
    };

    let env = TestEnv::builder()
        .verify_existing_blobs(true)
        .ota_manifest(make_manifest(vec![blob]))
        .mock_blob_reader(mock_blob_reader)
        .blob(blob_hash, blob_content.into())
        .build()
        .await;
    env.write_to_blobfs(blob_hash, blob_content).await;

    env.run_packageless_update().await.unwrap();

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![empty_merkle().into(), blob_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedBlobs(vec![blob_hash.into()]),
        Gc,
        OtaDownloader(OtaDownloaderEvent::FetchBlob(blob_hash.into())),
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}
