// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::PAGE_SIZE;

use starnix_uapi::uapi;

pub fn default_statfs(magic: u32) -> uapi::statfs {
    uapi::statfs {
        f_type: magic as i64,
        f_bsize: *PAGE_SIZE as i64,
        f_blocks: 0,
        f_bfree: 0,
        f_bavail: 0,
        f_files: 0,
        f_ffree: 0,
        f_fsid: uapi::__kernel_fsid_t::default(),
        f_namelen: uapi::NAME_MAX as i64,
        f_frsize: *PAGE_SIZE as i64,
        f_flags: 0,
        f_spare: [0, 0, 0, 0],
    }
}
