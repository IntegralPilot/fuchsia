// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::filesystems::{BlobFilesystem, DeliveryBlob};
use crate::testing::RamdiskFactory;
use delivery_blob::CompressionMode;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::vec::Vec;
use storage_benchmarks::{CacheClearableFilesystem, FilesystemConfig};

const DEVICE_SIZE: u64 = 128 * 1024 * 1024;
const BLOCK_SIZE: u64 = 4 * 1024;
const BLOCK_COUNT: u64 = DEVICE_SIZE / BLOCK_SIZE;

/// Starts the filesystem on a ramdisk and ensures that a file can be written to the filesystem and
/// be read back.
pub async fn check_filesystem<T: CacheClearableFilesystem>(
    filesystem: impl FilesystemConfig<Filesystem = T>,
) {
    const FILE_CONTENTS: &str = "file-contents";

    let ramdisk_factory = RamdiskFactory::new(BLOCK_SIZE, BLOCK_COUNT);
    let mut fs = filesystem.start_filesystem(&ramdisk_factory).await;

    let file_path = fs.benchmark_dir().join("filename");
    {
        let mut file = OpenOptions::new().create_new(true).write(true).open(&file_path).unwrap();
        file.write_all(FILE_CONTENTS.as_bytes()).unwrap();
    }
    fs.clear_cache().await;
    {
        let mut file = OpenOptions::new().read(true).open(&file_path).unwrap();
        let mut buf = [0u8; FILE_CONTENTS.len()];
        file.read_exact(&mut buf).unwrap();
        assert_eq!(std::str::from_utf8(&buf).unwrap(), FILE_CONTENTS);
    }
    fs.shutdown().await;
}

/// Starts the filesystem on a ramdisk and ensures that a blob can be written to the filesystem and
/// be read back.
pub async fn check_blob_filesystem<T: BlobFilesystem>(
    filesystem: impl FilesystemConfig<Filesystem = T>,
) {
    let blob_contents = Vec::from("blob-contents");
    let blob = DeliveryBlob::new(blob_contents.clone(), CompressionMode::Always);

    let ramdisk_factory = RamdiskFactory::new(BLOCK_SIZE, BLOCK_COUNT);
    let mut fs = filesystem.start_filesystem(&ramdisk_factory).await;
    fs.write_blob(&blob).await;
    fs.clear_cache().await;

    let vmo = fs.get_vmo(&blob.name).await;
    let mut buf = vec![0; vmo.get_content_size().unwrap() as usize];
    let () = vmo.read(&mut buf, 0).unwrap();
    assert_eq!(buf, blob_contents);

    fs.shutdown().await;
}
