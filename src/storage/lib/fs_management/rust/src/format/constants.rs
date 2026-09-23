// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const HEADER_SIZE: u32 = 4096;

// -- Magic numbers--
pub const MINFS_MAGIC: [u8; 16] = [
    0x21, 0x4d, 0x69, 0x6e, 0x46, 0x53, 0x21, 0x00, 0x04, 0xd3, 0xd3, 0xd3, 0xd3, 0x00, 0x50, 0x38,
];

pub const BLOBFS_MAGIC: [u8; 16] = [
    0x21, 0x4d, 0x69, 0x9e, 0x47, 0x53, 0x21, 0xac, 0x14, 0xd3, 0xd3, 0xd4, 0xd4, 0x00, 0x50, 0x98,
];

pub const GPT_MAGIC: [u8; 16] = [
    0x45, 0x46, 0x49, 0x20, 0x50, 0x41, 0x52, 0x54, 0x00, 0x00, 0x01, 0x00, 0x5c, 0x00, 0x00, 0x00,
];

pub const FVM_MAGIC: [u8; 8] = [0x46, 0x56, 0x4d, 0x20, 0x50, 0x41, 0x52, 0x54];

pub const ZXCRYPT_MAGIC: [u8; 16] = [
    0x5f, 0xe8, 0xf8, 0x00, 0xb3, 0x6d, 0x11, 0xe7, 0x80, 0x7a, 0x78, 0x63, 0x72, 0x79, 0x70, 0x74,
];

pub const BLOCK_VERITY_MAGIC: [u8; 16] = [
    0x62, 0x6c, 0x6f, 0x63, 0x6b, 0x2d, 0x76, 0x65, 0x72, 0x69, 0x74, 0x79, 0x2d, 0x76, 0x31, 0x00,
];

pub const VB_META_MAGIC: [u8; 4] = [b'A', b'V', b'B', b'0'];

pub const F2FS_MAGIC: [u8; 4] = [0x10, 0x20, 0xf5, 0xf2];

pub const FXFS_MAGIC: [u8; 8] = [b'F', b'x', b'f', b's', b'S', b'u', b'p', b'r'];

// -- Partition labels --
pub const BLOBFS_PARTITION_LABEL: &str = "blobfs";

pub const DATA_PARTITION_LABEL: &str = "data";

pub const LEGACY_DATA_PARTITION_LABEL: &str = "minfs";

pub const FVM_PARTITION_LABEL: &str = "fvm";

pub const FUCHSIA_FVM_PARTITION_LABEL: &str = "fuchsia-fvm";

pub const FTL_PARTITION_LABEL: &str = "ftl";

pub const SUPER_PARTITION_LABEL: &str = "super";

// NB: This name is a hard-coded value from the GPT component implementation
// (//src/storage/gpt/component).
// TODO(https://fxbug.dev/443980711): This should come from configuration.
pub const SUPER_AND_USERDATA_PARTITION_LABEL: &str = "super_and_userdata";

/// The set of partition labels which might contain the main system partition.
pub const ALL_SYSTEM_PARTITION_LABELS: [&str; 5] = [
    FVM_PARTITION_LABEL,
    FUCHSIA_FVM_PARTITION_LABEL,
    FTL_PARTITION_LABEL,
    SUPER_PARTITION_LABEL,
    SUPER_AND_USERDATA_PARTITION_LABEL,
];

pub const BENCHMARK_FVM_TYPE_GUID: [u8; 16] = [
    0x67, 0x45, 0x23, 0x01, 0xab, 0x89, 0xef, 0xcd, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
];
pub const BENCHMARK_FVM_VOLUME_NAME: &str = "benchmark-fvm";

pub const PAD_RW_PARTITION_LABEL: &str = "pad_rw";

/// The set of partition labels which might be used to run benchmarks out of.
pub const ALL_BENCHMARK_PARTITION_LABELS: [&str; 2] =
    [BENCHMARK_FVM_VOLUME_NAME, PAD_RW_PARTITION_LABEL];
