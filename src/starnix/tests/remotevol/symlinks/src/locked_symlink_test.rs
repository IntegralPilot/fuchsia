// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{EventStream, ExitStatus, Stopped};
use component_events::matcher::EventMatcher;
use fidl_fuchsia_component::{CreateChildArgs, RealmProxy};
use fidl_fuchsia_component_decl::{Child, CollectionRef, StartupMode};
use fuchsia_component_test::{RealmBuilder, RealmBuilderParams, RealmInstance};
use log::info;
use remotevol_fuchsia_test_util::{PROGRAM_COLLECTION, wait_for_starnix_volume_to_be_mounted};

/// This test ensures that when you read a locked symlink, unlock the symlink, and then re-read the
/// symlink, that the target path changes appropriately. When a directory is encrypted initially,
/// its starting state is unlocked because in order to encrypt a directory, a user must have
/// already added the key it wants to use to encrypt it. Rebooting Starnix allows us to test locked
/// encrypted directories.
/// TODO(https://fxbug.dev/358420498): This test can be written as an fscrypt syscall test if key
/// removal support is added.
#[fuchsia::test]
async fn read_locked_symlink_and_then_unlocked_symlink() {
    let mut events = EventStream::open().await.unwrap();
    info!("starting realm");
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("key_file")
            .from_relative_url("#meta/kernel_with_container.cm"),
    )
    .await
    .unwrap();
    let realm: RealmInstance = builder.build().await.unwrap();

    let realm_moniker = format!("realm_builder:{}", realm.root.child_name());
    info!(realm_moniker:%; "started");

    // Start the debian container
    realm.root.connect_to_binder().expect("failed to connect to binder");

    wait_for_starnix_volume_to_be_mounted().await;

    info!("starting create_encrypted_symlink");
    let test_realm: RealmProxy = realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    test_realm
        .create_child(
            &CollectionRef { name: PROGRAM_COLLECTION.to_string() },
            &Child {
                name: Some("create_encrypted_symlink".to_string()),
                url: Some("#meta/create_encrypted_symlink.cm".to_string()),
                startup: Some(StartupMode::Lazy),
                ..Default::default()
            },
            CreateChildArgs::default(),
        )
        .await
        .unwrap()
        .unwrap();

    let create_symlink_stopped = EventMatcher::ok()
        .moniker_regex(&format!("realm_builder:.+/{PROGRAM_COLLECTION}:create_encrypted_symlink"))
        .wait::<Stopped>(&mut events)
        .await
        .unwrap();
    assert_eq!(
        create_symlink_stopped.result().unwrap().status,
        ExitStatus::Clean,
        "create_encrypted_symlink must exit cleanly"
    );

    info!("Destroying realm");
    realm.destroy().await.expect("Failed to destroy realm on first boot");

    let mut events = EventStream::open().await.unwrap();
    info!("starting realm");
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("key_file")
            .from_relative_url("#meta/kernel_with_container.cm"),
    )
    .await
    .unwrap();
    let realm = builder.build().await.unwrap();

    let realm_moniker = format!("realm_builder:{}", realm.root.child_name());
    info!(realm_moniker:%; "started");

    // Starting the debian container
    realm.root.connect_to_binder().expect("failed to connect to binder");

    wait_for_starnix_volume_to_be_mounted().await;

    info!("starting read_encrypted_symlink");
    let test_realm: RealmProxy = realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    test_realm
        .create_child(
            &CollectionRef { name: PROGRAM_COLLECTION.to_string() },
            &Child {
                name: Some("read_encrypted_symlink".to_string()),
                url: Some("#meta/read_encrypted_symlink.cm".to_string()),
                startup: Some(StartupMode::Lazy),
                ..Default::default()
            },
            CreateChildArgs::default(),
        )
        .await
        .unwrap()
        .unwrap();

    let read_symlink_stopped = EventMatcher::ok()
        .moniker_regex(&format!("realm_builder:.+/{PROGRAM_COLLECTION}:read_encrypted_symlink"))
        .wait::<Stopped>(&mut events)
        .await
        .unwrap();
    assert_eq!(
        read_symlink_stopped.result().unwrap().status,
        ExitStatus::Clean,
        "read_encrypted_symlink must exit cleanly"
    );

    info!("Destroying realm");
    realm.destroy().await.expect("Failed to destroy realm on second boot");
}
