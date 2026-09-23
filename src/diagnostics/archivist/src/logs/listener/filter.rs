// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use super::ListenerError;
use diagnostics_data::logs_legacy::filter_by_tags;
use diagnostics_data::{LogsData, Severity};
use fidl_fuchsia_logger::{LogFilterOptions, LogLevelFilter};
use std::collections::HashSet;

/// Controls whether messages are seen by a given `Listener`. Created from
/// `fidl_fuchsia_logger::LogFilterOptions`.
#[derive(Default)]
pub(super) struct MessageFilter {
    /// Only send messages of greater or equal severity to this value.
    min_severity: Option<Severity>,

    /// Only send messages that purport to come from this PID.
    pid: Option<u64>,

    /// Only send messages that purport to come from this TID.
    tid: Option<u64>,

    /// Only send messages whose tags match one or more of those provided.
    tags: HashSet<String>,
}

impl MessageFilter {
    /// Constructs a new `MessageFilter` from the filter options provided to the methods
    /// `fuchsia.logger.Log.{Listen,DumpLogs}`.
    pub fn new(options: Option<Box<LogFilterOptions>>) -> Result<Self, ListenerError> {
        let mut this = Self::default();

        if let Some(mut options) = options {
            this.tags = options.tags.drain(..).collect();

            let count = this.tags.len();
            if count > fidl_fuchsia_logger::MAX_TAGS as usize {
                return Err(ListenerError::TooManyTags { count });
            }

            for (index, tag) in this.tags.iter().enumerate() {
                if tag.len() > fidl_fuchsia_logger::MAX_TAG_LEN_BYTES as usize {
                    return Err(ListenerError::TagTooLong { index });
                }
            }

            if options.filter_by_pid {
                this.pid = Some(options.pid)
            }
            if options.filter_by_tid {
                this.tid = Some(options.tid)
            }

            // TODO(b/322552971): Rename to raw_severity with LSC process.
            // This is defined in FIDL and will be renamed in its own change.
            if options.verbosity > 0 {
                // verbosity scale sits in the interstitial space between
                // INFO and DEBUG
                let raw_level: i32 = std::cmp::max(
                    fidl_fuchsia_logger::LogLevelFilter::Debug as i32 + 1,
                    fidl_fuchsia_logger::LogLevelFilter::Info as i32
                        - (options.verbosity as i32
                            * fidl_fuchsia_logger::LOG_VERBOSITY_STEP_SIZE as i32),
                );
                this.min_severity = Some(Severity::from(raw_level));
            } else if options.min_severity != LogLevelFilter::None {
                this.min_severity = Some(Severity::from(options.min_severity.into_primitive()));
            }
        }

        Ok(this)
    }

    /// This filter defaults to open, allowing messages through. If multiple portions of the filter
    /// are specified, they are additive, only allowing messages through that pass all criteria.
    pub fn should_send(&self, log_message: &LogsData) -> bool {
        let reject_pid = self.pid.map(|p| log_message.pid() != Some(p)).unwrap_or(false);
        let reject_tid = self.tid.map(|t| log_message.tid() != Some(t)).unwrap_or(false);
        let reject_severity =
            self.min_severity.map(|m| m > log_message.severity()).unwrap_or(false);
        let reject_tags = filter_by_tags(log_message, &self.tags);

        !(reject_pid || reject_tid || reject_severity || reject_tags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ComponentIdentity;
    use diagnostics_data::Severity;

    use moniker::ExtendedMoniker;

    fn test_message_with_tag(tag: Option<&str>) -> LogsData {
        let identity = ComponentIdentity::new(
            ExtendedMoniker::parse_str("./bogus/specious-at-best").unwrap(),
            "fuchsia-pkg://not-a-package",
        );
        let mut builder = diagnostics_data::LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            timestamp: zx::BootInstant::from_nanos(1),
            component_url: Some(identity.url.clone()),
            moniker: identity.moniker.clone(),
            severity: Severity::Info,
        });
        if let Some(tag) = tag {
            builder = builder.add_tag(tag);
        }
        builder.build()
    }

    fn test_message() -> LogsData {
        test_message_with_tag(None)
    }

    #[fuchsia::test]
    fn should_send_verbose() {
        let mut message = test_message();
        let mut filter = MessageFilter::new(Some(Box::new(LogFilterOptions {
            filter_by_pid: false,
            pid: 0,
            filter_by_tid: false,
            tid: 0,
            verbosity: 15,
            min_severity: LogLevelFilter::Info,
            tags: vec![],
        })))
        .unwrap();
        for raw_severity in 1..15 {
            let raw = (Severity::Debug as u8) + raw_severity;
            message.set_raw_severity(raw);
            assert!(
                filter.should_send(&message),
                "got false on should_send with raw_severity={raw} and level filter with verbosity=15"
            );
        }

        filter.min_severity = Some(Severity::Debug);
        message.set_raw_severity((Severity::Debug as u8) + 1);
        assert!(filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_reject_verbose() {
        let mut message = test_message();
        let mut filter = MessageFilter::new(Some(Box::new(LogFilterOptions {
            filter_by_pid: false,
            pid: 0,
            filter_by_tid: false,
            tid: 0,
            verbosity: 1,
            min_severity: LogLevelFilter::Info,
            tags: vec![],
        })))
        .unwrap();

        for raw_severity in 2..15 {
            message.set_raw_severity(raw_severity);
            assert!(!filter.should_send(&message));
        }

        filter.min_severity = Some(Severity::Info);
        message.set_raw_severity(1);
        assert!(!filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_send_info() {
        let mut message = test_message();
        let mut filter =
            MessageFilter { min_severity: Some(Severity::Info), ..MessageFilter::default() };

        message.metadata.severity = Severity::Info;
        assert!(filter.should_send(&message));

        filter.min_severity = Some(Severity::Debug);
        message.metadata.severity = Severity::Info;
        assert!(filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_reject_info() {
        let mut message = test_message();
        let filter =
            MessageFilter { min_severity: Some(Severity::Warn), ..MessageFilter::default() };

        message.metadata.severity = Severity::Info;
        assert!(!filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_send_warn() {
        let mut message = test_message();
        let mut filter =
            MessageFilter { min_severity: Some(Severity::Warn), ..MessageFilter::default() };

        message.metadata.severity = Severity::Warn;
        assert!(filter.should_send(&message));

        filter.min_severity = Some(Severity::Info);
        message.metadata.severity = Severity::Warn;
        assert!(filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_reject_warn() {
        let mut message = test_message();
        let filter =
            MessageFilter { min_severity: Some(Severity::Error), ..MessageFilter::default() };

        message.metadata.severity = Severity::Warn;
        assert!(!filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_send_error() {
        let mut message = test_message();
        let mut filter =
            MessageFilter { min_severity: Some(Severity::Error), ..MessageFilter::default() };

        message.metadata.severity = Severity::Error;
        assert!(filter.should_send(&message));

        filter.min_severity = Some(Severity::Warn);
        message.metadata.severity = Severity::Error;
        assert!(filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_reject_error() {
        let mut message = test_message();
        let filter =
            MessageFilter { min_severity: Some(Severity::Fatal), ..MessageFilter::default() };

        message.metadata.severity = Severity::Error;
        assert!(!filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_send_debug() {
        let mut message = test_message();
        let mut filter =
            MessageFilter { min_severity: Some(Severity::Debug), ..MessageFilter::default() };

        message.metadata.severity = Severity::Debug;
        assert!(filter.should_send(&message));

        filter.min_severity = Some(Severity::Trace);
        message.metadata.severity = Severity::Debug;
        assert!(filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_reject_debug() {
        let mut message = test_message();
        let filter =
            MessageFilter { min_severity: Some(Severity::Info), ..MessageFilter::default() };

        message.metadata.severity = Severity::Debug;
        assert!(!filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_send_trace() {
        let mut message = test_message();
        let filter =
            MessageFilter { min_severity: Some(Severity::Trace), ..MessageFilter::default() };

        message.metadata.severity = Severity::Trace;
        assert!(filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_reject_trace() {
        let mut message = test_message();
        let filter =
            MessageFilter { min_severity: Some(Severity::Debug), ..MessageFilter::default() };

        message.metadata.severity = Severity::Trace;
        assert!(!filter.should_send(&message));
    }

    #[fuchsia::test]
    fn should_send_attributed_tag() {
        let message = test_message();
        let filter = MessageFilter {
            tags: vec!["specious-at-best".to_string()].into_iter().collect(),
            ..MessageFilter::default()
        };

        assert!(filter.should_send(&message), "the filter should have sent {message:#?}");
    }

    #[fuchsia::test]
    fn should_send_prefix_tag() {
        let message = test_message_with_tag(Some("foo::bar::baz"));

        let filter = MessageFilter {
            tags: vec!["foo".to_string()].into_iter().collect(),
            ..MessageFilter::default()
        };

        assert!(filter.should_send(&message), "the filter should have sent {message:#?}");

        let message2 = test_message_with_tag(Some("foobar"));
        assert!(!filter.should_send(&message2), "the filter should not have sent {message2:#?}");

        let filter = MessageFilter {
            tags: vec!["foo::bar".to_string()].into_iter().collect(),
            ..MessageFilter::default()
        };

        assert!(filter.should_send(&message), "the filter should have sent {message:#?}");

        let filter = MessageFilter {
            tags: vec!["foo:ba".to_string()].into_iter().collect(),
            ..MessageFilter::default()
        };

        assert!(!filter.should_send(&message), "the filter should not have sent {message:#?}");
    }
}
