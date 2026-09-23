// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use diagnostics_log::LogEvent;
use diagnostics_log_encoding::encode::{Encoder, EncoderOpts, WriteEventParams};
use diagnostics_log_encoding::parse::{parse_argument, parse_record};
use diagnostics_log_encoding::{Argument, Value};
use fidl_fuchsia_logger::MAX_DATAGRAM_LEN_BYTES;
use fuchsia_criterion::{FuchsiaCriterion, criterion};
use log::kv::ToValue;
use std::fmt;
use std::io::Cursor;
use std::time::Duration;

mod common;

fn bench_argument(
    value: impl Into<Value<'static>>,
) -> impl FnMut(&mut criterion::Bencher<'_>) + 'static {
    let value = value.into();
    move |b: &mut criterion::Bencher<'_>| {
        let arg = Argument::new("foo", value.clone());
        let buffer = [0u8; MAX_DATAGRAM_LEN_BYTES as usize];
        let mut encoder = Encoder::new(Cursor::new(buffer), EncoderOpts::default());
        let _ = encoder.write_argument(arg);
        b.iter(|| parse_argument(encoder.inner().get_ref()))
    }
}

const ENCODE_SIZE: usize = 4096;

fn write(
    message: fmt::Arguments<'static>,
    kvs: &[(&str, log::kv::Value<'_>)],
) -> Encoder<Cursor<[u8; ENCODE_SIZE]>> {
    let key_values = Some(kvs);
    let record = log::Record::builder()
        .level(log::Level::Info)
        .key_values(&key_values)
        .args(message)
        .build();
    let buffer = [0u8; ENCODE_SIZE];
    let mut encoder = Encoder::new(Cursor::new(buffer), EncoderOpts::default());
    assert_matches!(
        encoder.write_event(WriteEventParams {
            event: LogEvent::new(&record),
            tags: &["some-tag"],
            metatags: std::iter::empty(),
            pid: *common::PROCESS_ID,
            tid: *common::THREAD_ID,
            dropped: 1,
        }),
        Ok(())
    );
    encoder
}

fn setup_read_event_benchmarks(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
) {
    let _ = group.bench_function("Decoder/ReadEvent/AllArguments", |b| {
        let encoder = write(
            format_args!("this is a log emitted from the benchmark"),
            &[
                ("tag", "logbench".to_value()),
                ("boolean", true.to_value()),
                ("float", 1234.5678.to_value()),
                ("int", (-123456).to_value()),
                ("string", "foobarbaz".to_value()),
                ("uint", 123456.to_value()),
            ],
        );
        b.iter(|| parse_record(encoder.inner().get_ref()).unwrap())
    });
    let _ = group.bench_function("Decoder/ReadEvent/NoArguments", |b| {
        let encoder = write(format_args!("this is a log emitted from the benchmark"), &[]);
        b.iter(|| parse_record(encoder.inner().get_ref()).unwrap())
    });
    let _ = group.bench_function("Decoder/ReadEvent/MessageAsString", |b| {
        let encoder = write(
            format_args!(
                "this is a log emitted from the benchmark boolean=true int=98765 string=foobarbaz"
            ),
            &[],
        );
        b.iter(|| parse_record(encoder.inner().get_ref()).unwrap())
    });
    let _ = group.bench_function("Decoder/ReadEvent/MessageWithSomeArguments", |b| {
        let encoder = write(
            format_args!("this is a log emitted from the benchmark"),
            &[
                ("boolean", true.to_value()),
                ("int", 98765.to_value()),
                ("string", "foobarbaz".to_value()),
            ],
        );
        b.iter(|| parse_record(encoder.inner().get_ref()).unwrap())
    });
}

fn main() {
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut criterion::Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(Duration::from_millis(1))
        .measurement_time(Duration::from_millis(100))
        .sample_size(100);

    let mut group = c.benchmark_group("fuchsia.diagnostics_log_rust.decoding");
    let _ = group.bench_function("Decoder/Argument/Boolean", bench_argument(true));
    let _ = group.bench_function("Decoder/Argument/Floating", bench_argument(1234.5678_f64));
    let _ = group.bench_function("Decoder/Argument/UnsignedInt", bench_argument(12345_u64));
    let _ = group.bench_function("Decoder/Argument/SignedInt", bench_argument(-12345_i64));

    for size in [16, 128, 256, 512, 1024, 32000] {
        let _ = group.bench_function(
            format!("Decoder/Argument/Text/{size}"),
            bench_argument((*common::PLACEHOLDER_TEXT).get(..size).unwrap()),
        );
    }

    setup_read_event_benchmarks(&mut group);
    group.finish();
}
