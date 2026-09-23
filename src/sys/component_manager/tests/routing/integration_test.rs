// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints;
use fidl_fidl_test_components as ftest;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use fuchsia_component::client;

#[fuchsia::test]
async fn routing() {
    let realm = client::connect_to_protocol::<fcomponent::RealmMarker>()
        .expect("could not connect to Realm service");

    // Bind to `echo_client`, causing it to execute.
    let child_ref = fdecl::ChildRef { name: "echo_client".to_string(), collection: None };
    let (exposed_dir, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
    realm
        .open_exposed_dir(&child_ref, server_end)
        .await
        .unwrap_or_else(|e| panic!("open_exposed_dir failed: {:?}", e))
        .unwrap_or_else(|e| panic!("failed to open child exposed dir: {:?}", e));
    let trigger = client::connect_to_protocol_at_dir_root::<ftest::TriggerMarker>(&exposed_dir)
        .expect("failed to open trigger service");
    let out = trigger.run().await.unwrap_or_else(|e| panic!("trigger failed: {:?}", e));
    assert_eq!(out, "Triggered");
}
