// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helpers for logging.
//!
//! Logging using the macros exported from this module will always include
//! the [`NETLINK_LOG_TAG`] tag.
//!
//! [`NETLINK_LOG_TAG`]: crate::NETLINK_LOG_TAG

/// Emits a log at the specified level.
///
/// This macro should not be used directly and the other `log_*` macros should
/// be used instead.
macro_rules! __log_inner {
    (level = $level:ident, $($arg:tt)*) => {
        log::$level!(tag = $crate::NETLINK_LOG_TAG; $($arg)*)
    }
}

/// Emits a debug log.
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::logging::__log_inner!(level = debug, $($arg)*)
    }
}

/// Emits an info log.
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::logging::__log_inner!(level = info, $($arg)*)
    }
}

/// Emits a warning log.
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::logging::__log_inner!(level = warn, $($arg)*)
    }
}

/// Emits an error log.
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::logging::__log_inner!(level = error, $($arg)*)
    }
}

// Re-exporting macros allows them to be used like regular rust items.
//
// We re-export `__log_inner` so that invocation sites of the `log_*` macros can
// access `__log_inner` as it is invoked by the `log_*` implementations. See
// https://doc.rust-lang.org/reference/macros-by-example.html#hygiene for more
// details.
pub(crate) use __log_inner;
pub(crate) use log_debug;
pub(crate) use log_error;
pub(crate) use log_info;
pub(crate) use log_warn;

#[cfg(test)]
pub(crate) mod testutils {
    use std::sync::atomic::AtomicBool;

    /// Install a logger for tests.
    ///
    /// Call this method at the beginning of the test for which logging is desired.
    /// This function sets global program state, so all tests that run after this
    /// function is called will use the logger.
    pub(crate) fn set_logger_for_test() {
        struct Logger;

        impl log::Log for Logger {
            fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
                true
            }

            fn log(&self, record: &log::Record<'_>) {
                println!("[{}] ({}) {}", record.level(), record.target(), record.args())
            }

            fn flush(&self) {}
        }

        static LOGGER_ONCE: AtomicBool = AtomicBool::new(true);

        // log::set_logger will panic if called multiple times.
        if LOGGER_ONCE.swap(false, std::sync::atomic::Ordering::AcqRel) {
            let _ = log::set_logger(&Logger);
            log::set_max_level(log::LevelFilter::Trace);
        }
    }
}
