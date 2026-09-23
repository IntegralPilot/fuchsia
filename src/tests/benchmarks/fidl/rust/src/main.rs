// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use criterion::Criterion;
use fuchsia_criterion::FuchsiaCriterion;
use std::mem;
use std::time::Duration;

fn main() {
    // FuchsiaCriterion is a wrapper around Criterion. To configure the inner
    // Criterion we have to use a strange, indirect approach. This is because
    // FuchsiaCriterion only provides access to it via DerefMut, and Criterion
    // only provides a builder API (i.e. consuming self) for configuration.
    let mut fc = FuchsiaCriterion::default();
    let c: &mut Criterion = &mut fc;
    *c = mem::take(c)
        .warm_up_time(Duration::from_millis(1))
        .measurement_time(Duration::from_millis(500))
        // We must reduce the sample size from the default of 100, otherwise
        // Criterion will sometimes override the 1ms + 500ms suggested times
        // and run for much longer. Criterion requires sample_size >= 10.
        .sample_size(10);

    let mut group = c.benchmark_group("fuchsia.fidl_microbenchmarks");
    let all = &benchmark_suite::ALL_BENCHMARKS;
    let benchmark_defs = all.iter().copied().flatten();
    for (label, function) in benchmark_defs {
        let _ = group.bench_function(wall_time_label(label), function);
    }
    group.finish();
}

fn wall_time_label(base: &str) -> String {
    format!("Rust/{}/WallTime", base)
}
