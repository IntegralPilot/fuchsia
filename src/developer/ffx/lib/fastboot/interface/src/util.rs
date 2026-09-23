// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Several methods operate on u64 but need size_of::<u32>()
pub const U32_SIZE: u64 = std::mem::size_of::<u32>() as u64;

/// Try to convert from T to U and log any error.
pub fn convert_log_err<T, U>(val: T) -> Result<U, <U as TryFrom<T>>::Error>
where
    U: TryFrom<T>,
    <U as TryFrom<T>>::Error: std::error::Error,
{
    val.try_into().inspect_err(|e| log::error!("Conversion error: {e}"))
}

/// Chain an arbitrary number of iterators.
#[macro_export]
macro_rules! multi_chain (
    ($datum:expr $(,)?) => {
        $datum
    };
    ($datum_1:expr, $( $data:expr ),+ $(,)? ) => {
        core::iter::chain($datum_1, multi_chain!($($data,)*))
    };
);

pub use multi_chain;
