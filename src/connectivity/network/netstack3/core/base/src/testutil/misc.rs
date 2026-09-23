// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Miscellaneous test utilities used in netstack3.
//!
//! # Note to developers
//!
//! Please refrain from adding types to this module, keep this only to
//! freestanding functions. If you require a new type, create a module for it.

// The crate is `no_std`, but test utilities are allowed to print to stdout.
extern crate std;

use alloc::vec::Vec;
use core::fmt::Debug;
use core::sync::atomic::{self, AtomicBool};

/// Install a logger for tests.
///
/// Call this method at the beginning of the test for which logging is desired.
/// This function sets global program state, so all tests that run after this
/// function is called will use the logger.
pub fn set_logger_for_test() {
    struct Logger;

    impl log::Log for Logger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            std::println!("[{}] ({}) {}", record.level(), record.target(), record.args())
        }

        fn flush(&self) {}
    }

    static LOGGER_ONCE: AtomicBool = AtomicBool::new(true);

    // log::set_logger will panic if called multiple times.
    if LOGGER_ONCE.swap(false, atomic::Ordering::AcqRel) {
        log::set_logger(&Logger).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    }
}

/// Asserts that an iterable object produces zero items.
///
/// `assert_empty` drains `into_iter.into_iter()` and asserts that zero
/// items are produced. It panics with a message which includes the produced
/// items if this assertion fails.
#[track_caller]
pub fn assert_empty<I: IntoIterator>(into_iter: I)
where
    I::Item: Debug,
{
    // NOTE: Collecting into a `Vec` is cheap in the happy path because
    // zero-capacity vectors are guaranteed not to allocate.
    let vec = into_iter.into_iter().collect::<Vec<_>>();
    assert!(vec.is_empty(), "vec={vec:?}");
}
