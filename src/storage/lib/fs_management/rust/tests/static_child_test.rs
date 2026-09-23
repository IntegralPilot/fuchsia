// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Test launching filesystems as static child components in a custom environment.

use fidl_fuchsia_fs_startup::{CreateOptions, MountOptions};
use fidl_fuchsia_io as fio;
use fs_management::filesystem::Filesystem;
use fs_management::{Blobfs, ComponentType, Fxfs, Minfs};
use ramdevice_client::RamdiskClient;

#[fuchsia::test]
async fn blobfs_static_child() {
    let ramdisk = RamdiskClient::create(1024, 1 << 16).await.unwrap();

    let config = Blobfs { component_type: ComponentType::StaticChild, ..Default::default() };
    let mut blobfs = Filesystem::from_boxed_config(ramdisk.connector().unwrap(), Box::new(config));

    blobfs.format().await.unwrap();
    blobfs.fsck().await.unwrap();
    let fs = blobfs.serve().await.unwrap();
    fs.shutdown().await.unwrap();
}

#[fuchsia::test]
async fn minfs_static_child() {
    let ramdisk = RamdiskClient::create(1024, 1 << 16).await.unwrap();

    let config = Minfs { component_type: ComponentType::StaticChild, ..Default::default() };
    let mut minfs = Filesystem::from_boxed_config(ramdisk.connector().unwrap(), Box::new(config));

    minfs.format().await.unwrap();
    minfs.fsck().await.unwrap();
    let fs = minfs.serve().await.unwrap();
    fs.shutdown().await.unwrap();
}

#[fuchsia::test]
async fn fxfs_static_child() {
    let ramdisk = RamdiskClient::create(1024, 1 << 16).await.unwrap();

    let config = Fxfs { component_type: ComponentType::StaticChild, ..Default::default() };
    let mut fxfs = Filesystem::from_boxed_config(ramdisk.connector().unwrap(), Box::new(config));

    fxfs.format().await.unwrap();
    fxfs.fsck().await.unwrap();
    let fs = fxfs.serve_multi_volume().await.unwrap();

    let volume =
        fs.create_volume("test", CreateOptions::default(), MountOptions::default()).await.unwrap();
    let _: Box<fio::FilesystemInfo> = volume.query().await.unwrap();

    fs.shutdown().await.unwrap();
}
