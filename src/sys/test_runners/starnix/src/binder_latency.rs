// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Runs a benchmark and converts the [custom output format] to fuchsiaperf JSON.
//!
//! [custom output format]: https://source.android.com/docs/core/tests/vts/performance#iterations

use crate::helpers::*;
use anyhow::{Context as _, Error, ensure};
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_test::{self as ftest};
use fuchsiaperf::{Direction, FuchsiaPerfBenchmarkResult, Unit};
use serde::Deserialize;
use std::collections::BTreeMap;

pub async fn run_binder_latency(
    test: ftest::Invocation,
    start_info: frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    component_runner: &frunner::ComponentRunnerProxy,
) -> Result<(), Error> {
    run_starnix_benchmark(
        test,
        start_info,
        run_listener_proxy,
        component_runner,
        "benchmark.json",
        binder_latency_to_fuchsiaperf,
    )
    .await
}

#[derive(Debug, Deserialize)]
struct BinderLatencyResults {
    cfg: BenchmarkConfig,
    #[allow(unused)] // TODO(https://fxbug.dev/42074253) require that inheritance passes
    inheritance: InheritanceResult,
    #[serde(flatten)]
    pairs: BTreeMap<String, PairResult>,
}

#[derive(Debug, Deserialize)]
struct BenchmarkConfig {
    pair: u32,
}

#[derive(Debug, Deserialize)]
struct PairResult {
    #[allow(unused)] // TODO(https://fxbug.dev/42074254) require a good sync ratio?
    #[serde(rename = "SYNC")]
    sync: SyncResult,
    other_ms: Timing,
    fifo_ms: Timing,
}

#[derive(Debug, Deserialize)]
struct Timing {
    avg: f64,
    #[serde(rename = "wst")]
    max: f64,
    #[serde(rename = "bst")]
    min: f64,
}

#[allow(unused)] // TODO(https://fxbug.dev/42074254) require a good sync ratio?
#[derive(Debug, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum SyncResult {
    Good,
    Poor,
}

#[allow(unused)] // TODO(https://fxbug.dev/42074253) require that inheritance passes
#[derive(Debug, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum InheritanceResult {
    Pass,
    Fail,
}

fn binder_latency_to_fuchsiaperf(
    contents: &str,
    test_suite: &str,
) -> Result<Vec<FuchsiaPerfBenchmarkResult>, Error> {
    let results: BinderLatencyResults =
        serde_json::from_str(&contents).context("deserializing results")?;
    ensure!(results.cfg.pair == 1, "expecting only a single pair of processes");

    let timing = results.pairs.get("P0").context("getting first pair's timing results")?;

    Ok(vec![
        FuchsiaPerfBenchmarkResult {
            label: "NormalSchedulerMin".to_string(),
            test_suite: test_suite.to_owned(),
            unit: Unit::Milliseconds,
            direction: Direction::SmallerBetter,
            values: vec![timing.other_ms.min],
        },
        FuchsiaPerfBenchmarkResult {
            label: "NormalSchedulerAvg".to_string(),
            test_suite: test_suite.to_owned(),
            unit: Unit::Milliseconds,
            direction: Direction::SmallerBetter,
            values: vec![timing.other_ms.avg],
        },
        FuchsiaPerfBenchmarkResult {
            label: "NormalSchedulerMax".to_string(),
            test_suite: test_suite.to_owned(),
            unit: Unit::Milliseconds,
            direction: Direction::SmallerBetter,
            values: vec![timing.other_ms.max],
        },
        FuchsiaPerfBenchmarkResult {
            label: "FifoSchedulerMin".to_string(),
            test_suite: test_suite.to_owned(),
            unit: Unit::Milliseconds,
            direction: Direction::SmallerBetter,
            values: vec![timing.fifo_ms.min],
        },
        FuchsiaPerfBenchmarkResult {
            label: "FifoSchedulerAvg".to_string(),
            test_suite: test_suite.to_owned(),
            unit: Unit::Milliseconds,
            direction: Direction::SmallerBetter,
            values: vec![timing.fifo_ms.avg],
        },
        FuchsiaPerfBenchmarkResult {
            label: "FifoSchedulerMax".to_string(),
            test_suite: test_suite.to_owned(),
            unit: Unit::Milliseconds,
            direction: Direction::SmallerBetter,
            values: vec![timing.fifo_ms.max],
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binder_latency_to_fuchsiaperf() {
        let suite = "suite.for.testing";
        let sample = r#"{
"cfg":{"pair":1,"iterations":1000,"deadline_us":2500},
"P0":{"SYNC":"GOOD","S":2000,"I":2000,"R":1,
    "other_ms":{ "avg":2.2 ,"wst":10  ,"bst":0.56,"miss":272,"meetR":0.728},
    "fifo_ms": { "avg":1.9 ,"wst":3.9 ,"bst":0.68,"miss":108,"meetR":0.892}
},
"inheritance": "FAIL"
}"#;
        let expected_perfs = vec![
            FuchsiaPerfBenchmarkResult {
                label: "NormalSchedulerMin".to_string(),
                test_suite: suite.to_owned(),
                unit: Unit::Milliseconds,
                direction: Direction::SmallerBetter,
                values: vec![0.56],
            },
            FuchsiaPerfBenchmarkResult {
                label: "NormalSchedulerAvg".to_string(),
                test_suite: suite.to_owned(),
                unit: Unit::Milliseconds,
                direction: Direction::SmallerBetter,
                values: vec![2.2],
            },
            FuchsiaPerfBenchmarkResult {
                label: "NormalSchedulerMax".to_string(),
                test_suite: suite.to_owned(),
                unit: Unit::Milliseconds,
                direction: Direction::SmallerBetter,
                values: vec![10.0],
            },
            FuchsiaPerfBenchmarkResult {
                label: "FifoSchedulerMin".to_string(),
                test_suite: suite.to_owned(),
                unit: Unit::Milliseconds,
                direction: Direction::SmallerBetter,
                values: vec![0.68],
            },
            FuchsiaPerfBenchmarkResult {
                label: "FifoSchedulerAvg".to_string(),
                test_suite: suite.to_owned(),
                unit: Unit::Milliseconds,
                direction: Direction::SmallerBetter,
                values: vec![1.9],
            },
            FuchsiaPerfBenchmarkResult {
                label: "FifoSchedulerMax".to_string(),
                test_suite: suite.to_owned(),
                unit: Unit::Milliseconds,
                direction: Direction::SmallerBetter,
                values: vec![3.9],
            },
        ];

        let perfs = binder_latency_to_fuchsiaperf(sample, suite).unwrap();
        assert_eq!(perfs.len(), expected_perfs.len());
        for i in 0..perfs.len() {
            assert_eq!(perfs[i], expected_perfs[i]);
        }
    }
}
