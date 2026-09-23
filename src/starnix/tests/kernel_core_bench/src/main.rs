// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Benchmarks for the starnix kernel core library.

use fuchsia_criterion::FuchsiaCriterion;
use fuchsia_criterion::criterion::{self as criterion, Criterion};
use fuchsia_runtime as frt;
use starnix_core::time;

fn main() {
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(std::time::Duration::from_millis(100))
        .measurement_time(std::time::Duration::from_millis(1000))
        .sample_size(100);
    let name = "fuchsia.starnix.kernel.core";

    let mut group = c.benchmark_group(name);
    let _ = group.bench_function("unstarted_clock_now", bench_unstarted_clock_now);
    let _ = group.bench_function("started_clock_now", bench_started_clock_now);
    let _ = group.bench_function(
        "unstarted_clock_estimate_boot_deadline",
        bench_unstarted_clock_estimate_boot_deadline,
    );
    let _ = group.bench_function(
        "started_clock_estimate_boot_deadline",
        bench_started_clock_estimate_boot_deadline,
    );
    group.finish();
}

fn new_clock(more_opts: zx::ClockOpts) -> time::utc::UtcClock {
    let utc_clock = frt::UtcClock::create(
        zx::ClockOpts::MAPPABLE | zx::ClockOpts::MONOTONIC | more_opts,
        Some(frt::UtcInstant::from_nanos(42)),
    )
    .expect("utc clock creation succeeds");
    time::utc::UtcClock::new(utc_clock)
}

fn bench_unstarted_clock_now(bencher: &mut criterion::Bencher<'_>) {
    let clock = new_clock(zx::ClockOpts::empty());
    bencher.iter(|| {
        let _now = std::hint::black_box(clock.now());
    });
}

fn bench_unstarted_clock_estimate_boot_deadline(bencher: &mut criterion::Bencher<'_>) {
    let clock = new_clock(zx::ClockOpts::empty());
    bencher.iter(|| {
        let _boot_time = std::hint::black_box(clock.estimate_boot_time(frt::UtcInstant::ZERO));
    });
}

fn bench_started_clock_now(bencher: &mut criterion::Bencher<'_>) {
    let clock = new_clock(zx::ClockOpts::AUTO_START);
    bencher.iter(|| {
        let _now = std::hint::black_box(clock.now());
    });
}

fn bench_started_clock_estimate_boot_deadline(bencher: &mut criterion::Bencher<'_>) {
    let clock = new_clock(zx::ClockOpts::AUTO_START);
    bencher.iter(|| {
        let _boot_time = std::hint::black_box(clock.estimate_boot_time(frt::UtcInstant::ZERO));
    });
}
