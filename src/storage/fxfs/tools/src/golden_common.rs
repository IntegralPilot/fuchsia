// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use fxfs::object_store::MAX_INLINE_XATTR_SIZE;
use fxfs::serialized_types::{DEFAULT_MAX_SERIALIZED_RECORD_SIZE, LATEST_VERSION};
use fxfs_crypto::WrappingKeyId;

// Since this file is shared between two crates, the generate and the test, only constants and
// methods used in both can be listed here.
pub const BLOB_LIST_PATH: &str = "blob_list";
pub const DEFAULT_VOLUME: &str = "default";
pub const DELETED_FILE_PATH: &str = "some/deleted.txt";
pub const EXPECTED_FILE_CONTENT: &[u8; 8] = b"content.";
pub const IMAGE_BLOCK_SIZE: u32 = 1024;
pub const REGULAR_DIRECTORY_PATH: &str = "some";
pub const REGULAR_FILE_PATH: &str = "some/file.txt";
pub const UNENCRYPTED_VOLUME: &str = "unencrypted";
pub const VERITY_FILE_PATH: &str = "some/fsverity.txt";
pub const WRAPPING_KEY_ID: WrappingKeyId = u128::to_le_bytes(2);
pub const LARGE_ATTR_PREFIX: &str = "user.large_attr";

pub const LARGE_ATTR_COUNT: usize =
    (DEFAULT_MAX_SERIALIZED_RECORD_SIZE as usize).div_ceil(MAX_INLINE_XATTR_SIZE) + 1;

/// Generates the filename where we expect to find a golden image for the current version of the
/// filesystem.
pub fn latest_image_filename() -> String {
    format!("fxfs_golden.{}.{}.img.zstd", LATEST_VERSION.major, LATEST_VERSION.minor)
}
