// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for handling of key shredding.  These are in a separate test package so they can allow
//! error logs, which occur due to apparent corruption if keys are rotated and attempted to be
//! reused.

pub mod config;
use assert_matches::assert_matches;
use config::{data_fs_spec, data_fs_type, new_builder, volumes_spec};
use fidl_fuchsia_fshost::AdminProxy;
use fidl_fuchsia_fxfs as _;
use fidl_fuchsia_io as fio;
use fs_management::filesystem::Filesystem;
use fshost_test_fixture::disk_builder::{DataSpec, Disk};
use fuchsia_async as _;
use futures::FutureExt as _;
use log;
use test_vmo_backed_block_server::{VmoBackedServer, VmoBackedServerConnector};

#[fuchsia::test]
async fn shred_data_volume_when_mounted_keymint() {
    // This test verifies that fshost correctly rotates keymint's keys when ShredDataVolume is
    // called.  It works by restoring the old keybag (which is also shredded during FDR) and
    // verifying that the old keys cannot be replayed, triggering a second FDR.
    let keymint = std::sync::Arc::new(fake_keymint::FakeKeymint::default());

    let mut builder = new_builder()
        .with_crypt_policy(crypt_policy::Policy::Keymint)
        .with_keymint_instance(keymint.clone());
    let data_spec = DataSpec { crypt_policy: crypt_policy::Policy::Keymint, ..data_fs_spec() };
    builder.with_disk().format_volumes(volumes_spec()).format_data(data_spec);
    let fixture = builder.build().await;

    fuchsia_fs::directory::open_file(
        &fixture.dir("data", fio::PERM_READABLE | fio::PERM_WRITABLE),
        "test-file",
        fio::Flags::FLAG_MAYBE_CREATE,
    )
    .await
    .expect("open_file failed");

    let disk = fixture.tear_down().await.unwrap();

    // Manually mount fxfs and backup the keybag.
    let (vmo, _) = disk.into_vmo_and_type_guid().await;
    let keybag_backup = {
        let server = std::sync::Arc::new(
            VmoBackedServer::from_vmo(512, vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                .unwrap(),
        );
        let connector = VmoBackedServerConnector::new(server);
        let fs = Filesystem::new(connector, fs_management::Fxfs::default());
        let vol = fs.serve_multi_volume().await.unwrap();
        let unencrypted = vol
            .open_volume("unencrypted", fidl_fuchsia_fs_startup::MountOptions::default())
            .await
            .unwrap();
        let keys_dir =
            fuchsia_fs::directory::open_directory(unencrypted.root(), "keys", fio::PERM_READABLE)
                .await
                .unwrap();

        // For Keymint policy, the file is "keymint.0".
        let file = fuchsia_fs::directory::open_file(&keys_dir, "keymint.0", fio::PERM_READABLE)
            .await
            .unwrap();
        fuchsia_fs::file::read(&file).await.expect("failed to read keybag")
    };

    // Start fshost again and shred.
    let fixture = new_builder()
        .with_disk_from(Disk::Prebuilt(
            vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            None,
        ))
        .with_crypt_policy(crypt_policy::Policy::Keymint)
        .with_keymint_instance(keymint.clone())
        .build()
        .await;
    fixture.check_fs_type("data", data_fs_type()).await;

    let admin: AdminProxy = fixture
        .realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("connect_to_protcol_at_exposed_dir failed");
    admin
        .shred_data_volume()
        .await
        .expect("shred_data_volume FIDL failed")
        .expect("shred_data_volume failed");

    let disk = fixture.tear_down().await.unwrap();

    // Manually mount fxfs and restore the keybag.
    let (vmo, _) = disk.into_vmo_and_type_guid().await;
    {
        let server = std::sync::Arc::new(
            VmoBackedServer::from_vmo(512, vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                .unwrap(),
        );
        let connector = VmoBackedServerConnector::new(server);
        let fs = Filesystem::new(connector, fs_management::Fxfs::default());
        let vol = fs.serve_multi_volume().await.unwrap();
        let unencrypted = vol
            .open_volume("unencrypted", fidl_fuchsia_fs_startup::MountOptions::default())
            .await
            .unwrap();

        // Re-create keys dir if missing.
        let keys_dir = fuchsia_fs::directory::create_directory(
            unencrypted.root(),
            "keys",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .unwrap();

        let file = fuchsia_fs::directory::open_file(
            &keys_dir,
            "keymint.0",
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE,
        )
        .await
        .unwrap();
        fuchsia_fs::file::write(&file, &keybag_backup).await.unwrap();
    }

    // Start fshost again and verify /data is mounted, but it was wiped (test-file should not
    // exist).
    let mut fixture = new_builder()
        .with_disk_from(Disk::Prebuilt(vmo, None))
        .with_crypt_policy(crypt_policy::Policy::Keymint)
        .with_keymint_instance(keymint)
        .build()
        .await;
    assert_matches!(
        fuchsia_fs::directory::open_file(
            &fixture.dir("data", fio::PERM_READABLE),
            "test-file",
            fio::PERM_READABLE,
        )
        .await
        .expect_err("open_file failed"),
        fuchsia_fs::node::OpenError::OpenError(zx::Status::NOT_FOUND)
    );

    // We expect crash reports because fshost thought that the data was corrupted.
    fixture.wait_for_crash_reports(1, "fxfs", "fuchsia-fxfs-unseal-error").await;

    fixture.tear_down().await;
}

/// Tests the critical race window during an inline key upgrade where a power failure occurs
/// immediately after the new upgraded blob is committed to disk, but before the old key can be
/// successfully deleted from the hardware KeyMint TEE.
///
/// If this race condition occurs, fshost writes an `old_blob` marker to the persistence file. Upon
/// the next boot, the `load_keymint_data` mechanism (tested in `upgrade_recovery` scenarios) is
/// responsible for reading this marker and resuming the paused deletion.
///
/// This test simulates the entire lifecycle:
/// 1. Triggering the inline upgrade.
/// 2. Simulating a power failure exactly when `DeleteSealingKey` is called by dropping the builder.
/// 3. Relaunching fshost using the identical disk state and identical KeyMint hardware state.
/// 4. Verifying that the second boot successfully mounts the filesystem and that the recovery
///    process gracefully handles any persistent deletion errors from the TEE.
#[fuchsia::test]
async fn data_formatted_keymint_upgrade_on_unseal_with_power_failure() {
    let mut builder = new_builder().with_crypt_policy(crypt_policy::Policy::Keymint);
    let shared_keymint = builder.keymint();
    let old_blob = shared_keymint.generate_static_sealing_key(b"fuchsia");
    log::info!("Generated old_blob: {:?}", old_blob);
    let disk = {
        builder.with_disk().format_volumes(volumes_spec()).format_data(DataSpec {
            crypt_policy: crypt_policy::Policy::Keymint,
            ..data_fs_spec()
        });
        let fixture = builder.build().await;
        fixture.tear_down().await.unwrap()
    };

    shared_keymint.bump_epoch();
    // Clone keymint so that fixture teardown doesn't mess with it.
    let shared_keymint = shared_keymint.clone();

    let (hang_done_tx, hang_done_rx) = futures::channel::oneshot::channel();
    let hang_done_tx = std::sync::Arc::new(std::sync::Mutex::new(Some(hang_done_tx)));
    let old_blob_clone = old_blob.clone();
    shared_keymint.set_delete_hook(move |blob| {
        let hang_done_tx = hang_done_tx.clone();
        let old_blob = old_blob_clone.clone();
        async move {
            log::info!("Delete hook triggered for blob: {:?}", blob);
            if blob == old_blob {
                if let Some(tx) = hang_done_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                log::info!("Hanging DeleteSealingKey call for old_blob");
                std::future::pending().await
            } else {
                None
            }
        }
        .boxed()
    });

    let disk = {
        log::info!("Starting first boot fshost");
        let fixture = new_builder()
            .with_crypt_policy(crypt_policy::Policy::Keymint)
            .with_disk_from(disk)
            .with_keymint_instance(shared_keymint.clone())
            .build()
            .await;

        // Wait until the delete call is initiated and hanging.
        log::info!("Waiting for delete hook to be triggered");
        hang_done_rx.await.unwrap();

        // The old key should still be in KeyMint since the delete is hanging.
        assert!(shared_keymint.has_key_blob(&old_blob));
        log::info!("Verified old_blob still exists in KeyMint before crash");

        // Now simulate a "crash" by taking a snapshot of the disk before the upgrade can be
        // committed or deleted. Notably, the new keys should have been flushed to disk by now.
        log::info!("Simulating crash/teardown");
        let disk = if let Some(Disk::Prebuilt(vmo, guid)) = &fixture.main_disk {
            let size = vmo.get_size().unwrap();
            let snapshot = zx::Vmo::create(size).unwrap();
            let mut buf = vec![0u8; size as usize];
            vmo.read(&mut buf, 0).unwrap();
            snapshot.write(&buf, 0).unwrap();
            Disk::Prebuilt(snapshot, *guid)
        } else {
            panic!("Expected prebuilt disk");
        };

        // Clean up the fixture that we left hanging
        fixture.tear_down().await;

        disk
    };

    // Verify the old blob was preserved despite the crash simulation.
    assert!(shared_keymint.has_key_blob(&old_blob));
    log::info!("Verified old_blob still exists in KeyMint after crash");

    let mut second_builder = new_builder()
        .with_crypt_policy(crypt_policy::Policy::Keymint)
        .with_disk_from(disk)
        .with_keymint_instance(shared_keymint.clone());

    // On second boot, unseal succeeds natively (we expect no KeyRequiresUpgrade this time),
    // and the delete operation is retried and succeeds.
    second_builder.keymint().set_delete_hook(|_| async { None }.boxed());

    log::info!("Starting second boot fshost");
    let fixture = second_builder.build().await;
    fixture.check_fs_type("data", data_fs_type()).await;

    // Confirm that fshost mounted the data volume smoothly despite the interrupted upgrade state.
    fixture.check_fs_type("blob", config::blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    // Verify the old blob was deleted during recovery setup, before any teardowns occur.
    assert!(!shared_keymint.has_key_blob(&old_blob));

    let disk = fixture.tear_down().await.unwrap();

    // Boot a third time to verify it doesn't try to delete again.
    let mut third_builder = new_builder()
        .with_crypt_policy(crypt_policy::Policy::Keymint)
        .with_disk_from(disk)
        .with_keymint_instance(shared_keymint.clone());

    let delete_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let delete_called_clone = delete_called.clone();
    third_builder.keymint().set_delete_hook(move |_| {
        delete_called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        async { None }.boxed()
    });

    log::info!("Starting third boot fshost");
    let fixture = third_builder.build().await;

    // Confirm that fshost mounted the data volume smoothly
    fixture.check_fs_type("blob", config::blob_fs_type()).await;
    fixture.check_fs_type("data", data_fs_type()).await;

    // Delete should not have been called on the third boot because the blob was cleared
    // successfully at the end of the second boot.
    assert!(!delete_called.load(std::sync::atomic::Ordering::SeqCst));

    fixture.tear_down().await.unwrap();
}
