// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_reader::ArchiveReader;
use fuchsia_component_test::{RealmBuilder, RealmBuilderParams};
use futures::StreamExt;
use log::info;
use remotevol_fuchsia_test_util::{
    get_storage_for_component_instance, open_sysrq_trigger, wait_for_starnix_volume_to_be_mounted,
};

/// The purpose of this test is to ensure that Starnix can still successfully mount if the power
/// gets cut between writing the key file to disk and creating a Starnix volume. It tests this
/// by deleting the Starnix volume between container boots and ensuring that the second boot
/// succeeds.
#[fuchsia::test]
async fn key_file_exists_but_starnix_volume_doesnt() {
    info!("starting realm");
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("key_file")
            .from_relative_url("#meta/kernel_with_container.cm"),
    )
    .await
    .unwrap();
    let realm = builder.build().await.unwrap();

    let storage_admin =
        fuchsia_component::client::connect_to_protocol::<fidl_fuchsia_sys2::StorageAdminMarker>()
            .expect("connect_to_protocol_at_exposed_dir failed");

    let container_data_proxy = get_storage_for_component_instance(
        &format!("realm_builder:{}/debian_container", realm.root.child_name()),
        storage_admin,
    )
    .await;

    let realm_moniker = format!("realm_builder:{}", realm.root.child_name());
    info!(realm_moniker:%; "started");
    let kernel_moniker = format!("{realm_moniker}/kernel");

    let mut kernel_logs = ArchiveReader::logs()
        .select_all_for_component(kernel_moniker.as_str())
        .snapshot_then_subscribe()
        .unwrap();

    // Open sysrq-trigger to start the kernel, then make sure we see its logs.
    let _sysrq = open_sysrq_trigger(&realm).await;
    let first_kernel_log = kernel_logs.next().await.unwrap().unwrap();
    info!(first_kernel_log:?; "receiving logs from starnix kernel now that it's started");

    wait_for_starnix_volume_to_be_mounted().await;

    // Check that both the volume keys were written out to the storage capability.
    let _key_file = fuchsia_fs::directory::read_file(&container_data_proxy, "key_file")
        .await
        .expect("failed to read the key file");

    info!("Destroying realm");
    realm.destroy().await.expect("Failed to destroy realm on first boot");

    // Delete the starnix volume to simulate the power getting cut after the metadata and data key
    // files were created but before the Starnix volume was created.
    let starnix_volume_admin = fuchsia_component::client::connect_to_protocol::<
        fidl_fuchsia_test_fxfs::StarnixVolumeAdminMarker,
    >()
    .expect("fidl_fuchsia_test_fxfs::StarnixVolumeAdminMarker");

    starnix_volume_admin
        .delete()
        .await
        .expect("fidl transport error")
        .expect("failed to delete the starnix volume");

    // Restart the container. Starnix should see the volume keys and call Mount, which should
    // silently create the Starnix volume, since it won't exist.
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
    let kernel_moniker = format!("{realm_moniker}/kernel");

    let mut kernel_logs = ArchiveReader::logs()
        .select_all_for_component(kernel_moniker.as_str())
        .snapshot_then_subscribe()
        .unwrap();

    // Open sysrq-trigger to start the kernel, then make sure we see its logs.
    let _sysrq = open_sysrq_trigger(&realm).await;
    let first_kernel_log = kernel_logs.next().await.unwrap().unwrap();
    info!(first_kernel_log:?; "receiving logs from starnix kernel now that it's started");

    wait_for_starnix_volume_to_be_mounted().await;

    info!("Destroying realm");
    realm.destroy().await.expect("Failed to destroy realm on second boot");
}
