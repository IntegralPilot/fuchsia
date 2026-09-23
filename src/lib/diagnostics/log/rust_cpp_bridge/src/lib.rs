// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Wrapper around the C++ syslog library for Fuchsia that allows initializing it with the same
//! initial settings as the Rust logging library.

/// Initialize the bridge, configuring the C++ logging backend with the same settings as those for
/// Rust at the time of this call.
pub fn init() {
    let cpp_severity = if log::log_enabled!(log::Level::Trace) {
        FUCHSIA_LOG_TRACE
    } else if log::log_enabled!(log::Level::Debug) {
        FUCHSIA_LOG_DEBUG
    } else if log::log_enabled!(log::Level::Info) {
        FUCHSIA_LOG_INFO
    } else if log::log_enabled!(log::Level::Warn) {
        FUCHSIA_LOG_WARNING
    } else if log::log_enabled!(log::Level::Error) {
        FUCHSIA_LOG_ERROR
    } else {
        FUCHSIA_LOG_FATAL
    };

    // SAFETY: basic FFI call with no invariants
    unsafe {
        init_cpp_logging(cpp_severity);
    }
}

pub fn init_with_log_severity(min_severity: u8) {
    unsafe {
        init_cpp_logging(min_severity);
    }
}

// Mirrored from //sdk/lib/syslog/structured_backend/fuchsia_syslog.h
pub const FUCHSIA_LOG_TRACE: u8 = 0x10;
pub const FUCHSIA_LOG_DEBUG: u8 = 0x20;
pub const FUCHSIA_LOG_INFO: u8 = 0x30;
pub const FUCHSIA_LOG_WARNING: u8 = 0x40;
pub const FUCHSIA_LOG_ERROR: u8 = 0x50;
pub const FUCHSIA_LOG_FATAL: u8 = 0x60;

unsafe extern "C" {
    fn init_cpp_logging(min_severity: u8);
}

#[cfg(test)]
mod tests {
    use crate::FUCHSIA_LOG_TRACE;
    use diagnostics_reader::{ArchiveReader, Severity};
    use futures::StreamExt;

    unsafe extern "C" {
        fn emit_trace_log_for_testing();
        fn emit_debug_log_for_testing();
        fn emit_info_log_for_testing();
        fn emit_warning_log_for_testing();
        fn emit_error_log_for_testing();
    }

    #[fuchsia::test(logging_minimum_severity = "TRACE")]
    async fn cpp_trace_log_appears_after_init() {
        super::init();
        let mut logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap();

        // SAFETY: basic FFI call with no invariants
        unsafe { emit_trace_log_for_testing() };

        let message = loop {
            let message = logs.next().await.unwrap().unwrap();
            if message.msg().unwrap() == "TRACE TEST MESSAGE FROM C++"
                && message.severity() == Severity::Trace
            {
                break message;
            }
        };
        assert_eq!(message.moniker.to_string(), ".", "messages must come from this component");
        let file_path = message.file_path().unwrap();
        assert!(file_path.ends_with(".cc"), "messages must come from C++, got {file_path}");
    }

    #[fuchsia::test]
    async fn cpp_log_appears_after_init_with_log_severity_trace() {
        super::init_with_log_severity(FUCHSIA_LOG_TRACE);
        let mut logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap();

        // SAFETY: basic FFI call with no invariants
        unsafe { emit_trace_log_for_testing() };

        let message = loop {
            let message = logs.next().await.unwrap().unwrap();
            if message.msg().unwrap() == "TRACE TEST MESSAGE FROM C++"
                && message.severity() == Severity::Trace
            {
                break message;
            }
        };
        assert_eq!(message.moniker.to_string(), ".", "messages must come from this component");
        let file_path = message.file_path().unwrap();
        assert!(file_path.ends_with(".cc"), "messages must come from C++, got {file_path}");
    }

    #[fuchsia::test(logging_minimum_severity = "DEBUG")]
    async fn cpp_debug_log_appears_after_init() {
        super::init();
        let mut logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap();

        // SAFETY: basic FFI call with no invariants
        unsafe { emit_debug_log_for_testing() };

        let message = loop {
            let message = logs.next().await.unwrap().unwrap();
            if message.msg().unwrap() == "DEBUG TEST MESSAGE FROM C++"
                && message.severity() == Severity::Debug
            {
                break message;
            }
        };
        assert_eq!(message.moniker.to_string(), ".", "messages must come from this component");
        let file_path = message.file_path().unwrap();
        assert!(file_path.ends_with(".cc"), "messages must come from C++, got {file_path}");
    }

    #[fuchsia::test(logging_minimum_severity = "INFO")]
    async fn cpp_info_log_appears_after_init() {
        super::init();
        let mut logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap();

        // SAFETY: basic FFI call with no invariants
        unsafe { emit_info_log_for_testing() };

        let message = loop {
            let message = logs.next().await.unwrap().unwrap();
            if message.msg().unwrap() == "INFO TEST MESSAGE FROM C++"
                && message.severity() == Severity::Info
            {
                break message;
            }
        };
        assert_eq!(message.moniker.to_string(), ".", "messages must come from this component");
        let file_path = message.file_path().unwrap();
        assert!(file_path.ends_with(".cc"), "messages must come from C++, got {file_path}");
    }

    #[fuchsia::test(logging_minimum_severity = "WARN")]
    async fn cpp_warn_log_appears_after_init() {
        super::init();
        let mut logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap();

        // SAFETY: basic FFI call with no invariants
        unsafe { emit_warning_log_for_testing() };

        let message = loop {
            let message = logs.next().await.unwrap().unwrap();
            if message.msg().unwrap() == "WARNING TEST MESSAGE FROM C++"
                && message.severity() == Severity::Warn
            {
                break message;
            }
        };
        assert_eq!(message.moniker.to_string(), ".", "messages must come from this component");
        let file_path = message.file_path().unwrap();
        assert!(file_path.ends_with(".cc"), "messages must come from C++, got {file_path}");
    }

    #[fuchsia::test(logging_minimum_severity = "ERROR")]
    async fn cpp_error_log_appears_after_init() {
        super::init();
        let mut logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap();

        // SAFETY: basic FFI call with no invariants
        unsafe { emit_error_log_for_testing() };

        let message = loop {
            let message = logs.next().await.unwrap().unwrap();
            if message.msg().unwrap() == "ERROR TEST MESSAGE FROM C++"
                && message.severity() == Severity::Error
            {
                break message;
            }
        };
        assert_eq!(message.moniker.to_string(), ".", "messages must come from this component");
        let file_path = message.file_path().unwrap();
        assert!(file_path.ends_with(".cc"), "messages must come from C++, got {file_path}");
    }
}
