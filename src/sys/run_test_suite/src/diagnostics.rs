// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::trace::duration;
use anyhow::Error;
use diagnostics_data::{LogTextDisplayOptions, LogTextPresenter, LogsData, Severity};
use flex_fuchsia_diagnostics::LogInterestSelector;
use flex_fuchsia_test_manager::LogsIteratorType;
use futures::{Stream, TryStreamExt};
use selectors::SelectorExt;
use std::io::Write;

/// Configuration for display of text-based (unstructured)
/// logs.
#[derive(Clone, Default)]
pub(crate) struct LogDisplayConfiguration {
    /// The minimum severity log to display.
    pub interest: Vec<LogInterestSelector>,

    /// How to format messages as text.
    pub text_options: LogTextDisplayOptions,
}

// TODO(https://fxbug.dev/42131693, https://fxbug.dev/42149841): deprecate this when implementing metadata selectors for
// logs or when we support automatically sending interest updates to all test components on startup.
// We currently don't have a way of setting the interest of a test realm before creating that realm.
#[derive(Clone, Default)]
pub(crate) struct LogCollectionOptions {
    /// The maximum allowed severity for logs.
    pub max_severity: Option<Severity>,

    /// Log display options for unstructured logs.
    pub format: LogDisplayConfiguration,
}

impl LogCollectionOptions {
    fn is_restricted_log(&self, log: &LogsData) -> bool {
        let severity = log.metadata.severity;
        matches!(self.max_severity, Some(max) if severity > max)
    }

    fn should_display(&self, log: &LogsData) -> bool {
        if self.format.interest.is_empty() {
            return true;
        }
        let mut found_matching_selector = false;
        for LogInterestSelector { interest, selector } in &self.format.interest {
            let Some(min_severity) = interest.min_severity.as_ref() else {
                continue;
            };
            // The selector should already be validated so in practice this will never happen.
            if log.moniker.matches_component_selector(&selector).unwrap_or(false) {
                found_matching_selector = true;
                if log.severity() >= *min_severity {
                    return true;
                }
            }
        }
        !found_matching_selector
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LogCollectionOutcome {
    Error { restricted_logs: Vec<String> },
    Passed,
}

impl From<Vec<String>> for LogCollectionOutcome {
    fn from(restricted_logs: Vec<String>) -> Self {
        if restricted_logs.is_empty() {
            LogCollectionOutcome::Passed
        } else {
            LogCollectionOutcome::Error { restricted_logs }
        }
    }
}

/// Collects logs from |stream|, filters out low severity logs, and stores the results
/// in |log_artifact|. Returns any high severity restricted logs that are encountered.
pub(crate) async fn collect_logs<S, W>(
    mut stream: S,
    mut log_artifact: W,
    options: LogCollectionOptions,
) -> Result<LogCollectionOutcome, Error>
where
    S: Stream<Item = Result<LogsData, Error>> + Unpin,
    W: Write,
{
    duration!("collect_logs");
    let mut restricted_logs = vec![];
    while let Some(log) = stream.try_next().await? {
        duration!("process_single_log");
        let is_restricted = options.is_restricted_log(&log);
        let should_display = options.should_display(&log);
        if !should_display && !is_restricted {
            continue;
        }

        let log_repr = format!("{}", LogTextPresenter::new(&log, options.format.text_options));

        if should_display {
            writeln!(log_artifact, "{}", log_repr)?;
        }

        if is_restricted {
            restricted_logs.push(log_repr);
        }
    }
    Ok(restricted_logs.into())
}

#[cfg(target_os = "fuchsia")]
pub fn get_logs_iterator_type() -> LogsIteratorType {
    LogsIteratorType::Batch
}

#[cfg(not(target_os = "fuchsia"))]
pub fn get_logs_iterator_type() -> LogsIteratorType {
    LogsIteratorType::Socket
}

#[cfg(test)]
mod test {
    use super::*;
    use diagnostics_data::{BuilderArgs, LogsDataBuilder, Timestamp};
    use moniker::Moniker;

    #[fuchsia::test]
    async fn filter_low_severity() {
        let input_logs = vec![
            LogsDataBuilder::new(BuilderArgs {
                moniker: Moniker::root().into(),
                timestamp: Timestamp::from_nanos(0),
                component_url: Some("test-root-url".into()),
                severity: Severity::Info,
            })
            .set_message("my info log")
            .build(),
            LogsDataBuilder::new(BuilderArgs {
                moniker: "child".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(1000),
                component_url: Some("test-child-url".into()),
                severity: Severity::Warn,
            })
            .set_message("my info log")
            .build(),
        ];
        let displayed_logs = vec![
            LogsDataBuilder::new(BuilderArgs {
                moniker: "child".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(1000),
                component_url: Some("test-child-url".into()),
                severity: Severity::Warn,
            })
            .set_message("my info log")
            .build(),
        ];

        let mut log_artifact = vec![];
        assert_eq!(
            collect_logs(
                futures::stream::iter(input_logs.into_iter().map(Ok)),
                &mut log_artifact,
                LogCollectionOptions {
                    max_severity: None,
                    format: LogDisplayConfiguration {
                        text_options: LogTextDisplayOptions {
                            show_full_moniker: true,
                            ..Default::default()
                        },
                        interest: vec![
                            selectors::parse_log_interest_selector_or_severity("WARN").unwrap()
                        ],
                    }
                },
            )
            .await
            .unwrap(),
            LogCollectionOutcome::Passed
        );
        assert_eq!(
            String::from_utf8(log_artifact).unwrap(),
            displayed_logs.iter().map(|log| format!("{}\n", log)).collect::<Vec<_>>().concat()
        );
    }

    #[fuchsia::test]
    async fn filter_log_severity_by_component() {
        let a_info_log = LogsDataBuilder::new(BuilderArgs {
            moniker: "a".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("test-root-url".into()),
            severity: Severity::Info,
        })
        .set_message("A's info log")
        .build();
        let a_warn_log = LogsDataBuilder::new(BuilderArgs {
            moniker: "a".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("test-root-url".into()),
            severity: Severity::Warn,
        })
        .set_message("A's warn log")
        .build();
        let b_info_log = LogsDataBuilder::new(BuilderArgs {
            moniker: "b".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("test-root-url".into()),
            severity: Severity::Info,
        })
        .set_message("B's info log")
        .build();
        let b_warn_log = LogsDataBuilder::new(BuilderArgs {
            moniker: "b".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("test-root-url".into()),
            severity: Severity::Warn,
        })
        .set_message("B's warn log")
        .build();
        let c_info_log = LogsDataBuilder::new(BuilderArgs {
            moniker: "c".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("test-root-url".into()),
            severity: Severity::Info,
        })
        .set_message("C's info log")
        .build();

        let input_logs = vec![
            a_info_log,
            a_warn_log.clone(),
            b_info_log,
            b_warn_log.clone(),
            c_info_log.clone(),
        ];
        let displayed_logs = vec![a_warn_log, b_warn_log, c_info_log];

        let mut log_artifact = vec![];
        assert_eq!(
            collect_logs(
                futures::stream::iter(input_logs.into_iter().map(Ok)),
                &mut log_artifact,
                LogCollectionOptions {
                    max_severity: None,
                    format: LogDisplayConfiguration {
                        text_options: LogTextDisplayOptions {
                            show_full_moniker: true,
                            ..Default::default()
                        },
                        interest: vec![
                            selectors::parse_log_interest_selector_or_severity("a#WARN").unwrap(),
                            selectors::parse_log_interest_selector_or_severity("b#WARN").unwrap(),
                        ],
                    }
                },
            )
            .await
            .unwrap(),
            LogCollectionOutcome::Passed
        );
        assert_eq!(
            String::from_utf8(log_artifact).unwrap(),
            displayed_logs.iter().map(|log| format!("{}\n", log)).collect::<Vec<_>>().concat()
        );
    }

    #[fuchsia::test]
    async fn filter_log_severity_by_component_multiple_matches() {
        let a_info_log = LogsDataBuilder::new(BuilderArgs {
            moniker: "a".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("test-root-url".into()),
            severity: Severity::Info,
        })
        .set_message("A's info log")
        .build();
        let a_warn_log = LogsDataBuilder::new(BuilderArgs {
            moniker: "a".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("test-root-url".into()),
            severity: Severity::Warn,
        })
        .set_message("A's warn log")
        .build();
        let b_info_log = LogsDataBuilder::new(BuilderArgs {
            moniker: "b".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("test-root-url".into()),
            severity: Severity::Info,
        })
        .set_message("B's info log")
        .build();
        let b_fatal_log = LogsDataBuilder::new(BuilderArgs {
            moniker: "b".try_into().unwrap(),
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("test-root-url".into()),
            severity: Severity::Fatal,
        })
        .set_message("B's fatal log")
        .build();

        let input_logs = vec![a_info_log, a_warn_log.clone(), b_info_log, b_fatal_log.clone()];
        let displayed_logs = vec![a_warn_log, b_fatal_log];

        let mut log_artifact = vec![];
        assert_eq!(
            collect_logs(
                futures::stream::iter(input_logs.into_iter().map(Ok)),
                &mut log_artifact,
                LogCollectionOptions {
                    max_severity: None,
                    format: LogDisplayConfiguration {
                        text_options: LogTextDisplayOptions {
                            show_full_moniker: true,
                            ..Default::default()
                        },
                        interest: vec![
                            selectors::parse_log_interest_selector_or_severity("**#FATAL").unwrap(),
                            selectors::parse_log_interest_selector_or_severity("a#WARN").unwrap(),
                        ],
                    }
                },
            )
            .await
            .unwrap(),
            LogCollectionOutcome::Passed
        );
        assert_eq!(
            String::from_utf8(log_artifact).unwrap(),
            displayed_logs.iter().map(|log| format!("{}\n", log)).collect::<Vec<_>>().concat()
        );
    }

    #[fuchsia::test]
    async fn filter_log_moniker() {
        let unaltered_logs = vec![
            LogsDataBuilder::new(BuilderArgs {
                moniker: Moniker::root().into(),
                timestamp: Timestamp::from_nanos(0),
                component_url: Some("test-root-url".into()),
                severity: Severity::Info,
            })
            .set_message("my info log")
            .build(),
            LogsDataBuilder::new(BuilderArgs {
                moniker: "child/a".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(1000),
                component_url: Some("test-child-url".into()),
                severity: Severity::Warn,
            })
            .set_message("my warn log")
            .build(),
        ];
        let altered_moniker_logs = vec![
            LogsDataBuilder::new(BuilderArgs {
                moniker: Moniker::root().into(),
                timestamp: Timestamp::from_nanos(0),
                component_url: Some("test-root-url".into()),
                severity: Severity::Info,
            })
            .set_message("my info log")
            .build(),
            LogsDataBuilder::new(BuilderArgs {
                moniker: "a".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(1000),
                component_url: Some("test-child-url".into()),
                severity: Severity::Warn,
            })
            .set_message("my warn log")
            .build(),
        ];

        let mut log_artifact = vec![];
        assert_eq!(
            collect_logs(
                futures::stream::iter(unaltered_logs.into_iter().map(Ok)),
                &mut log_artifact,
                LogCollectionOptions {
                    max_severity: None,
                    format: LogDisplayConfiguration {
                        text_options: LogTextDisplayOptions {
                            show_full_moniker: false,
                            ..Default::default()
                        },
                        interest: vec![]
                    }
                }
            )
            .await
            .unwrap(),
            LogCollectionOutcome::Passed
        );
        assert_eq!(
            String::from_utf8(log_artifact).unwrap(),
            altered_moniker_logs
                .iter()
                .map(|log| format!(
                    "{}\n",
                    LogTextPresenter::new(
                        log,
                        LogTextDisplayOptions { show_full_moniker: false, ..Default::default() }
                    )
                ))
                .collect::<Vec<_>>()
                .concat()
        );
    }

    #[fuchsia::test]
    async fn no_filter_log_moniker() {
        let unaltered_logs = vec![
            LogsDataBuilder::new(BuilderArgs {
                moniker: ".".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(0),
                component_url: Some("test-root-url".into()),
                severity: Severity::Info,
            })
            .set_message("my info log")
            .build(),
            LogsDataBuilder::new(BuilderArgs {
                moniker: "child/a".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(1000),
                component_url: Some("test-child-url".into()),
                severity: Severity::Warn,
            })
            .set_message("my warn log")
            .build(),
        ];
        let altered_moniker_logs = vec![
            LogsDataBuilder::new(BuilderArgs {
                moniker: ".".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(0),
                component_url: Some("test-root-url".into()),
                severity: Severity::Info,
            })
            .set_message("my info log")
            .build(),
            LogsDataBuilder::new(BuilderArgs {
                moniker: "child/a".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(1000),
                component_url: Some("test-child-url".into()),
                severity: Severity::Warn,
            })
            .set_message("my warn log")
            .build(),
        ];

        let mut log_artifact = vec![];
        assert_eq!(
            collect_logs(
                futures::stream::iter(unaltered_logs.into_iter().map(Ok)),
                &mut log_artifact,
                LogCollectionOptions {
                    max_severity: None,
                    format: LogDisplayConfiguration {
                        text_options: LogTextDisplayOptions {
                            show_full_moniker: true,
                            ..Default::default()
                        },
                        interest: vec![],
                    }
                }
            )
            .await
            .unwrap(),
            LogCollectionOutcome::Passed
        );
        assert_eq!(
            String::from_utf8(log_artifact).unwrap(),
            altered_moniker_logs
                .iter()
                .map(|log| format!("{}\n", log))
                .collect::<Vec<_>>()
                .concat()
        );
    }

    #[fuchsia::test]
    async fn display_restricted_logs() {
        let input_logs = vec![
            LogsDataBuilder::new(BuilderArgs {
                moniker: Moniker::root().into(),
                timestamp: Timestamp::from_nanos(0),
                component_url: Some("test-root-url".into()),
                severity: Severity::Info,
            })
            .set_message("my info log")
            .build(),
            LogsDataBuilder::new(BuilderArgs {
                moniker: "child".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(1000),
                component_url: Some("test-child-url".into()),
                severity: Severity::Error,
            })
            .set_message("my error log")
            .build(),
        ];
        let displayed_logs = vec![
            LogsDataBuilder::new(BuilderArgs {
                moniker: Moniker::root().into(),
                timestamp: Timestamp::from_nanos(0),
                component_url: Some("test-root-url".into()),
                severity: Severity::Info,
            })
            .set_message("my info log")
            .build(),
            LogsDataBuilder::new(BuilderArgs {
                moniker: "child".try_into().unwrap(),
                timestamp: Timestamp::from_nanos(1000),
                component_url: Some("test-child-url".into()),
                severity: Severity::Error,
            })
            .set_message("my error log")
            .build(),
        ];

        let mut log_artifact = vec![];
        assert_eq!(
            collect_logs(
                futures::stream::iter(input_logs.into_iter().map(Ok)),
                &mut log_artifact,
                LogCollectionOptions {
                    max_severity: Severity::Warn.into(),
                    format: LogDisplayConfiguration {
                        text_options: LogTextDisplayOptions {
                            show_full_moniker: true,
                            ..Default::default()
                        },
                        interest: vec![]
                    }
                }
            )
            .await
            .unwrap(),
            LogCollectionOutcome::Error { restricted_logs: vec![format!("{}", displayed_logs[1])] }
        );
        assert_eq!(
            String::from_utf8(log_artifact).unwrap(),
            displayed_logs.iter().map(|log| format!("{}\n", log)).collect::<Vec<_>>().concat()
        );
    }
}
