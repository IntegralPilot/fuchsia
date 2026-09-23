// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_fs_startup::StartOptions;

#[derive(Clone, Debug, Default)]
pub struct Config {
    /// If 'super' and 'userdata' exist and are contiguous, they will be merged into a single
    /// overlay partition called 'super_and_userdata'.
    /// NOTE: We will only attempt to merge the first `super` and `userdata` partitions found in the
    /// GPT.  In the unlikely event that multiple such partitions exist, matching will *only* occur
    /// on the first pair found.  All subsequent partitions will simply be bound as regular
    /// partitions.  This is mainly for simplicity, because it's not something we expect to happen
    /// anyways.
    pub merge_super_and_userdata: bool,
}

impl From<StartOptions> for Config {
    fn from(options: StartOptions) -> Self {
        Config { merge_super_and_userdata: options.merge_super_and_userdata.unwrap_or(false) }
    }
}
