// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
mod tests {
    use blob_writer::BlobWriter;
    use delivery_blob::{CompressionMode, Type1Blob};
    use fidl_fuchsia_fs_startup::{CreateOptions, MountOptions};
    use fs_management::Fxfs;
    use fs_management::filesystem::Filesystem;
    use fuchsia_component::client::connect_to_protocol_at_dir_svc;
    use ramdevice_client::RamdiskClient;
    const DEVICE_SIZE: u64 = 128 * 1024 * 1024;
    const BLOCK_SIZE: u64 = 4 * 1024;
    const BLOCK_COUNT: u64 = DEVICE_SIZE / BLOCK_SIZE;

    #[fuchsia::test]
    async fn small_write_no_wrap_test() {
        let ramdisk = RamdiskClient::create(BLOCK_SIZE, BLOCK_COUNT).await.unwrap();
        let mut fs =
            Filesystem::from_boxed_config(ramdisk.connector().unwrap(), Box::new(Fxfs::default()));
        fs.format().await.expect("Failed to format the filesystem");
        let serving_filesystem =
            fs.serve_multi_volume().await.expect("Failed to start the filesystem");
        let vol = serving_filesystem
            .create_volume(
                "blob",
                CreateOptions::default(),
                MountOptions { as_blob: Some(true), ..MountOptions::default() },
            )
            .await
            .expect("Failed to create volume");
        let blob_creator = connect_to_protocol_at_dir_svc::<fidl_fuchsia_fxfs::BlobCreatorMarker>(
            vol.exposed_dir(),
        )
        .expect("failed to connect to the BlobCreator");

        let blob_reader = connect_to_protocol_at_dir_svc::<fidl_fuchsia_fxfs::BlobReaderMarker>(
            vol.exposed_dir(),
        )
        .expect("failed to connect to the BlobReader");

        let mut data = vec![1; 196608];
        rand::fill(&mut data[..]);

        let hash = fuchsia_merkle::root_from_slice(&data);

        let compressed_data: Vec<u8> = Type1Blob::generate(&data, CompressionMode::Always);

        let writer_client_end = blob_creator
            .create(&hash.into(), false)
            .await
            .expect("transport error on BlobCreator.Create")
            .expect("failed to create blob");
        let writer = writer_client_end.into_proxy();
        let mut blob_writer = BlobWriter::create(writer, compressed_data.len() as u64)
            .await
            .expect("failed to create BlobWriter");

        blob_writer.write(&compressed_data).await.unwrap();

        let vmo = blob_reader
            .get_vmo(&hash.into())
            .await
            .expect("transport error on the BlobReader")
            .expect("get_vmo failed");
        let mut read_data = vec![0; vmo.get_content_size().unwrap() as usize];
        let () = vmo.read(&mut read_data, 0).expect("vmo read failed");
        assert_eq!(data, read_data)
    }

    #[fuchsia::test]
    async fn large_write_wraps_multiple_times_test() {
        let ramdisk = RamdiskClient::create(BLOCK_SIZE, BLOCK_COUNT).await.unwrap();
        let mut fs =
            Filesystem::from_boxed_config(ramdisk.connector().unwrap(), Box::new(Fxfs::default()));
        fs.format().await.expect("Failed to format the filesystem");
        let serving_filesystem =
            fs.serve_multi_volume().await.expect("Failed to start the filesystem");
        let vol = serving_filesystem
            .create_volume(
                "blob",
                CreateOptions::default(),
                MountOptions { as_blob: Some(true), ..MountOptions::default() },
            )
            .await
            .expect("Failed to create volume");
        let blob_creator = connect_to_protocol_at_dir_svc::<fidl_fuchsia_fxfs::BlobCreatorMarker>(
            vol.exposed_dir(),
        )
        .expect("failed to connect to the Blob service");

        let blob_reader = connect_to_protocol_at_dir_svc::<fidl_fuchsia_fxfs::BlobReaderMarker>(
            vol.exposed_dir(),
        )
        .expect("failed to connect to the BlobReader");

        let mut data = vec![1; 499712];
        rand::fill(&mut data[..]);

        let hash = fuchsia_merkle::root_from_slice(&data);

        let compressed_data = Type1Blob::generate(&data, CompressionMode::Always);

        let writer_client_end = blob_creator
            .create(&hash.into(), false)
            .await
            .expect("transport error on BlobCreator.Create")
            .expect("failed to create blob");
        let writer = writer_client_end.into_proxy();

        let mut blob_writer = BlobWriter::create(writer, compressed_data.len() as u64)
            .await
            .expect("failed to create BlobWriter");
        assert!(compressed_data.len() as u64 > blob_writer.vmo_size());

        blob_writer.write(&compressed_data).await.unwrap();

        let vmo = blob_reader
            .get_vmo(&hash.into())
            .await
            .expect("transport error on the BlobReader")
            .expect("get_vmo failed");
        let mut read_data = vec![0; vmo.get_content_size().unwrap() as usize];
        let () = vmo.read(&mut read_data, 0).expect("vmo read failed");
        assert_eq!(data, read_data)
    }
}
