// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust_testing::CapabilityBuilder;
use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_diagnostics::ArchiveAccessorProxy;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};
use futures::StreamExt;
use test_case::test_case;

#[test_case("#meta/logger_rust.cm")]
#[test_case("#meta/logger_cpp.cm")]
#[fuchsia::test]
async fn log_is_received(logger_url: &str) {
    let builder = RealmBuilder::new().await.unwrap();

    builder
        .add_capability(CapabilityBuilder::dictionary().name("diagnostics").build())
        .await
        .unwrap();

    let archivist = builder
        .add_child(
            "archivist",
            "archivist-for-embedding#meta/archivist-for-embedding.cm",
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(&archivist)
                .to(Ref::dictionary(Ref::self_(), "diagnostics")),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.diagnostics.ArchiveAccessor"))
                .from(Ref::dictionary(&archivist, "diagnostics-accessors"))
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("diagnostics"))
                .from(Ref::parent())
                .to(&archivist),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::event_stream("capability_requested"))
                .from(Ref::parent())
                .to(&archivist),
        )
        .await
        .unwrap();

    let logger =
        builder.add_child("logger", logger_url, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("diagnostics"))
                .from(Ref::self_())
                .to(&logger),
        )
        .await
        .unwrap();

    let instance = builder.build().await.unwrap();
    let accessor =
        instance.root.connect_to_protocol_at_exposed_dir::<ArchiveAccessorProxy>().unwrap();
    let mut reader = ArchiveReader::logs();
    reader.with_archive(accessor);
    let (mut stream, _errors) = reader.snapshot_then_subscribe().unwrap().split_streams();

    let mut found = false;
    while let Some(log) = stream.next().await {
        if log.msg().unwrap() == "Hello, Archivist!" {
            found = true;
            break;
        }
    }
    assert!(found, "didn't find expected log message");
}
