// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::errors::{Errno, errno};

pub fn round_up_to_increment<N, M>(size: N, increment: M) -> Result<N, Errno>
where
    N: TryInto<u64>,
    N: TryFrom<u64>,
    M: TryInto<u64>,
{
    let size: u64 = size.try_into().map_err(|_| errno!(EINVAL))?;
    let increment: u64 = increment.try_into().map_err(|_| errno!(EINVAL))?;
    let spare = size % increment;
    let result = if spare > 0 {
        size.checked_add(increment - spare).ok_or_else(|| errno!(EINVAL))?
    } else {
        size
    };
    N::try_from(result).map_err(|_| errno!(EINVAL))
}

pub fn round_down_to_increment<N, M>(size: N, increment: M) -> Result<N, Errno>
where
    N: TryInto<u64>,
    N: TryFrom<u64>,
    M: TryInto<u64>,
{
    let size: u64 = size.try_into().map_err(|_| errno!(EINVAL))?;
    let increment: u64 = increment.try_into().map_err(|_| errno!(EINVAL))?;
    let result = size - (size % increment);
    N::try_from(result).map_err(|_| errno!(EINVAL))
}
