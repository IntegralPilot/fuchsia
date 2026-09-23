// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_log::{Publisher, PublisherOptions};
use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_diagnostics::{ArchiveAccessorMarker, Format};
use fidl_fuchsia_logger::LogSinkMarker;
use fuchsia_async as fasync;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};
use fuchsia_criterion::FuchsiaCriterion;
use fuchsia_criterion::criterion::{self, Criterion};
use futures::{StreamExt, future};
use log::{Level, Log, Record};
use std::time::Duration;

const ARCHIVIST_URL: &str = "archivist-for-embedding#meta/archivist-for-embedding.cm";

async fn create_realm() -> RealmBuilder {
    let builder = RealmBuilder::new().await.expect("create realm builder");
    let archivist = builder
        .add_child("archivist", ARCHIVIST_URL, ChildOptions::new().eager())
        .await
        .expect("add child archivist");
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<LogSinkMarker>())
                .capability(
                    Capability::protocol_by_name("fuchsia.tracing.provider.Registry").optional(),
                )
                .capability(Capability::event_stream("capability_requested"))
                .from(Ref::parent())
                .to(&archivist),
        )
        .await
        .expect("added routes from parent to archivist");
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<LogSinkMarker>())
                .from(&archivist)
                .to(Ref::parent()),
        )
        .await
        .expect("added routes from archivist to parent");
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<ArchiveAccessorMarker>())
                .from(Ref::dictionary(&archivist, "diagnostics-accessors"))
                .to(Ref::parent()),
        )
        .await
        .expect("routed ArchiveAccessor from archivist to parent");
    builder
}

fn bench_write_read_log(b: &mut criterion::Bencher<'_>, format: Format) {
    let mut executor = fasync::LocalExecutor::default();
    let instance = executor.run_singlethreaded(async move {
        let builder = create_realm().await;
        builder.build().await.expect("create instance")
    });
    let log_sink_client = instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    let accessor_proxy = instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    let mut reader = ArchiveReader::logs();
    reader.with_archive(accessor_proxy).with_format(format);

    let options = PublisherOptions::default().tags(&["some-tag"]).use_log_sink(log_sink_client);
    let publisher = Publisher::new_sync(options).unwrap();
    let (stream, _errors) =
        reader.snapshot_then_subscribe().expect("subscribed to logs").split_streams();

    // Emit a placeholder log and skip it to ensure that any log coming after this, will be the
    // one we care about for benchmarking purposes.
    publisher
        .log(&Record::builder().args(format_args!("placeholder log")).level(Level::Info).build());
    let mut stream =
        stream.skip_while(|data| future::ready(!format!("{:?}", data).contains("placeholder log")));
    executor.run_singlethreaded(stream.next());

    b.iter(|| {
        publisher.log(
            &Record::builder()
                .args(format_args!("log from the benchmark"))
                .level(Level::Info)
                .build(),
        );
        executor.run_singlethreaded(stream.next());
    });
}

fn main() {
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(Duration::from_millis(1))
        .measurement_time(Duration::from_millis(100))
        .sample_size(10);

    // These benchmarks measure the time it takes to emit a log and get it from the ArchieAccessor.
    let mut group = c.benchmark_group("fuchsia.archivist");
    let _ = group.bench_function("LoggingE2E/WriteReadLog/Json", move |b| {
        bench_write_read_log(b, Format::Json);
    });
    let _ = group.bench_function("LoggingE2E/WriteReadLog/Fxt", move |b| {
        bench_write_read_log(b, Format::LegacyFxt);
    });

    group.finish();
}
