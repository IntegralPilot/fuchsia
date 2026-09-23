// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ahash::AHasher;
use criterion::{BatchSize, BenchmarkId, Criterion};
use flyweights::FlyStr;
use std::hash::{Hash, Hasher};
use std::hint::black_box;

const INPUTS: &[&str; 6] = &[
    // Empty
    "",
    // Extra-small
    "1",
    // Largest inline
    "1234567",
    // Smallest heap
    "12345678",
    // Larger heap
    "123456789123456789123456789123456789123456789123456789123456789123456789123456789123456789",
    // Really big heap
    concat!(
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
        "123456789123456789123456789123456789123456789123456789123456789123456789123456789",
    ),
];

#[cfg(not(miri))]
criterion::criterion_main!(lifecycle, access, hashing, comparison);

// Criterion doesn't work in miri right now: https://github.com/bheisler/criterion.rs/issues/778
#[cfg(miri)]
fn main() {}

macro_rules! bench_over_inputs {
    ($name:ident, |$bencher:ident, $input:ident| $bench:expr) => {
        fn $name(c: &mut Criterion) {
            let mut group = c.benchmark_group(stringify!($name));
            group.plot_config(
                criterion::PlotConfiguration::default()
                    .summary_scale(criterion::AxisScale::Logarithmic),
            );
            for input in INPUTS {
                group.bench_with_input(
                    BenchmarkId::from_parameter(input.len()),
                    input,
                    |$bencher, input: &&str| {
                        let $input = black_box(*input);
                        $bench;
                    },
                );
            }
            group.finish();
        }
    };
}

criterion::criterion_group!(
    lifecycle,
    from_str_first,
    from_str_dupe,
    clone,
    drop_dupe,
    drop_last
);

bench_over_inputs!(from_str_first, |b, input| b.iter(|| FlyStr::from(input)));

bench_over_inputs!(from_str_dupe, |b, input| {
    // Make a copy of the FlyStr that will be live the whole time.
    let _existing = FlyStr::from(input);
    b.iter(|| FlyStr::from(input));
});

bench_over_inputs!(clone, |b, input| {
    let first = black_box(FlyStr::from(input));
    b.iter(|| first.clone());
});

bench_over_inputs!(drop_dupe, |b, input| {
    // Make a copy of the FlyStr that will be live the whole time so we drop a copy.
    let input = black_box(FlyStr::from(input));
    b.iter_batched(|| input.clone(), drop, BatchSize::PerIteration);
});

bench_over_inputs!(drop_last, |b, input| {
    b.iter_batched(|| FlyStr::from(input), drop, BatchSize::PerIteration);
});

criterion::criterion_group!(access, as_str);

bench_over_inputs!(as_str, |b, input| {
    let input = black_box(FlyStr::from(input));
    b.iter(|| input.as_str());
});

criterion::criterion_group!(hashing, ahash);

bench_over_inputs!(ahash, |b, input| {
    let input = black_box(FlyStr::from(input));
    b.iter_batched_ref(
        AHasher::default,
        |hasher| {
            input.hash(hasher);
            hasher.finish()
        },
        BatchSize::PerIteration,
    );
});

criterion::criterion_group!(comparison, eq);

bench_over_inputs!(eq, |b, input| {
    let first = black_box(FlyStr::from(input));
    let second = black_box(first.clone());
    b.iter(|| first == second);
});
