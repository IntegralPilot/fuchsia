// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::BlockMarker;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::FutureExt as _;
use maplit::hashmap;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use test_case::test_case;
use test_vmo_backed_block_server::VmoBackedServer;

const BLOCK_SIZE: u32 = 512;

async fn create_realm(
    server: Arc<VmoBackedServer>,
    is_readonly: bool,
) -> Result<RealmInstance, Error> {
    let builder = RealmBuilder::new().await.expect("Failed to create RealmBuilder");
    let block = builder
        .add_local_child(
            "block",
            move |handles: fuchsia_component_test::LocalComponentHandles| {
                let server = server.clone();
                let scope = vfs::ExecutionScope::new();
                let outgoing = vfs::pseudo_directory! {
                    "block" => vfs::pseudo_directory! {
                        BlockMarker::PROTOCOL_NAME =>
                            vfs::service::host(move |requests| {
                                let server_clone = server.clone();
                                async move {
                                    let _ = server_clone.serve(requests).await;
                                }
                            }),
                    },
                };

                vfs::directory::serve_on(
                    outgoing,
                    fio::PERM_READABLE | fio::PERM_WRITABLE,
                    scope.clone(),
                    handles.outgoing_dir,
                );
                async move {
                    scope.wait().await;
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    let manifest = if is_readonly { "#meta/ext4_readonly.cm" } else { "#meta/ext4_server.cm" };
    let ext4_server =
        builder.add_child("ext4_server", manifest, ChildOptions::new()).await.unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::directory("block").path("/block").rights(fio::R_STAR_DIR))
                .from(&block)
                .to(&ext4_server),
        )
        .await
        .unwrap();
    let rights = if is_readonly { fio::R_STAR_DIR } else { fio::RW_STAR_DIR };
    builder
        .add_route(
            Route::new()
                .capability(Capability::directory("root").rights(rights))
                .from(&ext4_server)
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("diagnostics"))
                .from(Ref::parent())
                .to(&ext4_server),
        )
        .await
        .unwrap();

    builder.build().await.map_err(Into::into)
}

#[test_case(
    "/pkg/data/extents.img",
    hashmap!{
        "largefile".to_string() => "de2cf635ae4e0e727f1e412f978001d6a70d2386dc798d4327ec8c77a8e4895d".to_string(),
        "smallfile".to_string() => "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03".to_string(),
        "sparsefile".to_string() => "3f411e42c1417cd8845d7144679812be3e120318d843c8c6e66d8b2c47a700e9".to_string(),
        "a/multi/dir/path/within/this/crowded/extents/test/img/empty".to_string() => "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
    };
    "fs with multiple files with multiple extents")]
#[test_case(
    "/pkg/data/1file.img",
    hashmap!{
        "file1".to_string() => "6bc35bfb2ca96c75a1fecde205693c19a827d4b04e90ace330048f3e031487dd".to_string(),
    };
    "fs with one small file")]
#[test_case(
    "/pkg/data/nest.img",
    hashmap!{
        "file1".to_string() => "6bc35bfb2ca96c75a1fecde205693c19a827d4b04e90ace330048f3e031487dd".to_string(),
        "inner/file2".to_string() => "215ca145cbac95c9e2a6f5ff91ca1887c837b18e5f58fd2a7a16e2e5a3901e10".to_string(),
    };
    "fs with a single directory")]
#[fuchsia::test]
async fn ext4_server_mounts_block_device_from_namespace(
    ext4_path: &str,
    file_hashes: HashMap<String, String>,
) -> Result<(), Error> {
    let server = Arc::new(VmoBackedServer::from_file(BLOCK_SIZE, ext4_path));

    let realm = create_realm(server, false).await?;

    let fs_root = fuchsia_fs::directory::open_directory(
        realm.root.get_exposed_dir(),
        "root",
        fio::PERM_READABLE,
    )
    .await
    .expect("Failed to open fs root");

    for (file_path, expected_hash) in &file_hashes {
        let file =
            fuchsia_fs::directory::open_file(&fs_root, file_path, fio::PERM_READABLE).await?;
        let mut hasher = Sha256::new();
        hasher.update(&fuchsia_fs::file::read(&file).await?);
        assert_eq!(*expected_hash, hex::encode(hasher.finalize()));
    }

    Ok(())
}

#[test_case(
    "/pkg/data/extents.img",
    hashmap!{
        "largefile".to_string() => "de2cf635ae4e0e727f1e412f978001d6a70d2386dc798d4327ec8c77a8e4895d".to_string(),
    };
    "fs with multiple files with multiple extents readonly")]
#[fuchsia::test]
async fn ext4_readonly_server_mounts_block_device(
    ext4_path: &str,
    file_hashes: HashMap<String, String>,
) -> Result<(), Error> {
    let server = Arc::new(VmoBackedServer::from_file(BLOCK_SIZE, ext4_path));
    let realm = create_realm(server, true).await?;

    let fs_root = fuchsia_fs::directory::open_directory(
        realm.root.get_exposed_dir(),
        "root",
        fio::PERM_READABLE,
    )
    .await
    .expect("Failed to open fs root");

    for (file_path, expected_hash) in &file_hashes {
        let file =
            fuchsia_fs::directory::open_file(&fs_root, file_path, fio::PERM_READABLE).await?;
        let mut hasher = Sha256::new();
        hasher.update(&fuchsia_fs::file::read(&file).await?);
        assert_eq!(*expected_hash, hex::encode(hasher.finalize()));
    }

    Ok(())
}

#[test_case("/pkg/data/1file.img", vec!["file1".to_string()]; "fs with one small file")]
#[test_case(
    "/pkg/data/nest.img",
    vec!["file1".to_string(), "inner/file2".to_string()];
    "fs with a single directory")]
#[fuchsia::test]
async fn ext4_server_overwrites_persist(
    ext4_path: &str,
    file_paths: Vec<String>,
) -> Result<(), Error> {
    let server = Arc::new(VmoBackedServer::from_file(BLOCK_SIZE, ext4_path));

    {
        let realm = create_realm(server.clone(), false).await?;

        let fs_root = fuchsia_fs::directory::open_directory(
            realm.root.get_exposed_dir(),
            "root",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to open fs root");

        for file_path in &file_paths {
            let file = fuchsia_fs::directory::open_file(
                &fs_root,
                file_path,
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            )
            .await?;
            let content = fuchsia_fs::file::read(&file).await?;

            // Write 1 to the start of every file
            file.seek(fio::SeekOrigin::Start, 0)
                .await
                .expect("failed FIDL seek")
                .map_err(zx::Status::err_from_raw)
                .expect("failed to seek file");
            let mut expected = content;
            expected[0] = 1;
            fuchsia_fs::file::write(&file, &expected).await?;

            file.seek(fio::SeekOrigin::Start, 0)
                .await
                .expect("failed FIDL seek")
                .map_err(zx::Status::err_from_raw)
                .expect("failed to seek file");
            let updated_content = fuchsia_fs::file::read(&file).await?;
            assert_eq!(updated_content, expected);

            file.sync()
                .await
                .expect("file sync check failed")
                .map_err(zx::Status::err_from_raw)
                .expect("file sync error");
            file.close()
                .await
                .expect("file close check failed")
                .map_err(zx::Status::err_from_raw)
                .expect("file close error");
        }

        fs_root
            .close()
            .await
            .expect("dir close check failed")
            .map_err(zx::Status::err_from_raw)
            .expect("dir close error");
        realm.destroy().await.expect("realm destroy failed");
    }

    // Check persistence
    {
        let realm = create_realm(server.clone(), false).await?;

        let fs_root = fuchsia_fs::directory::open_directory(
            realm.root.get_exposed_dir(),
            "root",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to open fs root");

        for file_path in &file_paths {
            let file = fuchsia_fs::directory::open_file(
                &fs_root,
                file_path,
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            )
            .await?;
            file.seek(fio::SeekOrigin::Start, 0)
                .await
                .expect("failed FIDL seek")
                .map_err(zx::Status::err_from_raw)
                .expect("failed to seek file");
            let content = fuchsia_fs::file::read(&file).await?;
            assert_eq!(content[0], 1, "Change not persisted for {}", file_path);
        }
    }

    Ok(())
}

#[fuchsia::test]
async fn test_truncate_does_not_persist() -> Result<(), Error> {
    let server = Arc::new(VmoBackedServer::from_file(BLOCK_SIZE, "/pkg/data/1file.img"));
    let file_path = "file1";

    let original_content;
    {
        let realm = create_realm(server.clone(), false).await?;
        let fs_root = fuchsia_fs::directory::open_directory(
            realm.root.get_exposed_dir(),
            "root",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to open fs root");

        let file = fuchsia_fs::directory::open_file(
            &fs_root,
            &file_path,
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await?;
        original_content = fuchsia_fs::file::read(&file).await?;

        // Re-open with truncate
        let file_trunc = fuchsia_fs::directory::open_file(
            &fs_root,
            &file_path,
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FILE_TRUNCATE,
        )
        .await?;

        // Verify the file has truncated
        file_trunc
            .seek(fio::SeekOrigin::Start, 0)
            .await
            .expect("seek failed")
            .map_err(zx::Status::err_from_raw)
            .expect("seek error");
        let new_content = fuchsia_fs::file::read(&file_trunc).await?;
        assert_eq!(new_content, b"");

        // Need to call sync to persist new contents
        file.sync()
            .await
            .expect("file sync check failed")
            .map_err(zx::Status::err_from_raw)
            .expect("file sync error");
        file.close()
            .await
            .expect("file close check failed")
            .map_err(zx::Status::err_from_raw)
            .expect("file close error");
        realm.destroy().await.expect("realm destroy failed");
    }

    // Check what was persisted (fsync is not supported, we don't expect metadata to be persisted).
    // Hence we expect that the original file size persisted and we read the original content.
    {
        let realm = create_realm(server.clone(), false).await?;
        let fs_root = fuchsia_fs::directory::open_directory(
            realm.root.get_exposed_dir(),
            "root",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("Failed to open fs root");

        let file = fuchsia_fs::directory::open_file(
            &fs_root,
            &file_path,
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await?;
        let content = fuchsia_fs::file::read(&file).await?;
        assert_eq!(content, original_content);
    }

    Ok(())
}
