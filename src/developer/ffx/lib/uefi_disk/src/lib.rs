// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Shared utilities for preparing UEFI GPT full disk images and staging ZBI/VBMeta boot items.

mod boot_data;
mod builder;
mod vbmeta;

pub use boot_data::{
    SSH_BOOTLOADER_FILE_NAME, authorized_keys_to_boot_loader_file, embed_boot_data,
};
pub use builder::{
    DEFAULT_EMU_DISK_SIZE, DEFAULT_ZEDBOOT_CMDLINE, FuchsiaFullDiskImageBuilder,
    write_zedboot_cmdline,
};
pub use vbmeta::{FUCHSIA_HASH_DESCRIPTOR_NAME, generate_vbmeta};
