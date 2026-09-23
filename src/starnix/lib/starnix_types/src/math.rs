// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::PAGE_SIZE;

use starnix_uapi::errors::Errno;
use starnix_uapi::math::{round_down_to_increment, round_up_to_increment};

pub fn round_up_to_system_page_size<N>(size: N) -> Result<N, Errno>
where
    N: TryInto<u64>,
    N: TryFrom<u64>,
{
    round_up_to_increment(size, *PAGE_SIZE)
}

pub fn round_down_to_system_page_size<N>(size: N) -> Result<N, Errno>
where
    N: TryInto<u64>,
    N: TryFrom<u64>,
{
    round_down_to_increment(size, *PAGE_SIZE)
}
