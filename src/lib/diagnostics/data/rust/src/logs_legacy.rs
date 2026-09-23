// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Data, Logs};
use fidl_fuchsia_logger::LogMessage;

use std::collections::HashSet;
use std::fmt::Write;

/// Convert this `Message` to a FIDL representation suitable for sending to `LogListenerSafe`.
impl From<&Data<Logs>> for LogMessage {
    fn from(data: &Data<Logs>) -> LogMessage {
        let mut msg = data.msg().unwrap_or("").to_string();

        if let Some(payload) = data.payload_keys() {
            for property in payload.properties.iter() {
                write!(&mut msg, " {property}").expect("allocations have to fail for this to fail");
            }
        }
        let file = data.metadata.file.as_ref();
        let line = data.metadata.line.as_ref();
        if let (Some(file), Some(line)) = (file, line) {
            msg = format!("[{file}({line})] {msg}");
        }

        let tags = match &data.metadata.tags {
            None => vec![data.component_name().to_string()],
            Some(tags) if tags.is_empty() => vec![data.component_name().to_string()],
            Some(tags) => tags.clone(),
        };

        LogMessage {
            pid: data.pid().unwrap_or(zx::sys::ZX_KOID_INVALID),
            tid: data.tid().unwrap_or(zx::sys::ZX_KOID_INVALID),
            time: data.metadata.timestamp,
            severity: data.metadata.raw_severity() as i32,
            dropped_logs: data.dropped_logs().unwrap_or(0) as _,
            tags,
            msg,
        }
    }
}

/// Applies legacy formatting to a log message, matching the formatting
/// used for a legacy LogMessage.
/// Prefer using LogTextPresenter if possible instead of this function.
pub fn format_log_message(data: &Data<Logs>) -> String {
    let mut msg = data.msg().unwrap_or("").to_string();

    if let Some(payload) = data.payload_keys() {
        for property in payload.properties.iter() {
            write!(&mut msg, " {property}").expect("allocations have to fail for this to fail");
        }
    }
    let file = data.metadata.file.as_ref();
    let line = data.metadata.line.as_ref();
    if let (Some(file), Some(line)) = (file, line) {
        msg = format!("[{file}({line})] {msg}");
    }
    msg
}

// Rust uses tags of the form "<foo>::<bar>" so if we have a filter for "<foo>" we should
// include messages that have "<foo>" as a prefix.
fn include_tag_prefix(tag: &str, tags: &HashSet<String>) -> bool {
    if tag.contains("::") {
        tags.iter().any(|t| {
            tag.strip_prefix(t.as_str())
                .map(|rest| rest.starts_with("::") && rest.len() > 2)
                .unwrap_or(false)
        })
    } else {
        false
    }
}

/// Filters by tags according to legacy LogMessage rules.
/// Prefer filtering by moniker/selectors instead of using this function.
pub fn filter_by_tags(log_message: &Data<Logs>, include_tags: &HashSet<String>) -> bool {
    if include_tags.is_empty() {
        false
    } else if log_message.tags().map(|t| t.is_empty()).unwrap_or(true) {
        !include_tags.contains(log_message.component_name().as_ref())
    } else {
        !log_message
            .tags()
            .map(|tags| {
                tags.iter()
                    .any(|tag| include_tags.contains(tag) || include_tag_prefix(tag, include_tags))
            })
            .unwrap_or(false)
    }
}

/// Convert this `Message` to a FIDL representation suitable for sending to `LogListenerSafe`.
impl From<Data<Logs>> for LogMessage {
    fn from(data: Data<Logs>) -> LogMessage {
        LogMessage {
            pid: data.pid().unwrap_or(zx::sys::ZX_KOID_INVALID),
            tid: data.tid().unwrap_or(zx::sys::ZX_KOID_INVALID),
            time: data.metadata.timestamp,
            severity: data.metadata.raw_severity() as i32,
            dropped_logs: data.dropped_logs().unwrap_or(0) as _,
            msg: format_log_message(&data),
            tags: match data.metadata.tags {
                Some(tags) => tags,
                None => vec![data.component_name().to_string()],
            },
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{BuilderArgs, LogsDataBuilder, Severity, Timestamp};
    use fidl_fuchsia_diagnostics_types as fdiagnostics_types;

    const TEST_URL: &str = "fuchsia-pkg://test";
    const TEST_MONIKER: &str = "fake-test/moniker";

    macro_rules! severity_roundtrip_test {
        ($raw:expr, $expected:expr) => {
            let severity = Severity::from(u8::from($raw));
            let msg = LogsDataBuilder::new(BuilderArgs {
                timestamp: Timestamp::from_nanos(0),
                component_url: Some(TEST_URL.into()),
                moniker: moniker::ExtendedMoniker::parse_str(TEST_MONIKER).unwrap(),
                severity,
            })
            .build();

            let legacy_msg: LogMessage = (&msg).into();
            assert_eq!(
                legacy_msg.severity,
                i32::from($expected),
                "failed to round trip severity for {:?} (raw {}), intermediates: {:#?}\n{:#?}",
                severity,
                $raw,
                msg,
                legacy_msg
            );
        };
    }

    #[fuchsia::test]
    fn raw_severity_roundtrip_trace() {
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Trace.into_primitive() - 1,
            fdiagnostics_types::Severity::Trace.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Trace.into_primitive(),
            fdiagnostics_types::Severity::Trace.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Trace.into_primitive() + 1,
            fdiagnostics_types::Severity::Debug.into_primitive()
        );
    }

    #[fuchsia::test]
    fn severity_roundtrip_debug() {
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Debug.into_primitive() - 1,
            fdiagnostics_types::Severity::Debug.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Debug.into_primitive(),
            fdiagnostics_types::Severity::Debug.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Debug.into_primitive() + 1,
            fdiagnostics_types::Severity::Info.into_primitive()
        );
    }

    #[fuchsia::test]
    fn severity_roundtrip_info() {
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Info.into_primitive() - 1,
            fdiagnostics_types::Severity::Info.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Info.into_primitive(),
            fdiagnostics_types::Severity::Info.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Info.into_primitive() + 1,
            fdiagnostics_types::Severity::Warn.into_primitive()
        );
    }

    #[fuchsia::test]
    fn severity_roundtrip_warn() {
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Warn.into_primitive() - 1,
            fdiagnostics_types::Severity::Warn.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Warn.into_primitive(),
            fdiagnostics_types::Severity::Warn.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Warn.into_primitive() + 1,
            fdiagnostics_types::Severity::Error.into_primitive()
        );
    }

    #[fuchsia::test]
    fn severity_roundtrip_error() {
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Error.into_primitive() - 1,
            fdiagnostics_types::Severity::Error.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Error.into_primitive(),
            fdiagnostics_types::Severity::Error.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Error.into_primitive() + 1,
            fdiagnostics_types::Severity::Fatal.into_primitive()
        );
    }

    #[fuchsia::test]
    fn severity_roundtrip_fatal() {
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Fatal.into_primitive() - 1,
            fdiagnostics_types::Severity::Fatal.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Fatal.into_primitive(),
            fdiagnostics_types::Severity::Fatal.into_primitive()
        );
        severity_roundtrip_test!(
            fdiagnostics_types::Severity::Fatal.into_primitive() + 1,
            fdiagnostics_types::Severity::Fatal.into_primitive()
        );
    }

    #[fuchsia::test]
    fn test_include_tag_prefix_unicode() {
        let mut tags = HashSet::new();
        tags.insert("foo".to_string());
        tags.insert("🦀".to_string());

        assert!(!include_tag_prefix("🦀::bar", &["foo".to_string()].into_iter().collect()));
        assert!(include_tag_prefix("🦀::bar", &tags));
        assert!(!include_tag_prefix("🦀::", &tags));
        assert!(include_tag_prefix("foo::bar", &tags));
    }
}
