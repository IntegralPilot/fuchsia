// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The config module contains shared code that captures the environment to configure the test
//! runs. It must live in the binary directly, instead of in the fixture crate, which is why it
//! lives here and is recompiled into each separate test.

use fshost_test_fixture::disk_builder::{
    DEFAULT_DATA_VOLUME_SIZE, DEFAULT_DISK_SIZE, DataSpec, FVM_F2FS_SLICE_SIZE, FVM_SLICE_SIZE,
    VolumesSpec,
};
use fshost_test_fixture::{
    TestFixtureBuilder, VFS_TYPE_BLOBFS, VFS_TYPE_F2FS, VFS_TYPE_FXFS, VFS_TYPE_MINFS,
};

pub const FSHOST_COMPONENT_NAME: &'static str = std::env!("FSHOST_COMPONENT_NAME");
pub const DATA_FILESYSTEM_FORMAT: &'static str = std::env!("DATA_FILESYSTEM_FORMAT");
pub const DATA_FILESYSTEM_VARIANT: &'static str = std::env!("DATA_FILESYSTEM_VARIANT");

pub fn new_builder() -> TestFixtureBuilder {
    TestFixtureBuilder::new(FSHOST_COMPONENT_NAME)
}

pub fn blob_fs_type() -> u32 {
    if DATA_FILESYSTEM_VARIANT == "fxblob" { VFS_TYPE_FXFS } else { VFS_TYPE_BLOBFS }
}

pub fn data_fs_type() -> u32 {
    match DATA_FILESYSTEM_FORMAT {
        "f2fs" => VFS_TYPE_F2FS,
        "fxfs" => VFS_TYPE_FXFS,
        "minfs" => VFS_TYPE_MINFS,
        _ => panic!("invalid data filesystem format"),
    }
}

pub fn data_fs_name() -> &'static str {
    match DATA_FILESYSTEM_FORMAT {
        "f2fs" => "f2fs",
        "fxfs" => "fxfs",
        "minfs" => "minfs",
        _ => panic!("invalid data filesystem format"),
    }
}

pub fn data_fs_zxcrypt() -> bool {
    !DATA_FILESYSTEM_VARIANT.ends_with("no-zxcrypt")
}

pub fn volumes_spec() -> VolumesSpec {
    VolumesSpec { fxfs_blob: DATA_FILESYSTEM_VARIANT == "fxblob", create_data_partition: true }
}

pub fn data_fs_spec() -> DataSpec {
    DataSpec { format: Some(data_fs_name()), zxcrypt: data_fs_zxcrypt(), ..Default::default() }
}

pub fn fvm_slice_size() -> u64 {
    if data_fs_name() == "f2fs" { FVM_F2FS_SLICE_SIZE } else { FVM_SLICE_SIZE }
}

pub fn data_max_bytes() -> u64 {
    DEFAULT_DATA_VOLUME_SIZE - (DEFAULT_DATA_VOLUME_SIZE % fvm_slice_size())
}

pub fn disk_size_bytes() -> u64 {
    DEFAULT_DISK_SIZE
}
