// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::create_proxy;
use fidl_fuchsia_component::RealmMarker;
use fidl_fuchsia_component_decl::ChildRef;
use fidl_test_ping::PingMarker;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_dir_root};

#[fuchsia::test]
async fn base_resolver_test() {
    let realm =
        connect_to_protocol::<RealmMarker>().expect("failed to connect to fuchsia.component.Realm");
    let (exposed_dir, server_end) = create_proxy();
    realm
        .open_exposed_dir(&ChildRef { name: "base-component".into(), collection: None }, server_end)
        .await
        .expect("failed to call open_exposed_dir FIDL")
        .expect("failed to open exposed dir of child");
    let ping = connect_to_protocol_at_dir_root::<PingMarker>(&exposed_dir)
        .expect("failed to connect to Ping protocol");
    assert_eq!(ping.ping("ping").await.expect("Ping FIDL call failed"), "ping pong");
}

#[fuchsia::test]
async fn base_resolver_resolves_subpackages() {
    let realm =
        connect_to_protocol::<RealmMarker>().expect("failed to connect to fuchsia.component.Realm");
    let (exposed_dir, server_end) = create_proxy();
    realm
        .open_exposed_dir(
            &ChildRef { name: "base-superpackage-component".into(), collection: None },
            server_end,
        )
        .await
        .expect("failed to call open_exposed_dir FIDL")
        .expect("failed to open exposed dir of child");
    let ping = connect_to_protocol_at_dir_root::<PingMarker>(&exposed_dir)
        .expect("failed to connect to Ping protocol");
    assert_eq!(
        ping.ping("ping").await.expect("Ping FIDL call failed"),
        "forwarded ping and returned ping pong"
    );
}
