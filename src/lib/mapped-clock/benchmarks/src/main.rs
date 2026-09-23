// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Benchmarks for the `mapped-clock` library.
//!
//! Check the performance of reading a clock directly versus reading a clock that has been mapped
//! into the address space.

use fuchsia_criterion::FuchsiaCriterion;
use fuchsia_criterion::criterion::{self as criterion, Criterion};
use fuchsia_runtime as frt;
use mapped_clock::MappedClock;

fn main() {
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(std::time::Duration::from_millis(1000))
        .measurement_time(std::time::Duration::from_millis(1000))
        .sample_size(100);
    let name = "fuchsia.lib.mapped-clock";

    let mut group = c.benchmark_group(name);
    let _ = group.bench_function("baseline_read", bench_baseline_read);
    let _ = group.bench_function("mapped_read", bench_mapped_read);
    let _ = group.bench_function("baseline_get_details", bench_baseline_get_details);
    let _ = group.bench_function("mapped_get_details", bench_mapped_get_details);
    group.finish();
}

fn new_clock() -> zx::Clock<zx::MonotonicTimeline, zx::SyntheticTimeline> {
    zx::SyntheticClock::create(
        zx::ClockOpts::MAPPABLE | zx::ClockOpts::MONOTONIC | zx::ClockOpts::AUTO_START,
        Some(zx::SyntheticInstant::from_nanos(42)),
    )
    .unwrap()
}

fn new_mapped_clock() -> MappedClock<zx::MonotonicTimeline, zx::SyntheticTimeline> {
    let clock = new_clock();
    let vmar_root = frt::vmar_root_self();
    let clock_clone = clock.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    MappedClock::try_new(clock_clone, &vmar_root, zx::VmarFlags::PERM_READ).unwrap()
}

// Baseline benchmark for reading a clock from a Zircon clock object.
fn bench_baseline_read(bencher: &mut criterion::Bencher<'_>) {
    let clock = new_clock();
    bencher.iter(|| {
        let _now = std::hint::black_box(clock.read()).expect("read is a success");
    });
}

// Benchmark for reading a mapped clock.
fn bench_mapped_read(bencher: &mut criterion::Bencher<'_>) {
    let mapped_clock = new_mapped_clock();
    bencher.iter(|| {
        let _now = std::hint::black_box(mapped_clock.read()).expect("read is a success");
    });
}

// Baseline benchmark for getting clock details from a Zircon clock object.
fn bench_baseline_get_details(bencher: &mut criterion::Bencher<'_>) {
    let clock = new_clock();
    bencher.iter(|| {
        let _out = std::hint::black_box(clock.get_details()).expect("get_details is a success");
    });
}

// Benchmark for getting clock details from a mapped clock.
fn bench_mapped_get_details(bencher: &mut criterion::Bencher<'_>) {
    let mapped_clock = new_mapped_clock();
    bencher.iter(|| {
        let _out =
            std::hint::black_box(mapped_clock.get_details()).expect("get_details is a success");
    });
}
