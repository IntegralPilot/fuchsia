// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_component::{CreateChildArgs, RealmMarker};
use fidl_fuchsia_component_decl::{Child, ChildRef, CollectionRef, StartupMode};

#[fuchsia::test]
async fn client_integration_test() {
    let realm = fuchsia_component::client::connect_to_protocol::<RealmMarker>().unwrap();
    realm
        .create_child(
            &CollectionRef { name: "test".to_string() },
            &Child {
                name: Some("test".to_string()),
                url: Some("#meta/cpp_elf_receiver.cm".to_string()),
                startup: Some(StartupMode::Lazy),
                ..Default::default()
            },
            CreateChildArgs::default(),
        )
        .await
        .unwrap()
        .unwrap();

    let (_exposed_dir_proxy, exposed_dir_server) = fidl::endpoints::create_proxy();

    // Trying to open the component's dir will fail because we've given it a bad CVF file.
    assert_eq!(
        realm
            .open_exposed_dir(
                &ChildRef { collection: Some("test".to_string()), name: "test".to_string() },
                exposed_dir_server,
            )
            .await
            .unwrap(),
        Err(fidl_fuchsia_component::Error::InstanceCannotResolve)
    );
}
