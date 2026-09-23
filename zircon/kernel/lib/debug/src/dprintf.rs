// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Debug logging (`dprintf`) mechanism for the Zircon kernel.
//!
//! # Overview
//!
//! This crate provides a Rust implementation of the global debug printing mechanism
//! (`dprintf!`) used in the Zircon kernel. It is the Rust counterpart to the C++
//! header `zircon/kernel/include/debug.h`.
//!
//! # Verbosity Levels
//!
//! The macro `dprintf!` filters messages at compile time based on the global
//! GN build argument `kernel_debug_print_level`.
//!
//! This argument defaults to `2` (`SPEW`) in the kernel parameters (see
//! `zircon/kernel/params.gni`).
//!
//! To override this level in your local build, add the following to your `args.gn`:
//!
//! ```gn
//! kernel_debug_print_level = 1  # Restrict to INFO and CRITICAL
//! ```
//!
//! The following levels are defined, matching C++:
//! * `CRITICAL` (0) / `ALWAYS` (0): Critical errors or messages that should always be printed.
//! * `INFO` (1): Informational messages.
//! * `SPEW` (2): Verbose debugging messages.
//!
//! If a message's level is greater than the configured `kernel_debug_print_level`,
//! the macro call compiles down to nothing, incurring zero runtime overhead.
//!
//! # Usage
//!
//! To use global debug printing in a Rust file:
//!
//! ```rust
//! use debug::dprintf;
//!
//! fn my_function() {
//!     dprintf!(INFO, "This is an info message: {}\n", 42);
//!     dprintf!(SPEW, "This is verbose spew\n");
//! }
//! ```
//!
//! The macro supports literal tokens `CRITICAL`, `ALWAYS`, `INFO`, and `SPEW`
//! without needing to import them, but you can also use arbitrary expressions
//! that evaluate to `u32` (e.g. `debug::dprintf::INFO` or a local variable).

pub const CRITICAL: u32 = 0;
pub const ALWAYS: u32 = 0;
pub const INFO: u32 = 1;
pub const SPEW: u32 = 2;

pub struct KernelConsoleWriter;

impl core::fmt::Write for KernelConsoleWriter {
    #[cfg(not(test))]
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        kprint::kprint!("{:s}", s);
        Ok(())
    }

    #[cfg(test)]
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        std::print!("{}", s);
        Ok(())
    }
}

#[doc(hidden)]
#[inline(always)]
pub const fn dprintf_enabled(level: u32) -> bool {
    let limit = if cfg!(debug_print_level = "0") {
        CRITICAL
    } else if cfg!(debug_print_level = "1") {
        INFO
    } else if cfg!(debug_print_level = "2") {
        SPEW
    } else {
        SPEW
    };
    level <= limit
}

#[doc(hidden)]
#[inline(always)]
pub fn print_dprintf_args(args: core::fmt::Arguments<'_>) {
    use core::fmt::Write;
    let mut writer = KernelConsoleWriter;
    let _ = writer.write_fmt(args);
}

/// Formats and prints a debug message if the global debug print level is
/// greater than or equal to the specified level.
///
/// Equivalent to C++ `dprintf`.
///
/// # Examples
/// ```rust
/// dprintf!(INFO, "initialized with value {}\n", val);
/// ```
#[macro_export]
macro_rules! dprintf {
    (CRITICAL, $($arg:tt)*) => { $crate::dprintf!($crate::dprintf::CRITICAL, $($arg)*) };
    (ALWAYS, $($arg:tt)*) => { $crate::dprintf!($crate::dprintf::ALWAYS, $($arg)*) };
    (INFO, $($arg:tt)*) => { $crate::dprintf!($crate::dprintf::INFO, $($arg)*) };
    (SPEW, $($arg:tt)*) => { $crate::dprintf!($crate::dprintf::SPEW, $($arg)*) };
    ($level:expr, $($arg:tt)*) => {
        if $crate::dprintf::dprintf_enabled($level) {
            $crate::dprintf::print_dprintf_args(core::format_args!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_dprintf() {
        dprintf!(CRITICAL, "critical\n");
        dprintf!(ALWAYS, "always\n");
        dprintf!(INFO, "info\n");
        dprintf!(SPEW, "spew\n");
        dprintf!(3, "should not print\n");
        dprintf!(INFO, "info const\n");
    }
}
