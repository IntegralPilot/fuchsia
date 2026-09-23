// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Compile-time assertion.
/// Fails to compile if the condition is false.
#[macro_export]
macro_rules! static_assert {
    ($x:expr $(,)?) => {
        const _: [(); 0 - !{
            const ASSERT: bool = $x;
            ASSERT
        } as usize] = [];
    };
}

/// Compile-time assertion that a type's size is <= `max_size` and alignment == `expected_align`.
#[macro_export]
macro_rules! static_assert_size_and_align {
    ($ty:ty, $max_size:expr, $expected_align:expr $(,)?) => {
        $crate::static_assert!(core::mem::size_of::<$ty>() <= $max_size as usize);
        $crate::static_assert!(core::mem::align_of::<$ty>() == $expected_align as usize);
    };
}
