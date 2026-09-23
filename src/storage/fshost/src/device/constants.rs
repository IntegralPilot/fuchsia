// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// -- Fxfs volume labels --
pub const BLOB_VOLUME_LABEL: &str = "blob";
pub const BLOB_IMAGE_VOLUME_LABEL: &str = "blob-image";
pub const DATA_VOLUME_LABEL: &str = "data";
pub const UNENCRYPTED_VOLUME_LABEL: &str = "unencrypted";

// -- Partition type GUIDs --
pub const DATA_TYPE_GUID: [u8; 16] = [
    0x0c, 0x5f, 0x18, 0x08, 0x2d, 0x89, 0x8a, 0x42, 0xa7, 0x89, 0xdb, 0xee, 0xc8, 0xf5, 0x5e, 0x6a,
];
pub const LEGACY_FVM_TYPE_GUID: [u8; 16] = [
    0x40, 0xe3, 0xd0, 0x41, 0xe3, 0x57, 0x4e, 0x95, 0x8c, 0x1e, 0x17, 0xec, 0xac, 0x44, 0xcf, 0xf5,
];

pub const DEFAULT_F2FS_MIN_BYTES: u64 = 50 * 1024 * 1024;
