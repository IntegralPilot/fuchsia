// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_data::{
    BuilderArgs, InspectDataBuilder, InspectHandleName, LogsDataBuilder, LogsField, LogsProperty,
    Severity, Timestamp,
};
use diagnostics_hierarchy::{DiagnosticsHierarchy, Property};
use fidl_fuchsia_diagnostics::{DataType, Format};
use fuchsia_async as fasync;
use fuchsia_criterion::FuchsiaCriterion;
use fuchsia_criterion::criterion::{self, Criterion};
use futures::{StreamExt, stream};

use archivist_lib::constants::FORMATTED_CONTENT_CHUNK_SIZE_TARGET;
use archivist_lib::formatter::{JsonPacketSerializer, SerializedVmo};
use std::convert::TryInto;
use std::time::Duration;

fn bench_serialization(b: &mut criterion::Bencher<'_>, n: usize, m: usize, format: Format) {
    let mut children = vec![];
    for i in 0..n {
        let mut properties = vec![];
        for j in 0..m {
            properties.push(Property::Uint(j.to_string(), (5 * i + j) as u64));
        }
        children.push(DiagnosticsHierarchy::new(i.to_string(), properties, vec![]));
    }
    let data = InspectDataBuilder::new(
        "bench".try_into().unwrap(),
        "fuchsia-pkg://fuchsia.com/testing#meta/bench.cm",
        Timestamp::from_nanos(1),
    )
    .with_hierarchy(DiagnosticsHierarchy::new("root", vec![], children))
    .with_name(InspectHandleName::filename("fuchsia.inspect.Tree"))
    .build();
    b.iter(|| {
        let _ = std::hint::black_box(SerializedVmo::serialize(&data, DataType::Inspect, format));
    });
}

fn bench_json_packet_serializer(b: &mut criterion::Bencher<'_>, total_logs: u64) {
    let logs = (0u64..total_logs)
        .map(|i| {
            LogsDataBuilder::new(BuilderArgs {
                component_url: Some(
                    format!("fuchsia-pkg://fuchsia.com/testing#meta/bench-{i}.cm").into(),
                ),
                moniker: format!("moniker-{i}").as_str().try_into().unwrap(),
                severity: Severity::Info,
                timestamp: Timestamp::from_nanos(i as i64),
            })
            .set_message(format!("Benching #{i}"))
            .set_file(format!("bench-{i}.rs"))
            .set_line(400 + i)
            .set_pid(100 + i)
            .set_tid(2000 + i)
            .set_dropped(2)
            .add_tag("benches")
            .add_key(LogsProperty::Uint(LogsField::Other("id".to_string()), i))
            .build()
        })
        .collect::<Vec<_>>();

    let mut executor = fasync::LocalExecutor::default();
    b.iter(|| {
        let logs_for_fut = logs.clone();
        executor.run_singlethreaded(async move {
            let mut stream = JsonPacketSerializer::new_without_stats(
                FORMATTED_CONTENT_CHUNK_SIZE_TARGET,
                stream::iter(logs_for_fut),
            );
            while let Some(res) = stream.next().await {
                let _ = std::hint::black_box(res);
            }
        });
    });
}

fn main() {
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(Duration::from_secs(2))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20);

    // The following benchmarks measure the performance of SerializedVmo and JsonPacketSerializer.
    // This is fundamental when serializing responses in the archivist.

    let mut group = c.benchmark_group("fuchsia.archivist");
    let _ = group.bench_function("Formatter/JsonPacketSerializer/5", move |b| {
        bench_json_packet_serializer(b, 5);
    });
    let _ = group.bench_function("Formatter/JsonPacketSerializer/5k", move |b| {
        bench_json_packet_serializer(b, 5000);
    });
    let _ = group.bench_function("Formatter/JsonPacketSerializer/16k", move |b| {
        bench_json_packet_serializer(b, 16000);
    });

    // Benchmark the time needed to serialize a DiagnosticsData into a SerializedVmo with N children
    // and M properties each, for Json and Cbor.
    for (format_name, format) in [("JsonString", Format::Json), ("CborData", Format::Cbor)].iter() {
        for (size_name, n, m) in
            [("5x5", 5, 5), ("10x10", 10, 10), ("100x100", 100, 100), ("1000x500", 1000, 500)]
                .iter()
        {
            let label = format!("Formatter/{format_name}/Fill/{size_name}");
            let _ = group.bench_function(&label, move |b| {
                bench_serialization(b, *n, *m, *format);
            });
        }
    }

    group.finish();
}
