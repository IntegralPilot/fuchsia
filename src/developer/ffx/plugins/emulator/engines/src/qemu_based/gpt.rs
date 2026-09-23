// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for preparing GPT full disk images for qemu based emulator runs.

pub(crate) use ffx_uefi_disk::{
    DEFAULT_EMU_DISK_SIZE, FuchsiaFullDiskImageBuilder, write_zedboot_cmdline,
};
