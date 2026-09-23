// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_diagnostics::ArchiveAccessorProxy;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, Ref, Route,
};
use futures::channel::mpsc;
use futures::prelude::*;

// asan variants don't support mallopt()
#[cfg_attr(feature = "variant_asan", ignore)]
#[cfg_attr(feature = "variant_hwasan", ignore)]
#[fuchsia::test]
async fn boot_controller() {
    let params = RealmBuilderParams::new().from_relative_url("#meta/boot_test_realm.cm");
    let builder = RealmBuilder::with_params(params).await.unwrap();
    let (tx, mut rx) = mpsc::unbounded();
    let trigger = builder
        .add_local_child(
            "trigger",
            move |handles| {
                let mut tx = tx.clone();
                async move {
                    let client: fsys::BootControllerProxy = handles.connect_to_protocol().unwrap();
                    client.notify().await.unwrap();
                    tx.send(()).await.unwrap();
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsys::BootControllerMarker>())
                .from(Ref::parent())
                .to(&trigger),
        )
        .await
        .unwrap();
    let cm_realm_instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    cm_realm_instance.start_component_tree().await.unwrap();

    rx.next().await.unwrap();

    // BootController/Notify has responded so the boot timestamp should now be set.
    let archive = cm_realm_instance
        .root
        .connect_to_protocol_at_exposed_dir::<ArchiveAccessorProxy>()
        .unwrap();
    let data = ArchiveReader::inspect()
        .with_archive(archive)
        .add_selector("<component_manager>:root")
        .snapshot()
        .await
        .unwrap();
    assert_eq!(data.len(), 1);
    let hierarchy = data[0].payload.as_ref().unwrap();
    let node = hierarchy.get_child("boot").unwrap();
    let ts = node.get_property("last_memory_purge_timestamp").unwrap().uint().unwrap();
    assert!(ts > 0, "{ts} > 0");
}
