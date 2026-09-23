// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::SimpleFile;
use crate::execution_scope::ExecutionScope;
use crate::{
    assert_close, assert_get_attr, assert_read, assert_read_at, assert_seek, assert_truncate_err,
    assert_write_err, file,
};
use assert_matches::assert_matches;
use flex_fuchsia_io as fio;
use futures::StreamExt;
use libc::{S_IFREG, S_IRUSR};
use zx_status::Status;

fn test_scope() -> ExecutionScope {
    #[cfg(feature = "fdomain")]
    let client = flex_local::local_client_empty();
    #[cfg(feature = "fdomain")]
    return ExecutionScope::new(client);
    #[cfg(not(feature = "fdomain"))]
    return ExecutionScope::new();
}

/// Verify that [`SimpleFile::read_only`] works with static and owned data. Compile-time test.
#[test]
fn read_only_types() {
    // Static data.
    SimpleFile::read_only("from str");
    SimpleFile::read_only(b"from bytes");

    const STATIC_STRING: &'static str = "static string";
    const STATIC_BYTES: &'static [u8] = b"static bytes";
    SimpleFile::read_only(STATIC_STRING);
    SimpleFile::read_only(&STATIC_STRING);
    SimpleFile::read_only(STATIC_BYTES);
    SimpleFile::read_only(&STATIC_BYTES);

    // Owned data.
    SimpleFile::read_only(String::from("Hello, world"));
    SimpleFile::read_only(vec![0u8; 2]);

    // Borrowed data.
    let runtime_string = String::from("Hello, world");
    SimpleFile::read_only(&runtime_string);
    let runtime_bytes = vec![0u8; 2];
    SimpleFile::read_only(&runtime_bytes);
}

#[fuchsia::test]
async fn read_only_read() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Read only test");
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_read!(proxy, "Read only test");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_only_read_owned() {
    let scope = test_scope();
    let bytes = String::from("Run-time value");
    let file = SimpleFile::read_only(bytes);
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_read!(proxy, "Run-time value");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_only_ignore_inherit_flag() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Content");
    let proxy = file::serve_proxy_with_scope(
        file,
        fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE,
        &scope,
    );
    assert_read!(proxy, "Content");
    assert_write_err!(proxy, "Can write", Status::BAD_HANDLE);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_only_read_with_describe() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Read only test");
    let proxy = file::serve_proxy_with_scope(
        file,
        fio::PERM_READABLE | fio::Flags::FLAG_SEND_REPRESENTATION,
        &scope,
    );
    assert_matches!(
        proxy.take_event_stream().next().await,
        Some(Ok(fio::FileEvent::OnRepresentation { .. }))
    );
}

#[fuchsia::test]
async fn read_only_write_is_not_supported() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Read only test");
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE | fio::PERM_WRITABLE, &scope);
    assert_matches!(
        proxy.take_event_stream().next().await,
        Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. }))
            if epitaph == Status::ACCESS_DENIED
    );
}

#[fuchsia::test]
async fn read_at_0() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Whole file content");
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_read_at!(proxy, 0, "Whole file content");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_at_overlapping() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Content of the file");
    //                                 0         1
    //                                 0123456789012345678
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_read_at!(proxy, 3, "tent of the");
    assert_read_at!(proxy, 11, "the file");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_mixed_with_read_at() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Content of the file");
    //                                 0         1
    //                                 0123456789012345678
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_read!(proxy, "Content");
    assert_read_at!(proxy, 3, "tent of the");
    assert_read!(proxy, " of the ");
    assert_read_at!(proxy, 11, "the file");
    assert_read!(proxy, "file");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn seek_valid_positions() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Long file content");
    //                     0         1
    //                     01234567890123456
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_seek!(proxy, 5, Start);
    assert_read!(proxy, "file");
    assert_seek!(proxy, 1, Current, Ok(10));
    assert_read!(proxy, "content");
    assert_seek!(proxy, -12, End, Ok(5));
    assert_read!(proxy, "file content");
    assert_close!(proxy);
}
#[fuchsia::test]
async fn seek_valid_beyond_size() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Content");
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_seek!(proxy, 20, Start);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn seek_triggers_overflow() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"File size and contents don't matter for this test");
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_seek!(proxy, i64::MAX, Start);
    assert_seek!(proxy, i64::MAX, Current, Ok(u64::MAX - 1));
    assert_seek!(proxy, 2, Current, Err(Status::OUT_OF_RANGE));
    assert_close!(proxy);
}

#[fuchsia::test]
async fn seek_invalid_before_0() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Seek position is unaffected");
    //                     0        1         2
    //                     12345678901234567890123456
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_seek!(proxy, -10, Current, Err(Status::OUT_OF_RANGE));
    assert_read!(proxy, "Seek");
    assert_seek!(proxy, -10, Current, Err(Status::OUT_OF_RANGE));
    assert_read!(proxy, " position");
    assert_seek!(proxy, -100, End, Err(Status::OUT_OF_RANGE));
    assert_read!(proxy, " is unaffected");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn seek_empty_file() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"");
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_seek!(proxy, 0, Start);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn truncate_read_only_file_fails() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Read-only content");
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_truncate_err!(proxy, 10, Status::BAD_HANDLE);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn get_attr_read_only() {
    let scope = test_scope();
    let file = SimpleFile::read_only(b"Content");
    let proxy = file::serve_proxy_with_scope(file, fio::PERM_READABLE, &scope);
    assert_get_attr!(
        proxy,
        fio::NodeAttributes {
            mode: S_IFREG | S_IRUSR,
            id: fio::INO_UNKNOWN,
            content_size: 7,
            storage_size: 7,
            link_count: 1,
            creation_time: 0,
            modification_time: 0,
        }
    );
    assert_close!(proxy);
}
