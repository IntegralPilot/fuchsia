// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use crate::*;

pub type stat64 = crate::stat;
pub type sigaction64_t = crate::sigaction_t;

impl From<crate::statfs> for crate::statfs64 {
    fn from(_statfs: crate::statfs) -> Self {
        unreachable!()
    }
}
