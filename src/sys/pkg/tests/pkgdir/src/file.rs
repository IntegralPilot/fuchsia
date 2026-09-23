// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for the fuchsia.io/File behavior of file, meta/file, and meta as file. To optimize for
//! simplicity and speed, we don't test e.g. dir/file or meta/dir/file because there aren't any
//! meaningful differences in File behavior.

use crate::{PackageSource, dirs_to_test, repeat_by_n};
use assert_matches::assert_matches;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_io as fio;
use fuchsia_fs::directory::open_file;
use std::cmp;
use std::convert::TryFrom as _;

const TEST_PKG_HASH: &str = "44e7704720706ffd287bf0d27133cec7d609581547095ab6c6fee94f83a5bcbc";

#[fuchsia::test]
async fn read() {
    for source in dirs_to_test().await {
        read_per_package_source(source).await
    }
}

async fn read_per_package_source(source: PackageSource) {
    let root_dir = source.dir;
    for (path, expected_contents) in
        [("file", "file"), ("meta/file", "meta/file"), ("meta", TEST_PKG_HASH)]
    {
        assert_read_max_buffer_success(&root_dir, path, expected_contents).await;
        assert_read_buffer_success(&root_dir, path, expected_contents).await;
        assert_read_past_end(&root_dir, path, expected_contents).await;
    }

    assert_read_exceeds_buffer_success(&root_dir, "exceeds_max_buf").await;
    assert_read_exceeds_buffer_success(&root_dir, "meta/exceeds_max_buf").await;
}

async fn assert_read_max_buffer_success(
    root_dir: &fio::DirectoryProxy,
    path: &str,
    expected_contents: &str,
) {
    let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();
    let bytes = file.read(fio::MAX_BUF).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected_contents);
}

async fn assert_read_buffer_success(
    root_dir: &fio::DirectoryProxy,
    path: &str,
    expected_contents: &str,
) {
    for buffer_size in 0..expected_contents.len() {
        let expected_contents = &expected_contents[0..buffer_size];

        let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();
        let bytes = file
            .read(buffer_size.try_into().unwrap())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| {
                panic!(
                    "path: {path}, expected_contents: {expected_contents}, buffer size: {buffer_size}"
                )
            });
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected_contents);
    }
}

async fn assert_read_past_end(root_dir: &fio::DirectoryProxy, path: &str, expected_contents: &str) {
    let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();
    let bytes = file.read(fio::MAX_BUF).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected_contents);

    let bytes = file.read(fio::MAX_BUF).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(bytes, &[] as &[u8]);
}

async fn assert_read_exceeds_buffer_success(root_dir: &fio::DirectoryProxy, path: &str) {
    let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();

    // Read the first MAX_BUF contents.
    let bytes = file.read(fio::MAX_BUF).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        &repeat_by_n('a', fio::MAX_BUF.try_into().unwrap())
    );

    // There should be one remaining "a".
    let bytes = file.read(fio::MAX_BUF).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), "a");

    // Since we are now at the end of the file, bytes should be empty.
    let bytes = file.read(fio::MAX_BUF).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(bytes, &[] as &[u8]);
}

#[fuchsia::test]
async fn read_at() {
    for source in dirs_to_test().await {
        read_at_per_package_source(source).await
    }
}

async fn read_at_per_package_source(source: PackageSource) {
    for path in ["file", "meta/file", "meta"] {
        let expected_contents = if path == "meta" { TEST_PKG_HASH } else { path };
        assert_read_at_max_buffer_success(&source.dir, path, expected_contents).await;
        assert_read_at_success(&source.dir, path, expected_contents).await;

        assert_read_at_does_not_affect_seek_offset(&source.dir, path).await;
        assert_read_at_is_unaffected_by_seek(&source.dir, path).await;
    }
}

async fn assert_read_at_max_buffer_success(
    root_dir: &fio::DirectoryProxy,
    path: &str,
    expected_contents: &str,
) {
    let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();
    let bytes =
        file.read_at(fio::MAX_BUF, 0).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected_contents);
}

async fn assert_read_at_success(
    root_dir: &fio::DirectoryProxy,
    path: &str,
    full_expected_contents: &str,
) {
    for offset in 0..full_expected_contents.len() {
        for count in 0..full_expected_contents.len() {
            let end = cmp::min(count + offset, full_expected_contents.len());
            let expected_contents = &full_expected_contents[offset..end];

            let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();
            let bytes = file
                .read_at(count.try_into().unwrap(), offset.try_into().unwrap())
                .await
                .unwrap()
                .map_err(zx::Status::err_from_raw)
                .unwrap_or_else(|_| {
                    panic!(
                        "path: {path}, offset: {offset}, count: {count}, expected_contents: {expected_contents}"
                    )
                });
            assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected_contents);
        }
    }
}

async fn assert_read_at_does_not_affect_seek_offset(root_dir: &fio::DirectoryProxy, path: &str) {
    for seek_offset in 0..path.len() as i64 {
        let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();

        let position = file
            .seek(fio::SeekOrigin::Start, seek_offset)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));
        assert_eq!(position, seek_offset as u64);

        let _: Vec<u8> = file
            .read_at(fio::MAX_BUF, 0)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));

        // get seek offset
        let position = file
            .seek(fio::SeekOrigin::Current, 0)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));
        assert_eq!(position, seek_offset as u64)
    }
}

async fn assert_read_at_is_unaffected_by_seek(root_dir: &fio::DirectoryProxy, path: &str) {
    for seek_offset in 0..path.len() as i64 {
        let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();

        let first_read_bytes = file
            .read_at(fio::MAX_BUF, 0)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));

        let position = file
            .seek(fio::SeekOrigin::Start, seek_offset)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));
        assert_eq!(position, seek_offset as u64);

        let second_read_bytes = file
            .read_at(fio::MAX_BUF, 0)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));
        assert_eq!(
            std::str::from_utf8(&first_read_bytes).unwrap(),
            std::str::from_utf8(&second_read_bytes).unwrap()
        );
    }
}

#[fuchsia::test]
async fn seek() {
    for source in dirs_to_test().await {
        seek_per_package_source(source).await
    }
}

async fn seek_per_package_source(source: PackageSource) {
    let root_dir = &source.dir;
    for path in ["file", "meta/file", "meta"] {
        let expected = if path == "meta" { TEST_PKG_HASH } else { path };
        assert_seek_success(root_dir, path, expected, fio::SeekOrigin::Start).await;
        assert_seek_success(root_dir, path, expected, fio::SeekOrigin::Current).await;

        assert_seek_affects_read(root_dir, path, expected).await;

        assert_seek_past_end(root_dir, path, expected, fio::SeekOrigin::Start).await;
        assert_seek_past_end(root_dir, path, expected, fio::SeekOrigin::Current).await;
        assert_seek_past_end_end_origin(root_dir, path, expected).await;
    }
}

async fn assert_seek_success(
    root_dir: &fio::DirectoryProxy,
    path: &str,
    expected: &str,
    seek_origin: fio::SeekOrigin,
) {
    for expected_position in 0..expected.len() {
        let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();
        let position = file
            .seek(seek_origin, expected_position.try_into().unwrap())
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| {
                panic!(
                    "path: {path}, seek_origin: {seek_origin:?}, expected_position: {expected_position}"
                )
            });
        assert_eq!(position, expected_position as u64);
    }
}

async fn assert_seek_affects_read(root_dir: &fio::DirectoryProxy, path: &str, expected: &str) {
    for seek_offset in 0..expected.len() {
        let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();
        let bytes = file
            .read(fio::MAX_BUF)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected);

        let bytes = file
            .read(fio::MAX_BUF)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));
        assert_eq!(bytes, &[] as &[u8]);

        let expected_contents = &expected[expected.len() - seek_offset..];
        let position = file
            .seek(fio::SeekOrigin::End, -(seek_offset as i64))
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));

        assert_eq!(position, (expected.len() - seek_offset) as u64);

        let bytes = file
            .read(fio::MAX_BUF)
            .await
            .unwrap()
            .map_err(zx::Status::err_from_raw)
            .unwrap_or_else(|_| panic!("path: {path}, seek_offset: {seek_offset}"));
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected_contents);
    }
}

async fn assert_seek_past_end(
    root_dir: &fio::DirectoryProxy,
    path: &str,
    expected: &str,
    seek_origin: fio::SeekOrigin,
) {
    let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();
    let position = file
        .seek(seek_origin, expected.len() as i64 + 1)
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw)
        .unwrap();
    assert_eq!(expected.len() as u64 + 1, position);

    let bytes = file.read(fio::MAX_BUF).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(bytes, &[] as &[u8]);
}

// The difference between this test and `assert_seek_past_end` is that the offset is 1
// so that the position is evaluated to path.len() + 1 like in `assert_seek_past_end`.
async fn assert_seek_past_end_end_origin(
    root_dir: &fio::DirectoryProxy,
    path: &str,
    expected: &str,
) {
    let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();
    let position = file
        .seek(fio::SeekOrigin::End, 1)
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw)
        .unwrap();
    assert_eq!(expected.len() as u64 + 1, position);

    let bytes = file.read(fio::MAX_BUF).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(bytes, &[] as &[u8]);
}

#[fuchsia::test]
async fn get_backing_memory() {
    for source in dirs_to_test().await {
        get_backing_memory_per_package_source(source).await
    }
}

async fn get_backing_memory_per_package_source(source: PackageSource) {
    let root_dir = &source.dir;

    // calls with supported flags should succeed for files that are not meta-as-file.
    for size in [0, 1, 4095, 4096, 4097] {
        for path in [format!("file_{size}"), format!("meta/file_{size}")] {
            let file = open_file(root_dir, &path, fuchsia_fs::PERM_READABLE).await.unwrap();

            let _: zx::Vmo =
                test_get_backing_memory_success(&file, fio::VmoFlags::READ, size).await;

            let vmo0 = test_get_backing_memory_success(
                &file,
                fio::VmoFlags::READ | fio::VmoFlags::PRIVATE_CLONE,
                size,
            )
            .await;
            let vmo1 = test_get_backing_memory_success(
                &file,
                fio::VmoFlags::READ | fio::VmoFlags::PRIVATE_CLONE,
                size,
            )
            .await;
            assert_ne!(
                vmo0.as_handle_ref().koid().unwrap(),
                vmo1.as_handle_ref().koid().unwrap(),
                "We should receive our own clone each time we invoke with {:?}",
                fio::VmoFlags::PRIVATE_CLONE,
            );
        }
    }

    let file = open_file(root_dir, "meta", fuchsia_fs::PERM_READABLE).await.unwrap();
    let result = file
        .get_backing_memory(fio::VmoFlags::READ)
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw);
    assert_matches!(result, Ok(_));

    // For files NOT under meta, calls with unsupported flags should successfully return the FIDL
    // call with a failure status.
    let file = open_file(root_dir, "file", fuchsia_fs::PERM_READABLE).await.unwrap();
    let result = file
        .get_backing_memory(fio::VmoFlags::EXECUTE)
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw);
    assert_eq!(result, Err(zx::Status::ACCESS_DENIED));
    let result = file
        .get_backing_memory(fio::VmoFlags::PRIVATE_CLONE | fio::VmoFlags::SHARED_BUFFER)
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw);
    assert_eq!(result, Err(zx::Status::INVALID_ARGS));
    let result = file
        .get_backing_memory(fio::VmoFlags::WRITE)
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw);
    assert_eq!(result, Err(zx::Status::ACCESS_DENIED));

    let file = open_file(root_dir, "meta/file", fuchsia_fs::PERM_READABLE).await.unwrap();
    let result = file
        .get_backing_memory(fio::VmoFlags::EXECUTE)
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw);
    assert_eq!(result, Err(zx::Status::ACCESS_DENIED));
    let result = file
        .get_backing_memory(fio::VmoFlags::PRIVATE_CLONE | fio::VmoFlags::SHARED_BUFFER)
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw);
    assert_eq!(result, Err(zx::Status::INVALID_ARGS));
    let result = file
        .get_backing_memory(fio::VmoFlags::WRITE)
        .await
        .unwrap()
        .map_err(zx::Status::err_from_raw);
    assert_eq!(result, Err(zx::Status::ACCESS_DENIED));
}

fn round_up_to_4096_multiple(val: usize) -> usize {
    (val + 4095) & !4095
}

async fn test_get_backing_memory_success(
    file: &fio::FileProxy,
    flags: fio::VmoFlags,
    size: usize,
) -> zx::Vmo {
    let vmo =
        file.get_backing_memory(flags).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

    let vmo_size = vmo.get_size().unwrap().try_into().unwrap();
    assert_eq!(vmo.get_content_size().unwrap(), u64::try_from(size).unwrap());
    let mut actual_contents = vec![0u8; vmo_size];
    let () = vmo.read(actual_contents.as_mut_slice(), 0).unwrap();

    let rounded_size = round_up_to_4096_multiple(size);
    assert_eq!(vmo_size, rounded_size);
    assert!(
        b"ABCD"
            .iter()
            .copied()
            .cycle()
            .take(size)
            // VMOs should be zero-padded to 4096 bytes.
            .chain(std::iter::repeat(b'\0'))
            .take(rounded_size)
            .eq(actual_contents.iter().copied()),
        "vmo content mismatch for file size {vmo_size}",
    );

    vmo
}

#[fuchsia::test]
async fn clone() {
    for source in dirs_to_test().await {
        clone_per_package_source(source).await
    }
}

async fn clone_per_package_source(source: PackageSource) {
    let root_dir = &source.dir;
    assert_clone_success(root_dir, "file", "file").await;
    assert_clone_success(root_dir, "meta/file", "meta/file").await;
    assert_clone_success(root_dir, "meta", TEST_PKG_HASH).await;
}

async fn assert_clone_success(
    package_root: &fio::DirectoryProxy,
    path: &str,
    expected_contents: &str,
) {
    let parent = open_file(package_root, path, fuchsia_fs::PERM_READABLE)
        .await
        .expect("open parent directory");
    let (clone, server_end) = create_proxy::<fio::FileMarker>();
    let node_request = fidl::endpoints::ServerEnd::new(server_end.into_channel());
    parent.clone(node_request).expect("cloned node");
    let bytes = clone.read(fio::MAX_BUF).await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected_contents);
}

#[fuchsia::test]
async fn unsupported() {
    for source in dirs_to_test().await {
        unsupported_per_package_source(source).await
    }
}

async fn unsupported_per_package_source(source: PackageSource) {
    let root_dir = &source.dir;
    async fn verify_unsupported_calls(
        root_dir: &fio::DirectoryProxy,
        path: &str,
        expected_status: zx::Status,
    ) {
        let file = open_file(root_dir, path, fuchsia_fs::PERM_READABLE).await.unwrap();

        // Verify write() fails.
        let result = file.write(b"potato").await.unwrap().map_err(zx::Status::err_from_raw);
        assert_eq!(result, Err(expected_status));

        // Verify writeAt() fails.
        let status = file.write_at(b"potato", 0).await.unwrap().map_err(zx::Status::err_from_raw);
        assert_eq!(status, Err(expected_status));

        // Verify resize() fails.
        assert_eq!(
            file.resize(0).await.unwrap().map_err(zx::Status::err_from_raw),
            Err(expected_status)
        );
    }

    // The name of this test is slightly misleading because files not under meta will yield
    // BAD_HANDLE for the unsupported file APIs. This is actually consistent with the fuchsia.io
    // documentation because files without WRITE permissions *should* yield BAD_HANDLE for these
    // methods.
    verify_unsupported_calls(root_dir, "file", zx::Status::BAD_HANDLE).await;
    verify_unsupported_calls(root_dir, "meta/file", zx::Status::BAD_HANDLE).await;
    verify_unsupported_calls(root_dir, "meta", zx::Status::BAD_HANDLE).await;
}
