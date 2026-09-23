// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::puppet::LogMessage;
use diagnostics_data::{ExtendedMoniker, LogsData};
use fidl_fuchsia_diagnostics_types::Severity;
use futures::{Stream, StreamExt};

type LogStreamItem = Result<LogsData, diagnostics_reader::Error>;

pub(crate) trait LogStream: Stream<Item = LogStreamItem> + Unpin {}
impl<T: Stream<Item = LogStreamItem> + Unpin> LogStream for T {}

/// Asserts that an ordered sequence of contiguous messages appears next in the log output.
///
/// This may completely drain `logs`. Log messages are only matched if they come from a
/// component with moniker `component_moniker`.
///
/// # Panics
///
/// Panics if any the contiguous sequence of messages is not found.
pub(crate) async fn assert_logs_sequence<S: LogStream>(
    mut logs: S,
    component_moniker: &ExtendedMoniker,
    messages: Vec<LogMessage>,
) {
    for (i, (expected_severity, expected_message)) in messages.iter().enumerate() {
        let data = logs.next().await.expect("got log response").expect("log isn't an error");
        if !logs_data_matches(data, component_moniker, expected_severity, expected_message) {
            fail_on_incomplete_match(messages, i);
            return; // Rust thinks this loop can iterate again if this return statement is removed.
        }
    }
}

fn logs_data_matches(
    data: LogsData,
    component_moniker: &ExtendedMoniker,
    severity: &Severity,
    message: &str,
) -> bool {
    data.moniker == *component_moniker
        && data.msg().unwrap() == message
        && data.metadata.severity == *severity
}

// Panics with an error message indicating that only `m` items of `expected_sequence`
// were matched, and highlights which items weren't found.
fn fail_on_incomplete_match(expected_sequence: Vec<LogMessage>, m: usize) {
    let mut err = "ERROR: some log messages were not found (+got, -want):\n".to_string();
    for (i, (severity, message)) in expected_sequence.iter().enumerate() {
        let symbol = if i < m { "+" } else { "-" };
        err += &format!("{symbol} [{severity:?}] {message}\n");
    }
    panic!("{err}");
}
