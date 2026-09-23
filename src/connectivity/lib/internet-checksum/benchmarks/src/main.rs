// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_criterion::FuchsiaCriterion;
use fuchsia_criterion::criterion::{self as criterion, Criterion};
#[allow(unused)]
use internet_checksum::{Checksum, checksum, update};

fn main() {
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(std::time::Duration::from_millis(1))
        .measurement_time(std::time::Duration::from_millis(100))
        .sample_size(100);
    let name = "fuchsia.netstack.internet-checksum";

    macro_rules! bench_sizes {
        ($group:expr, $prefix:ident, $func:ident, $( $size:literal ),*) => {
            $(
                let _ = $group.bench_function(
                    concat!(stringify!($prefix), "/", stringify!($size)),
                    $func::<$size>,
                );
            )*
        };
    }

    let mut group = c.benchmark_group(name);
    bench_sizes!(group, checksum, bench_checksum, 20, 31, 32, 64, 128, 256, 1023, 1024);
    bench_sizes!(group, update, bench_update, 2, 4, 8);
    group.finish();
}

fn bench_checksum<const N: usize>(bencher: &mut criterion::Bencher<'_>) {
    bencher.iter(|| {
        let buf = std::hint::black_box([0xFF; N]);
        let mut c = Checksum::new();
        c.add_bytes(&buf);
        let _ = std::hint::black_box(c.checksum());
    });
}

fn bench_update<const N: usize>(bencher: &mut criterion::Bencher<'_>) {
    bencher.iter(|| {
        let old = std::hint::black_box([0xDE; N]);
        let new = std::hint::black_box([0xAD; N]);
        let _ = std::hint::black_box(update([0xBE, 0xEF], &old[..], &new[..]));
    });
}
