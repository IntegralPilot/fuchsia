// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_diagnostics::ArchiveAccessorProxy;
use fidl_fuchsia_driver_test::RealmArgs;
use fuchsia_component::client;
use fuchsia_component::client::connect;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::StreamExt;

#[fuchsia::test]
async fn log_is_received() {
    let builder = RealmBuilder::new().await.unwrap();
    builder.driver_test_realm_setup().await.unwrap();
    let instance = builder.build().await.unwrap();
    instance
        .driver_test_realm_start(RealmArgs {
            root_driver: Some("#meta/logger_driver.cm".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    // This is a static child instead of part of the driver test realm because it's not trivial to
    // embed archivist or intercept the default diagnostics route in a driver test realm.
    let archivist_exposed = client::open_childs_exposed_directory("archivist", None).await.unwrap();
    let accessor = connect::connect_to_named_protocol_at_dir_root::<ArchiveAccessorProxy>(
        &archivist_exposed,
        "diagnostics-accessors/fuchsia.diagnostics.ArchiveAccessor",
    )
    .unwrap();
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
