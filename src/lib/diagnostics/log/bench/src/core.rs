// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_log::{Publisher, PublisherOptions};
use fidl::endpoints::RequestStream;
use fidl_fuchsia_logger::{LogSinkMarker, LogSinkOnInitRequest};
use fuchsia_criterion::{FuchsiaCriterion, criterion};
use ring_buffer::RingBuffer;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

const RING_BUFFER_SIZE: usize = 512 * 1024;

static RING_BUFFER: LazyLock<Arc<RingBuffer>> =
    LazyLock::new(|| Arc::clone(&RingBuffer::create(RING_BUFFER_SIZE)));

fn create_logger() -> Publisher {
    let (client, stream) = fidl::endpoints::create_request_stream::<LogSinkMarker>();
    stream
        .control_handle()
        .send_on_init(LogSinkOnInitRequest {
            buffer: Some(RING_BUFFER.new_iob_writer(0).unwrap().0),
            ..Default::default()
        })
        .unwrap();
    Publisher::new_sync(PublisherOptions::default().tags(&["some-tag"]).use_log_sink(client))
        .unwrap()
}

fn write_log_benchmark<F>(bencher: &mut criterion::Bencher<'_>, mut logging_fn: F)
where
    F: FnMut(),
{
    bencher.iter_batched(
        || {
            // Reset the ring buffer so that it doesn't run out of room.
            RING_BUFFER.increment_tail((RING_BUFFER.head() - RING_BUFFER.tail()) as usize);
        },
        |_| logging_fn(),
        // Limiting the batch size to 100 should prevent the buffer from running out of
        // space.
        criterion::BatchSize::NumIterations(100),
    );
}

// The benchmarks below measure the time it takes to write a log message when calling a macro
// to log. They set up different cases: just a string, a string with arguments, the same string
// but with the arguments formatted, etc. It'll measure the time it takes for the log to go
// through the tracing mechanisms, our encoder and finally writing to the socket.
fn set_up_log_write_benchmarks(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &str,
) {
    let all_args_bench = move |b: &mut criterion::Bencher<'_>| {
        write_log_benchmark(b, || {
            log::info!(
                tag = "logbench",
                boolean = true,
                float = 1234.5678,
                int = -123456,
                string = "foobarbaz",
                uint = 123456;
                "this is a log emitted from the benchmark"
            );
        });
    };
    let _ = group.bench_function(format!("Publisher/{name}/AllArguments"), all_args_bench);
    let _ = group.bench_function(format!("Publisher/{name}/NoArguments"), move |b| {
        write_log_benchmark(b, || {
            log::info!("this is a log emitted from the benchmark");
        });
    });
    let _ = group.bench_function(format!("Publisher/{name}/MessageWithSomeArguments"), move |b| {
        write_log_benchmark(b, || {
            log::info!(
                boolean = true,
                int = -123456,
                string = "foobarbaz";
                "this is a log emitted from the benchmark",
            );
        });
    });
    let _ = group.bench_function(format!("Publisher/{name}/MessageAsString"), move |b| {
        write_log_benchmark(b, || {
            log::info!(
                "this is a log emitted from the benchmark boolean={} int={} string={}",
                true,
                -123456,
                "foobarbaz",
            );
        });
    });
}

fn set_up_old_log_write_benchmarks(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &str,
) {
    let _ = group.bench_function(format!("Publisher/{name}/NoArguments"), move |b| {
        write_log_benchmark(b, || {
            log::info!("this is a log emitted from the benchmark");
        });
    });
    let _ = group.bench_function(format!("Publisher/{name}/MessageAsString"), move |b| {
        write_log_benchmark(b, || {
            log::info!(
                "this is a log emitted from the benchmark boolean={} int={} string={}",
                true,
                -123456,
                "foobarbaz",
            );
        });
    });
}

fn main() {
    let _executor = fuchsia_async::LocalExecutor::default();
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut criterion::Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(Duration::from_millis(1))
        .measurement_time(Duration::from_millis(100))
        // We must reduce the sample size from the default of 100, otherwise
        // Criterion will sometimes override the 1ms + 500ms suggested times
        // and run for much longer.
        .sample_size(10);

    let logger = create_logger();
    logger.register_logger(None).expect("set up logger");

    // TODO(https://fxbug.dev/344980783): keep the old benchmarks to see continuity, but then
    // rename "Tracing" to "Log" and remove the old tracing benchmarks.
    let mut group = c.benchmark_group("fuchsia.diagnostics_log_rust.core");
    set_up_log_write_benchmarks(&mut group, "Tracing");
    set_up_old_log_write_benchmarks(&mut group, "Log");
    group.finish();
}
