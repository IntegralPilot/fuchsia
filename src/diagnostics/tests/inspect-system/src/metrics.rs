// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsiaperf::{Direction, FuchsiaPerfBenchmarkResult, Unit};
use std::collections::HashMap;
use std::fmt::Write;

/// Container for metrics associated with a particular test.
///
/// The set of metrics may optionally contain hints for the types
/// of the data stored, which affects how it is displayed.
#[derive(Default)]
pub struct MetricSet {
    metrics: HashMap<&'static str, Vec<f64>>,
    type_hints: HashMap<&'static str, MetricTypeHint>,
}

impl MetricSet {
    pub fn set_type_hints(&mut self, hints: impl Iterator<Item = (&'static str, MetricTypeHint)>) {
        self.type_hints.extend(hints);
    }

    /// Add a measurement for the given metric name.
    ///
    /// Metric names must be static.
    pub fn add_measurement(&mut self, name: &'static str, value: f64) {
        self.metrics.entry(name).or_default().push(value);
    }

    /// Output the contained metrics in the fuchsiaperf file format to the given writer.
    ///
    /// test_case is used to format the label of the metric in the output.
    /// The test suite is always the constant "fuchsia.system_inspect_metrics".
    ///
    /// Note: Only metrics with a MetricTypeHint set are included in this output.
    ///
    /// See here for more info: https://fuchsia.dev/fuchsia-src/development/performance/fuchsiaperf_format
    pub fn write_fuchsiaperf(
        &self,
        test_case: &str,
        writer: impl std::io::Write,
    ) -> Result<(), Error> {
        let mut perf_result = vec![];

        for (key, values) in self.metrics.iter() {
            if let Some(type_hint) = self.type_hints.get(*key) {
                perf_result.push(FuchsiaPerfBenchmarkResult {
                    label: format!("{key}/{test_case}"),
                    test_suite: "fuchsia.system_inspect_metrics".to_string(),
                    unit: type_hint.unit,
                    direction: Direction::SmallerBetter,
                    values: values.clone(),
                })
            }
        }

        serde_json::to_writer_pretty(writer, &perf_result)?;

        Ok(())
    }

    /// Format the contained metrics as human-readable text.
    pub fn format_text(&self) -> String {
        if self.metrics.is_empty() {
            return "Empty metric set...".to_string();
        }

        let mut keys = self.metrics.keys().collect::<Vec<_>>();
        keys.sort();

        const EXTRA_FORMATTING_CHARACTERS_FOR_UNIT: usize = 3;
        let get_key_display_len = |key: &str| {
            key.len()
                + self
                    .type_hints
                    .get(key)
                    .map(|v| v.unit.to_string().len() + EXTRA_FORMATTING_CHARACTERS_FOR_UNIT)
                    .unwrap_or_default()
        };

        // Calculate longest key name + unit name in parentheses w/ space
        let max_len = keys.iter().map(|v| get_key_display_len(v)).max().unwrap();

        let mut ret = String::new();
        for key in keys.iter() {
            let pad = " ".repeat(max_len - get_key_display_len(key));
            writeln!(
                ret,
                "{}{}{} = {}",
                key,
                self.type_hints.get(*key).map(|v| format!(" ({})", v.unit)).unwrap_or_default(),
                pad,
                Self::format_stats_text(self.metrics.get(*key).unwrap(), self.type_hints.get(*key))
            )
            .ok();
        }

        ret
    }

    fn format_stats_text(vals: &[f64], maybe_type_hint: Option<&MetricTypeHint>) -> String {
        if vals.is_empty() {
            return "No measurements".to_string();
        }

        let mut min = vals[0];
        let mut max = vals[1];
        let mut total = 0f64;
        for v in vals {
            min = min.min(*v);
            max = max.max(*v);
            total += *v;
        }

        // By default, types are decimal. Otherwise it is overridden by the type hint.
        let is_decimal = maybe_type_hint.map(|v| v.is_integral).unwrap_or(true);
        if is_decimal {
            format!(
                "Avg: {:10.3}  Count: {:4}  Min: {:10}  Max: {:10}",
                total / (vals.len() as f64),
                vals.len(),
                min.trunc() as i64,
                max.trunc() as i64,
            )
        } else {
            format!(
                "Avg: {:10.3}  Count: {:4}  Min: {:10.3}  Max: {:10.3}",
                total / (vals.len() as f64),
                vals.len(),
                min,
                max
            )
        }
    }
}

/// Configuration that provides hints for metric output formatting.
pub struct MetricTypeHint {
    /// If true, format the metric as an integer where that makes sense.
    /// True is the default if no type hint is provided for a metric.
    pub is_integral: bool,

    /// The name of the units for the metric.
    ///
    /// This is formatted in the text output, and it is directly
    /// copied to the "units" field in fuchsiaperf output.
    pub unit: Unit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_format_empty() {
        let metrics = MetricSet::default();

        assert_eq!(metrics.format_text(), "Empty metric set...",);
    }

    #[test]
    fn text_format_no_hint() {
        let mut metrics = MetricSet::default();

        metrics.add_measurement("test", 5.0);
        metrics.add_measurement("test", 10.0);

        assert_eq!(
            metrics.format_text(),
            "test = Avg:      7.500  Count:    2  Min:          5  Max:         10\n"
        );
    }

    #[test]
    fn text_format_hint() {
        let mut metrics = MetricSet::default();
        metrics.set_type_hints(
            [("test", MetricTypeHint { is_integral: false, unit: Unit::Milliseconds })].into_iter(),
        );

        metrics.add_measurement("test", 5.0);
        metrics.add_measurement("test", 10.0);

        assert_eq!(
            metrics.format_text(),
            "test (ms) = Avg:      7.500  Count:    2  Min:      5.000  Max:     10.000\n"
        );
    }

    #[test]
    fn fuchsiaperf_no_hint_empty() {
        // Only metrics with type hints are included in fuchsiaperf, so we expect there to be no metrics in this output.
        let mut metrics = MetricSet::default();

        metrics.add_measurement("test", 5.0);
        metrics.add_measurement("test", 10.0);

        let mut bytes = vec![];
        metrics.write_fuchsiaperf("core/fake", &mut bytes).expect("write");
        let val: Vec<FuchsiaPerfBenchmarkResult> =
            serde_json::from_slice(bytes.as_slice()).expect("deserialize");

        assert_eq!(val.len(), 0);
    }

    #[test]
    fn fuchsiaperf_hint_multiple() {
        let mut metrics = MetricSet::default();
        metrics.set_type_hints(
            [
                ("test", MetricTypeHint { is_integral: false, unit: Unit::Milliseconds }),
                ("size", MetricTypeHint { is_integral: false, unit: Unit::Bytes }),
            ]
            .into_iter(),
        );

        metrics.add_measurement("test", 5.0);
        metrics.add_measurement("test", 10.0);
        metrics.add_measurement("size", 100.0);

        let mut bytes = vec![];
        metrics.write_fuchsiaperf("core::fake", &mut bytes).expect("write");
        let val: Vec<FuchsiaPerfBenchmarkResult> =
            serde_json::from_slice(bytes.as_slice()).expect("deserialize");

        assert_eq!(val.len(), 2);

        let test_entry = val.iter().find(|v| v.unit == Unit::Milliseconds);
        assert!(test_entry.is_some(), "Expected to find one metric with unit 'ms'");
        let test_entry = test_entry.unwrap();

        let size_entry = val.iter().find(|v| v.unit == Unit::Bytes);
        assert!(size_entry.is_some(), "Expected to find one metric with unit 'bytes'");
        let size_entry = size_entry.unwrap();

        assert_eq!(test_entry.test_suite, "fuchsia.system_inspect_metrics");
        assert_eq!(size_entry.test_suite, "fuchsia.system_inspect_metrics");

        assert_eq!(test_entry.label, "test/core::fake");
        assert_eq!(size_entry.label, "size/core::fake");

        assert_eq!(test_entry.values, vec![5.0, 10.0]);
        assert_eq!(size_entry.values, vec![100.0]);
    }
}
