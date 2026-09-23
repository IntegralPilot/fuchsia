// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![cfg_attr(not(test), no_std)]

#[cfg(test)]
use kprint as _;

// For use in `kernel_oops!`.
#[doc(hidden)]
pub use kprint::kprint as __kprint;

pub mod dprintf;
pub mod ltrace;

/// A [`kprint`]-like macro which prepend's the user's message with the special
/// "ZIRCON KERNEL OOPS" tag. Automated builds look for tags like this and
/// consider their presence to indicate test failures, even if the higher level
/// test framework code thinks the test passed.
///
/// A kernel OOPS indicates the presence of a bug, however, the bug may not be
/// in the kernel itself.
///
/// # Examples
/// ```rust
/// kernel_oops!("value {} is out of range", val);
/// ```
#[macro_export]
macro_rules! kernel_oops {
    ($fmt:literal, $($arg:tt)*) => {
        $crate::__kprint!(concat!("\nZIRCON KERNEL OOPS\n", $fmt), $($arg)*)
    };
}
