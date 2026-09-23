// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::act::ActionContext;
use crate::act_structured::StructuredActionContext;
use crate::metrics::MetricState;
use crate::metrics::metric_value::MetricValue;
use anyhow::{Error, bail};
use injectable_time::{MonotonicInstant, TimeSource};
use regex_lite::Regex;

pub(crate) mod act; // Perform appropriate actions and collect results as strings.
pub(crate) mod act_structured; // Perform appropriate actions and collect serializable actions.
pub(crate) mod config; // Read the config file(s) for metric and action specs.
pub mod inspect_logger;
pub(crate) mod metrics; // Retrieve and calculate the metrics.
pub(crate) mod plugins; // Plugins for additional analysis.
pub(crate) mod result_format; // Formats the triage results.
pub(crate) mod validate; // Check config - including that metrics/triggers work correctly.

pub use act::{Action, ActionResults, SnapshotTrigger, WarningVec};
pub use act_structured::TriageOutput;
pub use config::{ActionTagDirective, DataFetcher, DiagnosticData, ParseResult, Source};
pub use result_format::ActionResultFormatter;

const DEVICE_UPTIME_KEY: &str = "device.uptime";

fn time_from_snapshot(files: &[DiagnosticData]) -> Option<i64> {
    if let Some(file) = files.iter().find(|file| file.source == Source::Annotations)
        && let DataFetcher::KeyValue(fetcher) = &file.data
        && let MetricValue::String(duration) = fetcher.fetch(DEVICE_UPTIME_KEY)
    {
        let re = Regex::new(r"^(\d+)d(\d+)h(\d+)m(\d+)s$").unwrap();
        if let Some(c) = re.captures(&duration) {
            let dhms = (c.get(1), c.get(2), c.get(3), c.get(4));
            if let (Some(d), Some(h), Some(m), Some(s)) = dhms {
                let dhms = (
                    d.as_str().parse::<i64>(),
                    h.as_str().parse::<i64>(),
                    m.as_str().parse::<i64>(),
                    s.as_str().parse::<i64>(),
                );
                if let (Ok(d), Ok(h), Ok(m), Ok(s)) = dhms {
                    return Some(1_000_000_000 * (s + 60 * (m + 60 * (h + 24 * d))));
                }
            }
        }
    }
    None
}

/// Analyze all DiagnosticData against loaded configs and generate corresponding ActionResults.
/// Each DiagnosticData will yield a single ActionResults instance.
/// Minor errors will not be included.
pub fn analyze(
    diagnostic_data: &[DiagnosticData],
    parse_result: &ParseResult,
) -> Result<ActionResults, Error> {
    inner_analyze(diagnostic_data, parse_result, false /* verbose */)
}

/// Analyze all DiagnosticData against loaded configs and generate corresponding ActionResults.
/// Each DiagnosticData will yield a single ActionResults instance.
/// Include minor errors.
pub fn analyze_verbose(
    diagnostic_data: &[DiagnosticData],
    parse_result: &ParseResult,
) -> Result<ActionResults, Error> {
    inner_analyze(diagnostic_data, parse_result, true /* verbose */)
}

fn inner_analyze(
    diagnostic_data: &[DiagnosticData],
    parse_result: &ParseResult,
    verbose: bool,
) -> Result<ActionResults, Error> {
    parse_result.reset_state();
    let now = time_from_snapshot(diagnostic_data);
    let mut action_context =
        ActionContext::new(&parse_result.metrics, &parse_result.actions, diagnostic_data, now);
    action_context.set_verbose(verbose);
    Ok(action_context.process().clone())
}

/// Analyze all DiagnosticData against loaded configs and generate the corresponding TriageOutput.
/// A single TriageOutput instance is returned regardless of the length of DiagnosticData.
pub fn analyze_structured(
    diagnostic_data: &[DiagnosticData],
    parse_result: &ParseResult,
) -> Result<TriageOutput, Error> {
    parse_result.reset_state();
    let now = time_from_snapshot(diagnostic_data);
    let mut structured_action_context = StructuredActionContext::new(
        &parse_result.metrics,
        &parse_result.actions,
        diagnostic_data,
        now,
    );
    Ok(structured_action_context.process().clone())
}

// Do not call this from WASM - WASM does not provde a monotonic clock.
pub fn snapshots(
    data: &[DiagnosticData],
    parse_result: &ParseResult,
) -> (Vec<SnapshotTrigger>, act::WarningVec) {
    parse_result.reset_state();
    let now = Some(MonotonicInstant::new().now());
    let evaluator = ActionContext::new(&parse_result.metrics, &parse_result.actions, data, now);
    evaluator.into_snapshots()
}

pub fn all_selectors(parse: &ParseResult) -> Vec<String> {
    parse.all_selectors()
}

pub fn evaluate_int_math(expression: &str) -> Result<i64, Error> {
    metric_value_to_int(MetricState::evaluate_math(expression))
}

pub fn metric_value_to_int(metric_value: MetricValue) -> Result<i64, Error> {
    match metric_value {
        MetricValue::Int(i) => Ok(i),
        MetricValue::Float(f) => match metrics::safe_float_to_int(f) {
            Some(i) => Ok(i),
            None => bail!("Non-numeric float result {}", f),
        },
        MetricValue::Problem(problem) => bail!("Eval error: {:?}", problem),
        bad_type => bail!("Non-numeric result: {:?}", bad_type),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    fn time_parses_correctly() {
        fn file(name: &str, source: Source, contents: &str) -> DiagnosticData {
            DiagnosticData::new(name.to_string(), source, contents.to_string()).unwrap()
        }
        assert_eq!(time_from_snapshot(&[]), None);
        // DiagnosticData can't be created with invalid JSON.
        let files = vec![file("foo.json", Source::Annotations, r#"{"a":"b"}"#)];
        assert_eq!(time_from_snapshot(&files), None);
        let files = vec![file("any.name.works", Source::Annotations, r#"{"device.uptime":"b"}"#)];
        assert_eq!(time_from_snapshot(&files), None);
        let files = vec![file("foo.json", Source::Annotations, r#"{"device.uptime":"1h1m1s"}"#)];
        assert_eq!(time_from_snapshot(&files), None);
        let files = vec![file("foo.json", Source::Annotations, r#"{"device.uptime":"1d1h1m"}"#)];
        assert_eq!(time_from_snapshot(&files), None);
        let files = vec![file("foo.json", Source::Annotations, r#"{"device.uptime":"0d0h0m0s"}"#)];
        assert_eq!(time_from_snapshot(&files), Some(0));
        let files = vec![file("a.b", Source::Annotations, r#"{"device.uptime":"2d3h4m5s"}"#)];
        assert_eq!(time_from_snapshot(&files), Some(1_000_000_000 * 183845));
        let files = vec![file("a.b", Source::Annotations, r#"{"device.uptime":"11d13h17m19s"}"#)];
        let seconds = 19 + 17 * 60 + 13 * 3600 + 11 * 3600 * 24;
        assert_eq!(time_from_snapshot(&files), Some(1_000_000_000 * seconds));
        let files = vec![file("", Source::Annotations, r#"{"device.uptime":"3d5h7m11s"}"#)];
        let hours = 5 + 24 * 3;
        let minutes = 7 + 60 * hours;
        let seconds = 11 + 60 * minutes;
        assert_eq!(time_from_snapshot(&files), Some(1_000_000_000 * seconds));
        let files = vec![file("foo.json", Source::Annotations, r#"{"device.uptime":"0d0h0m1s"}"#)];
        assert_eq!(time_from_snapshot(&files), Some(1_000_000_000));
        let files = vec![file("foo.json", Source::Annotations, r#"{"device.uptime":"0d0h1m0s"}"#)];
        assert_eq!(time_from_snapshot(&files), Some(1_000_000_000 * 60));
        let files = vec![file("foo.json", Source::Annotations, r#"{"device.uptime":"0d1h0m0s"}"#)];
        assert_eq!(time_from_snapshot(&files), Some(1_000_000_000 * 60 * 60));
        let files = vec![file("foo.json", Source::Annotations, r#"{"device.uptime":"1d0h0m0s"}"#)];
        assert_eq!(time_from_snapshot(&files), Some(1_000_000_000 * 60 * 60 * 24));
    }
}
