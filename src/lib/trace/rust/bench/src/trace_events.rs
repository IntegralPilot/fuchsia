// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_criterion::{FuchsiaCriterion, criterion};
use fuchsia_trace as trace;
use fuchsia_trace::Scope;
use std::time::Duration;

#[doc(hidden)]
pub mod __reexport {
    pub use cstringify::cstringify;
}

macro_rules! bench_trace_record_fn {
    ($group:ident, $name:ident $(, $arg:expr)? $(, args: $key:expr => $val:expr)?) => {
        let name = $crate::__reexport::cstringify!($name);
        let _ = $group.bench_function(
            concat!(stringify!($name), "/0Args"),
            move |b| {
                b.iter(|| {trace::$name!(c"benchmark", name $(, $arg)? $(, $key => $val)?);});
            }
        );
        let _ = $group.bench_function(
            concat!(stringify!($name), "/15Args"),
            move |b| {
                b.iter(|| {trace::$name!(c"benchmark", name $(, $arg)?,
                        "a"=>1,
                        "b"=>2,
                        "c"=>3,
                        "d"=>4,
                        "e"=>5,
                        "f"=>6,
                        "g"=>7,
                        "h"=>8,
                        "i"=>9,
                        "j"=>10,
                        "k"=>11,
                        "l"=>12,
                        "m"=>13,
                        "n"=>14,
                        "o"=>15);});
            }
        );
    };
}

macro_rules! bench_async_trace_record_fn {
    ($group:ident, $name:ident) => {
        let name = $crate::__reexport::cstringify!($name);
        let _ = $group.bench_function(
            concat!(stringify!($name), "/0Args"),
            move |b| {
                b.iter(|| trace::$name!(fuchsia_trace::Id::new(), c"benchmark", name));
            }
        );
        let _ = $group.bench_function(
            concat!(stringify!($name), "/15Args"),
            move |b| {
                b.iter(|| trace::$name!(fuchsia_trace::Id::new(), c"benchmark", name,
                        "a"=>1,
                        "b"=>2,
                        "c"=>3,
                        "d"=>4,
                        "e"=>5,
                        "f"=>6,
                        "g"=>7,
                        "h"=>8,
                        "i"=>9,
                        "j"=>10,
                        "k"=>11,
                        "l"=>12,
                        "m"=>13,
                        "n"=>14,
                        "o"=>15));
            }
        );
    };
}

fn main() {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    fuchsia_trace_provider::trace_provider_wait_for_init();

    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut criterion::Criterion = &mut c;

    *internal_c = std::mem::take(internal_c)
        .warm_up_time(Duration::from_millis(1))
        .measurement_time(Duration::from_millis(100))
        .sample_size(10);

    let mut group = c.benchmark_group("fuchsia.trace_records.rust");

    let _ = group.bench_function("TraceEventRust/Empty", |b| {
        b.iter(|| 1);
    });

    bench_trace_record_fn!(group, instant, Scope::Process);
    bench_trace_record_fn!(group, counter, 0, args: "a" => 0);
    bench_trace_record_fn!(group, duration);
    bench_trace_record_fn!(group, duration_begin);
    bench_trace_record_fn!(group, duration_end);
    bench_trace_record_fn!(group, flow_begin, 1.into());
    bench_trace_record_fn!(group, flow_step, 1.into());
    bench_trace_record_fn!(group, flow_end, 1.into());
    bench_trace_record_fn!(group, blob, &[1, 2, 3, 4, 5, 6]);

    bench_async_trace_record_fn!(group, async_enter);
    bench_async_trace_record_fn!(group, async_instant);

    group.finish();
}
