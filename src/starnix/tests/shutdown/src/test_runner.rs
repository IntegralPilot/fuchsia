// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_sys2 as fsys2;
use fuchsia_async as fasync;
use fuchsia_component_test::{
    Capability, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use futures::StreamExt;
use zx;

async fn build_realm() -> RealmInstance {
    let builder =
        RealmBuilder::with_params(RealmBuilderParams::new().from_relative_url("#meta/realm.cm"))
            .await
            .expect("created");

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsys2::LifecycleControllerMarker>())
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    builder.build().await.unwrap()
}

#[fuchsia::test]
async fn test_shutdown_with_zombie() {
    let realm_instance = build_realm().await;
    let lifecycle_controller: fsys2::LifecycleControllerProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    // Run the test program to generate an unreaped zombie.
    let (_, binder_server) = fidl::endpoints::create_endpoints();
    lifecycle_controller.start_instance("./zombie_shutdown", binder_server).await.unwrap().unwrap();
    wait_for_zombie_signal(&realm_instance).await;

    let stop_future = async {
        lifecycle_controller
            .stop_instance("./zombie_shutdown")
            .await
            .map_err(|e| anyhow!("FIDL error: {:?}", e))?
            .map_err(|e| anyhow!("Lifecycle error: {:?}", e))
    };

    let result = fasync::TimeoutExt::on_timeout(
        stop_future,
        fasync::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5)),
        || Err(anyhow!("Container shutdown hung")),
    )
    .await;

    assert!(result.is_ok(), "Container failed to stop gracefully: {:?}", result);
}

// Wait until the spawned zombie process signals that it is ready via stdout.
async fn wait_for_zombie_signal(realm_instance: &RealmInstance) {
    let realm_moniker = format!("realm_builder:{}", realm_instance.root.child_name());
    let kernel_moniker = format!("{realm_moniker}/kernel");

    let mut logs = ArchiveReader::logs()
        .select_all_for_component(kernel_moniker.as_str())
        .snapshot_then_subscribe()
        .expect("failed to subscribe to kernel logs");

    while let Some(log) = logs.next().await {
        let log = log.expect("failed to read log from stream");
        if let Some(msg) = log.msg() {
            if msg.contains("[ZOMBIE_READY]") {
                break;
            }
        }
    }
}
