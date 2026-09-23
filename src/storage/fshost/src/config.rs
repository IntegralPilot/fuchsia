// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
pub fn default_test_config() -> fshost_config::Config {
    fshost_config::Config {
        blobfs: true,
        blobfs_initial_inodes: 0,
        blob_max_bytes: 0,
        blobfs_use_deprecated_padded_format: false,
        data: true,
        data_filesystem_format: String::new(),
        data_max_bytes: 0,
        disable_block_watcher: false,
        factory: false,
        format_data_on_corruption: true,
        check_filesystems: true,
        fvm: true,
        ramdisk_image: false,
        fvm_slice_size: 1024 * 1024, // Default to 1 MiB slice size for tests.
        fxfs_blob: false,
        fxfs_crypt_url: String::from("#meta/fxfs-crypt.cm"),
        gpt: true,
        gpt_all: false,
        merge_super_and_userdata: false,
        no_zxcrypt: false,
        starnix_volume_name: "".to_string(),
        inline_crypto: false,
        disable_automount: false,
        provision_fxfs: false,
        watch_deprecated_v1_drivers: false,
    }
}
