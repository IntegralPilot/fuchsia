// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_reader::ArchiveReader;
use fidl::endpoints::create_proxy;
use fuchsia_async::DurationExt;
use fuchsia_component_test::RealmInstance;
use log::info;
use {fidl_fuchsia_io as fio, fuchsia_async as fasync};

pub const PROGRAM_COLLECTION: &str = "debian_programs";

pub async fn open_sysrq_trigger(realm: &RealmInstance) -> fio::FileProxy {
    info!("opening sysrq trigger");
    fuchsia_fs::directory::open_file(
        realm.root.get_exposed_dir(),
        "/fs_root/proc/sysrq-trigger",
        fuchsia_fs::PERM_WRITABLE,
    )
    .await
    .unwrap()
}

pub async fn is_starnix_volume_mounted() -> Option<bool> {
    let test_fxfs_inspect =
        ArchiveReader::inspect().select_all_for_component("test-fxfs").snapshot().await.unwrap();
    if test_fxfs_inspect.len() == 0 {
        return None;
    }
    let payload = test_fxfs_inspect[0].payload.as_ref().unwrap();
    // Fxfs won't export the GUID property if the volume is locked (which is the case when an
    // encrypted volume is unmounted).
    if let Some(child) = payload.get_child_by_path(&["stores", "test_fxfs_user_volume"]) {
        Some(child.get_property("guid").is_some())
    } else {
        None
    }
}

pub async fn wait_for_starnix_volume_to_be_mounted() {
    loop {
        if is_starnix_volume_mounted().await == Some(true) {
            return;
        }
        fasync::Timer::new(fasync::MonotonicDuration::from_millis(100).after_now()).await;
    }
}

pub async fn get_storage_for_component_instance(
    moniker_prefix: &str,
    storage_admin: fidl_fuchsia_sys2::StorageAdminProxy,
) -> fio::DirectoryProxy {
    let (storage_user_iterator, storage_user_iterator_server_end) =
        create_proxy::<fidl_fuchsia_sys2::StorageIteratorMarker>();
    storage_admin
        .list_storage_in_realm(".", storage_user_iterator_server_end)
        .await
        .unwrap()
        .unwrap();
    let mut matching_storage_users = vec![];
    loop {
        let chunk = storage_user_iterator.next().await.unwrap();
        if chunk.is_empty() {
            break;
        }
        let mut matches: Vec<String> = chunk
            .into_iter()
            .filter(|moniker| {
                info!("moniker is {moniker}");
                moniker.starts_with(moniker_prefix)
            })
            .collect();
        matching_storage_users.append(&mut matches);
    }
    assert!(!matching_storage_users.is_empty());
    let (proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
    storage_admin
        .open_storage(matching_storage_users.first().unwrap(), server_end.into_channel().into())
        .await
        .unwrap()
        .unwrap();
    proxy
}
