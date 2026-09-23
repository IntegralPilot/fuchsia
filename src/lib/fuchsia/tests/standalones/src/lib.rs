// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[doc(hidden)]
pub use log as __log;

#[derive(argh::FromArgs, Debug)]
/// Test component.
pub struct Options {
    #[argh(switch)]
    /// test argument that should always be off
    pub should_be_false: bool,
}

#[macro_export]
macro_rules! assert_logger_registered {
    () => {
        $crate::__log::set_boxed_logger(Box::new($crate::NoOpLogger {})).unwrap_err()
    };
}

#[macro_export]
macro_rules! assert_no_logger_registered {
    () => {
        $crate::__log::set_boxed_logger(Box::new($crate::NoOpLogger {})).unwrap()
    };
}

pub struct NoOpLogger {}

impl log::Log for NoOpLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, _record: &log::Record<'_>) {}
    fn flush(&self) {}
}
