// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use openthread::prelude::*;
use std::ffi::CStr;

pub const fn tracing_level_from(log_level: ot::LogLevel) -> log::Level {
    match log_level {
        ot::LogLevel::None => log::Level::Error,
        ot::LogLevel::Crit => log::Level::Error,
        ot::LogLevel::Warn => log::Level::Warn,
        ot::LogLevel::Note => log::Level::Info,
        ot::LogLevel::Info => log::Level::Info,
        ot::LogLevel::Debg => log::Level::Debug,
    }
}

pub const fn ot_log_level_from(log_level: log::Level) -> ot::LogLevel {
    match log_level {
        log::Level::Error => ot::LogLevel::Crit,
        log::Level::Warn => ot::LogLevel::Warn,
        log::Level::Info => ot::LogLevel::Info,
        log::Level::Debug => ot::LogLevel::Debg,
        log::Level::Trace => ot::LogLevel::Debg,
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatLogLine(
    log_level: otsys::otLogLevel,
    _: otsys::otLogRegion, // otLogRegion is deprecated.
    line: *const ::std::os::raw::c_char,
) {
    let line = unsafe { CStr::from_ptr(line) };
    let tracing_level = tracing_level_from(log_level.into());

    match line.to_str() {
        // Log line isn't valid UTF-8, display it directly.
        Ok(line) => match tracing_level {
            log::Level::Error => log::error!(tag = "ot"; "{line}"),
            log::Level::Warn => log::warn!(tag = "ot"; "{line}"),
            log::Level::Info => log::info!(tag = "ot"; "{line}"),
            log::Level::Debug => log::debug!(tag = "ot"; "{line}"),
            log::Level::Trace => log::trace!(tag = "ot"; "{line}"),
        },

        // Log line isn't valid UTF-8, try rendering with escaping.
        Err(_) => match tracing_level {
            log::Level::Error => log::error!(tag = "ot"; "{line:?}"),
            log::Level::Warn => log::warn!(tag = "ot"; "{line:?}"),
            log::Level::Info => log::info!(tag = "ot"; "{line:?}"),
            log::Level::Debug => log::debug!(tag = "ot"; "{line:?}"),
            log::Level::Trace => log::trace!(tag = "ot"; "{line:?}"),
        },
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatLogHandleLevelChanged(log_level: otsys::otLogLevel) {
    let tracing_level = tracing_level_from(log_level.into());

    log::info!(
        tag = "openthread_fuchsia";
        "otPlatLogHandleLevelChanged: {log_level} (Tracing: {tracing_level})"
    );
}
