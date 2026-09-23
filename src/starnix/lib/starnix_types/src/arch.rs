// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_uapi::user_address::ArchSpecific;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchWidth {
    #[default]
    Arch64,
    #[cfg(target_arch = "aarch64")]
    Arch32,
}

impl ArchSpecific for ArchWidth {
    fn is_arch32(&self) -> bool {
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "aarch64")] {
                self == &Self::Arch32
            } else {
                false
            }
        }
    }
}
