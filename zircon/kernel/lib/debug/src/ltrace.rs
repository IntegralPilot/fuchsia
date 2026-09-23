// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Local trace (`LOCAL_TRACE`) logging mechanism for the Zircon kernel.
//!
//! # Overview
//!
//! This crate provides a compile-time-guarded debug tracing and logging
//! mechanism (`LOCAL_TRACE`, `ltrace!`, `ltracef!`, etc.) for Zircon kernel
//! development. It allows developers to maintain detailed, file-scoped debug
//! logging inside source files with zero overhead in production builds, while
//! enabling targeted high-verbosity logs when developing or debugging.
//!
//! # Defining Local Trace Verbosity
//!
//! Unlike unconditional trace macros (`trace!`, `tracef!`, etc.), the `ltrace*`
//! family of macros requires a `u32` constant named `LOCAL_TRACE` to be defined
//! in the caller's scope.
//!
//! To use local tracing in a Rust file or module (`foo.rs`), define a
//! file-scoped or module-scoped `u32` constant at the top of the file:
//!
//! ```rust
//! // Disable local tracing by default for this file/module:
//! const LOCAL_TRACE: u32 = 0;
//! ```
//!
//! # Enabling Trace Output Locally During Debugging
//!
//! To locally enable verbose trace logs in a specific file while developing
//! or debugging, edit the local `const` definition:
//!
//! ```rust
//! const LOCAL_TRACE: u32 = 1; // Or a higher verbosity level like 2
//! ```
//!
//! Because Rust lexical scoping prefers constants defined in the current module
//! over those in outer modules, crates or parent modules can define a default
//! fallback `const LOCAL_TRACE: u32 = 0;` at their root, and any specific submodule
//! or file can override that fallback locally by defining its own `LOCAL_TRACE`.
//!
//! # Zero Runtime Overhead When Disabled
//!
//! When `LOCAL_TRACE` is `0` at compile-time, trace macros expand to
//! `if false { ... }`. The Rust compiler (`rustc`/LLVM) type-checks and
//! syntax-checks the arguments inside the macro block, but eliminates the dead
//! branch during compilation. Therefore, disabled trace statements incur zero
//! runtime CPU cost and generate zero string data in `.rodata`.
//!
//! # Porting from C++ (`trace.h`)
//!
//! When porting C++ kernel code that uses `zircon/kernel/include/trace.h`
//! (`#define LOCAL_TRACE 0`, `LTRACE_ENTRY`, `LTRACEF`, etc.), trace logging
//! statements should be preserved using this crate.
//!
//! The table below maps C++ `trace.h` macros to their Rust `ltrace` equivalents:
//!
//! * `TRACE_ENTRY` -> [`trace_entry!`]
//! * `TRACE_EXIT` -> [`trace_exit!`]
//! * `TRACE_ENTRY_OBJ` -> [`trace_entry_obj!`]
//! * `TRACE_EXIT_OBJ` -> [`trace_exit_obj!`]
//! * `TRACE` -> [`trace!`]
//! * `TRACEF(str, x...)` -> [`tracef!`]
//! * `LTRACE_ENTRY` -> [`ltrace_entry!`]
//! * `LTRACE_EXIT` -> [`ltrace_exit!`]
//! * `LTRACE_ENTRY_OBJ` -> [`ltrace_entry_obj!`]
//! * `LTRACE_EXIT_OBJ` -> [`ltrace_exit_obj!`]
//! * `LTRACE` -> [`ltrace!`]
//! * `LTRACEF(x...)` -> [`ltracef!`]
//! * `LTRACEF_LEVEL(lvl, x...)` -> [`ltracef_level!`]

#[doc(hidden)]
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
pub fn print_trace_args(module_path: &str, line: u32, args: core::fmt::Arguments<'_>) {
    use core::fmt::Write;
    let mut writer = KernelConsoleWriter;
    let _ = write!(writer, "{}:{}: ", module_path, line);
    let _ = writer.write_fmt(args);
}

#[doc(hidden)]
#[inline(always)]
pub fn print_trace_location(module_path: &str, line: u32) {
    use core::fmt::Write;
    let mut writer = KernelConsoleWriter;
    let _ = write!(writer, "{}:{}\n", module_path, line);
}

#[doc(hidden)]
#[inline(always)]
pub fn print_trace_action(module_path: &str, line: u32, action: &str) {
    use core::fmt::Write;
    let mut writer = KernelConsoleWriter;
    let _ = write!(writer, "{}:{}: {}\n", module_path, line, action);
}

#[doc(hidden)]
#[inline(always)]
pub fn print_trace_action_named(name: &str, action: &str) {
    use core::fmt::Write;
    let mut writer = KernelConsoleWriter;
    let _ = write!(writer, "{}: {}\n", name, action);
}

#[doc(hidden)]
#[inline(always)]
pub fn print_trace_obj<T: ?Sized>(module_path: &str, line: u32, obj: &T, action: &str) {
    use core::fmt::Write;
    let ptr = obj as *const T as *const ();
    let mut writer = KernelConsoleWriter;
    let _ = write!(writer, "{}:{}: {} {:p}\n", module_path, line, action, ptr);
}

#[doc(hidden)]
#[inline(always)]
pub fn print_trace_obj_named<T: ?Sized>(name: &str, obj: &T, action: &str) {
    use core::fmt::Write;
    let ptr = obj as *const T as *const ();
    let mut writer = KernelConsoleWriter;
    let _ = write!(writer, "{}: {} {:p}\n", name, action, ptr);
}

/// Prints function/module entry information (`{module}:{line}: entry` or `{name}: entry`).
///
/// Equivalent to C++ `TRACE_ENTRY`.
///
/// # Examples
/// ```rust
/// trace_entry!();           // Prints "<module>:<line>: entry"
/// trace_entry!("Foo::bar"); // Prints "Foo::bar: entry"
/// ```
#[macro_export]
macro_rules! trace_entry {
    () => {
        $crate::ltrace::print_trace_action(core::module_path!(), core::line!(), "entry")
    };
    ($name:expr) => {
        $crate::ltrace::print_trace_action_named($name, "entry")
    };
}

/// Prints function/module exit information (`{module}:{line}: exit` or `{name}: exit`).
///
/// Equivalent to C++ `TRACE_EXIT`.
#[macro_export]
macro_rules! trace_exit {
    () => {
        $crate::ltrace::print_trace_action(core::module_path!(), core::line!(), "exit")
    };
    ($name:expr) => {
        $crate::ltrace::print_trace_action_named($name, "exit")
    };
}

/// Prints function entry along with the object pointer (`self` or reference).
///
/// Equivalent to C++ `TRACE_ENTRY_OBJ`.
///
/// # Examples
/// ```rust
/// trace_entry_obj!(self);             // Prints "<module>:<line>: entry obj <ptr>"
/// trace_entry_obj!("Foo::bar", self); // Prints "Foo::bar: entry obj <ptr>"
/// ```
#[macro_export]
macro_rules! trace_entry_obj {
    ($obj:expr) => {
        $crate::ltrace::print_trace_obj(core::module_path!(), core::line!(), $obj, "entry obj")
    };
    ($name:expr, $obj:expr) => {
        $crate::ltrace::print_trace_obj_named($name, $obj, "entry obj")
    };
}

/// Prints function exit along with the object pointer (`self` or reference).
///
/// Equivalent to C++ `TRACE_EXIT_OBJ`.
#[macro_export]
macro_rules! trace_exit_obj {
    ($obj:expr) => {
        $crate::ltrace::print_trace_obj(core::module_path!(), core::line!(), $obj, "exit obj")
    };
    ($name:expr, $obj:expr) => {
        $crate::ltrace::print_trace_obj_named($name, $obj, "exit obj")
    };
}

/// Prints the current module path and line (`{module}:{line}`).
///
/// Equivalent to C++ `TRACE`.
#[macro_export]
macro_rules! trace {
    () => {
        $crate::ltrace::print_trace_location(core::module_path!(), core::line!())
    };
}

/// Formats and prints a trace message prefixed with module path and line number.
///
/// Equivalent to C++ `TRACEF`.
///
/// # Examples
/// ```rust
/// tracef!("initialized with value {}\n", val);
/// ```
#[macro_export]
macro_rules! tracef {
    ($($arg:tt)*) => {
        $crate::ltrace::print_trace_args(core::module_path!(), core::line!(), core::format_args!($($arg)*))
    };
}

/// Prints function entry (`TRACE_ENTRY`) if `LOCAL_TRACE >= 1`.
///
/// Equivalent to C++ `LTRACE_ENTRY`.
#[macro_export]
macro_rules! ltrace_entry {
    ($($arg:tt)*) => {
        if LOCAL_TRACE >= 1u32 {
            $crate::trace_entry!($($arg)*);
        }
    };
}

/// Prints function exit (`TRACE_EXIT`) if `LOCAL_TRACE >= 1`.
///
/// Equivalent to C++ `LTRACE_EXIT`.
#[macro_export]
macro_rules! ltrace_exit {
    ($($arg:tt)*) => {
        if LOCAL_TRACE >= 1u32 {
            $crate::trace_exit!($($arg)*);
        }
    };
}

/// Prints function entry with object pointer (`TRACE_ENTRY_OBJ`) if `LOCAL_TRACE >= 1`.
///
/// Equivalent to C++ `LTRACE_ENTRY_OBJ`.
#[macro_export]
macro_rules! ltrace_entry_obj {
    ($($arg:tt)*) => {
        if LOCAL_TRACE >= 1u32 {
            $crate::trace_entry_obj!($($arg)*);
        }
    };
}

/// Prints function exit with object pointer (`TRACE_EXIT_OBJ`) if `LOCAL_TRACE >= 1`.
///
/// Equivalent to C++ `LTRACE_EXIT_OBJ`.
#[macro_export]
macro_rules! ltrace_exit_obj {
    ($($arg:tt)*) => {
        if LOCAL_TRACE >= 1u32 {
            $crate::trace_exit_obj!($($arg)*);
        }
    };
}

/// Prints current module path and line (`TRACE`) if `LOCAL_TRACE >= 1`.
///
/// Equivalent to C++ `LTRACE`.
#[macro_export]
macro_rules! ltrace {
    () => {
        if LOCAL_TRACE >= 1u32 {
            $crate::trace!();
        }
    };
    ($($stmt:tt)+) => {
        if LOCAL_TRACE >= 1u32 {
            $($stmt)+
        }
    };
}

/// Formats and prints a trace message (`TRACEF`) if `LOCAL_TRACE >= 1`.
///
/// Equivalent to C++ `LTRACEF`.
///
/// # Examples
/// ```rust
/// const LOCAL_TRACE: u32 = 0; // Edit to 1 when locally debugging this file
///
/// fn my_func(x: i32) {
///     ltracef!("my_func called with x = {}\n", x);
/// }
/// ```
#[macro_export]
macro_rules! ltracef {
    ($($arg:tt)*) => {
        if LOCAL_TRACE >= 1u32 {
            $crate::tracef!($($arg)*);
        }
    };
}

/// Prints current module path and line (`TRACE`) if `LOCAL_TRACE >= level`.
#[macro_export]
macro_rules! ltrace_level {
    ($level:expr) => {
        if LOCAL_TRACE >= $level {
            $crate::trace!();
        }
    };
}

/// Formats and prints a trace message (`TRACEF`) if `LOCAL_TRACE >= level`.
///
/// Equivalent to C++ `LTRACEF_LEVEL`.
///
/// # Examples
/// ```rust
/// const LOCAL_TRACE: u32 = 1;
///
/// // Printed when LOCAL_TRACE >= 1:
/// ltracef_level!(1, "basic info: {}\n", val);
/// // Optimized out unless LOCAL_TRACE >= 2:
/// ltracef_level!(2, "detailed info: {}\n", val);
/// ```
#[macro_export]
macro_rules! ltracef_level {
    ($level:expr, $($arg:tt)*) => {
        if LOCAL_TRACE >= $level {
            $crate::tracef!($($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_unconditional_macros() {
        trace!();
        trace_entry!();
        trace_entry!("Test::foo");
        trace_exit!();
        trace_exit!("Test::foo");
        let obj = 42;
        trace_entry_obj!(&obj);
        trace_entry_obj!("Test::foo", &obj);
        trace_exit_obj!(&obj);
        trace_exit_obj!("Test::foo", &obj);
        tracef!("hello {}\n", 123);
    }

    #[test]
    fn test_disabled_local_trace() {
        const LOCAL_TRACE: u32 = 0;
        ltrace!();
        ltrace_entry!();
        ltrace_entry!("Test::disabled");
        ltrace_exit!();
        ltrace_exit!("Test::disabled");
        let obj = 42;
        ltrace_entry_obj!(&obj);
        ltrace_exit_obj!(&obj);
        ltracef!("should not print: {}\n", 123);
        ltracef_level!(1, "should not print: {}\n", 456);
        let mut executed = false;
        ltrace!(executed = true);
        assert!(!executed);
    }

    #[test]
    fn test_enabled_local_trace() {
        const LOCAL_TRACE: u32 = 2;
        ltrace!();
        ltrace_entry!();
        ltrace_entry!("Test::enabled");
        ltrace_exit!();
        ltrace_exit!("Test::enabled");
        let obj = 42;
        ltrace_entry_obj!(&obj);
        ltrace_exit_obj!(&obj);
        ltracef!("enabled level 1: {}\n", 123);
        ltracef_level!(1, "enabled level 1: {}\n", 123);
        ltracef_level!(2, "enabled level 2: {}\n", 456);
        ltracef_level!(3, "should not print level 3: {}\n", 789);
        let mut executed = false;
        ltrace!(executed = true);
        assert!(executed);
    }
}
