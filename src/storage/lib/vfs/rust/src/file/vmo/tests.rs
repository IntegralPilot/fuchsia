// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for the asynchronous files.

use super::{VmoFile, read_only};
use crate::{
    assert_close, assert_get_attr, assert_read, assert_read_at, assert_seek, assert_truncate_err,
    assert_write_err, file,
};
use assert_matches::assert_matches;
use flex_client::ProxyHasDomain;
use flex_fuchsia_io as fio;
use futures::{FutureExt as _, StreamExt as _};
use libc::{S_IFREG, S_IRUSR};
use zx::Vmo;
use zx_status::Status;

/// Verify that [`read_only`] works with static and owned data. Compile-time test.
#[test]
fn read_only_types() {
    // Static data.
    read_only("from str");
    read_only(b"from bytes");

    const STATIC_STRING: &'static str = "static string";
    const STATIC_BYTES: &'static [u8] = b"static bytes";
    read_only(STATIC_STRING);
    read_only(&STATIC_STRING);
    read_only(STATIC_BYTES);
    read_only(&STATIC_BYTES);

    // Owned data.
    read_only(String::from("Hello, world"));
    read_only(vec![0u8; 2]);

    // Borrowed data.
    let runtime_string = String::from("Hello, world");
    read_only(&runtime_string);
    let runtime_bytes = vec![0u8; 2];
    read_only(&runtime_bytes);
}

#[fuchsia::test]
async fn read_only_read() {
    let file = read_only(b"Read only test");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_read!(proxy, "Read only test");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_only_read_owned() {
    let bytes = String::from("Run-time value");
    let file = read_only(bytes);
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_read!(proxy, "Run-time value");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_only_ignore_inherit_flag() {
    let file = read_only(b"Content");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE);
    assert_read!(proxy, "Content");
    assert_write_err!(proxy, "Can write", Status::BAD_HANDLE);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_only_read_no_status() {
    let file = read_only(b"Read only test");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    // Since serve_proxy/open are sync, the event would be on the channel by the time they return
    // if there was going to be one.
    assert_matches::assert_matches!(proxy.take_event_stream().next().now_or_never(), None);
}

#[fuchsia::test]
async fn read_only_read_with_describe() {
    let file = read_only(b"Read only test");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE | fio::Flags::FLAG_SEND_REPRESENTATION);
    assert_matches!(
        proxy.take_event_stream().next().await,
        Some(Ok(fio::FileEvent::OnRepresentation { .. }))
    );
}

#[fuchsia::test]
async fn read_only_write_is_not_supported() {
    let file = read_only(b"Read only test");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE | fio::PERM_WRITABLE);
    assert_matches!(
        proxy.take_event_stream().next().await,
        Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. }))
            if epitaph == Status::ACCESS_DENIED
    );
}

#[fuchsia::test]
async fn read_at_0() {
    let file = read_only(b"Whole file content");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_read_at!(proxy, 0, "Whole file content");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_at_overlapping() {
    let file = read_only(b"Content of the file");
    //                     0         1
    //                     0123456789012345678
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_read_at!(proxy, 3, "tent of the");
    assert_read_at!(proxy, 11, "the file");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_mixed_with_read_at() {
    let file = read_only(b"Content of the file");
    //                     0         1
    //                     0123456789012345678
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_read!(proxy, "Content");
    assert_read_at!(proxy, 3, "tent of the");
    assert_read!(proxy, " of the ");
    assert_read_at!(proxy, 11, "the file");
    assert_read!(proxy, "file");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn executable_read() {
    const FILE_CONTENTS: &'static str = "file-contents";
    let vmo = Vmo::create(FILE_CONTENTS.len() as u64).unwrap();
    vmo.write(FILE_CONTENTS.as_bytes(), 0).unwrap();
    let file = VmoFile::new_with_inode_and_executable(vmo, fio::INO_UNKNOWN, true);
    let proxy = file::serve_proxy(file, fio::PERM_READABLE | fio::PERM_EXECUTABLE);
    assert_read!(proxy, FILE_CONTENTS);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn seek_valid_positions() {
    let file = read_only(b"Long file content");
    //                     0         1
    //                     01234567890123456
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
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
    let file = read_only(b"Content");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_seek!(proxy, 20, Start);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn seek_triggers_overflow() {
    let file = read_only(b"File size and contents don't matter for this test");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_seek!(proxy, i64::MAX, Start);
    assert_seek!(proxy, i64::MAX, Current, Ok(u64::MAX - 1));
    assert_seek!(proxy, 2, Current, Err(Status::INVALID_ARGS));
    assert_close!(proxy);
}

#[fuchsia::test]
async fn seek_invalid_before_0() {
    let file = read_only(b"Seek position is unaffected");
    //                     0        1         2
    //                     12345678901234567890123456
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_seek!(proxy, -10, Current, Err(Status::INVALID_ARGS));
    assert_read!(proxy, "Seek");
    assert_seek!(proxy, -10, Current, Err(Status::INVALID_ARGS));
    assert_read!(proxy, " position");
    assert_seek!(proxy, -100, End, Err(Status::INVALID_ARGS));
    assert_read!(proxy, " is unaffected");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn seek_empty_file() {
    let file = read_only(b"");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_seek!(proxy, 0, Start);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn truncate_read_only_file_fails() {
    let file = read_only(b"Read-only content");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_truncate_err!(proxy, 10, Status::BAD_HANDLE);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn clone_inherit_access() {
    const FILE_CONTENTS: &'static str = "Initial content";
    let file = read_only(FILE_CONTENTS);
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_read!(proxy, FILE_CONTENTS);
    let (clone_proxy, server) = proxy.domain().create_proxy::<fio::FileMarker>();
    proxy.clone(server.into_channel().into()).unwrap();
    assert_read!(clone_proxy, FILE_CONTENTS);
    assert_close!(proxy);
    assert_close!(clone_proxy);
}

#[fuchsia::test]
async fn get_attr_read_only() {
    let file = read_only(b"Content");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
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

#[fuchsia::test]
async fn get_attr_read_only_with_inode() {
    const INODE: u64 = 12345;
    const CONTENT: &'static [u8] = b"Content";
    let content_len: u64 = CONTENT.len().try_into().unwrap();
    let vmo = Vmo::create(content_len).unwrap();
    vmo.write(&CONTENT, 0).unwrap();
    let file = VmoFile::new_with_inode(vmo, INODE);
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_get_attr!(
        proxy,
        fio::NodeAttributes {
            mode: S_IFREG | S_IRUSR,
            id: INODE, // Custom inode was specified for this file.
            content_size: content_len,
            storage_size: content_len,
            link_count: 1,
            creation_time: 0,
            modification_time: 0,
        }
    );
    assert_close!(proxy);
}

#[fuchsia::test]
async fn get_vmo_read_only() {
    let file = read_only(b"Read only test");
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);

    async fn assert_get_vmo(
        proxy: &fio::FileProxy,
        flags: fio::VmoFlags,
    ) -> Result<fidl::Vmo, Status> {
        proxy
            .get_backing_memory(flags)
            .await
            .expect("get_backing_memory fidl error")
            .map_err(Status::err_from_raw)
    }

    fn assert_vmo_content(vmo: &fidl::Vmo, expected: &[u8]) {
        let size = vmo.get_content_size().unwrap() as usize;
        assert_eq!(size, expected.len());
        let mut buffer = vec![0; size];
        vmo.read(&mut buffer, 0).unwrap();
        assert_eq!(buffer, expected);
    }

    let vmo = assert_get_vmo(&proxy, fio::VmoFlags::READ).await.unwrap();
    assert_vmo_content(&vmo, b"Read only test");

    let vmo =
        assert_get_vmo(&proxy, fio::VmoFlags::READ | fio::VmoFlags::SHARED_BUFFER).await.unwrap();
    assert_vmo_content(&vmo, b"Read only test");

    let vmo =
        assert_get_vmo(&proxy, fio::VmoFlags::READ | fio::VmoFlags::PRIVATE_CLONE).await.unwrap();
    assert_vmo_content(&vmo, b"Read only test");

    assert_eq!(
        assert_get_vmo(&proxy, fio::VmoFlags::READ | fio::VmoFlags::WRITE).await.unwrap_err(),
        Status::ACCESS_DENIED
    );
    assert_eq!(
        assert_get_vmo(
            &proxy,
            fio::VmoFlags::READ | fio::VmoFlags::WRITE | fio::VmoFlags::SHARED_BUFFER
        )
        .await
        .unwrap_err(),
        Status::ACCESS_DENIED
    );
    assert_eq!(
        assert_get_vmo(
            &proxy,
            fio::VmoFlags::READ | fio::VmoFlags::WRITE | fio::VmoFlags::PRIVATE_CLONE
        )
        .await
        .unwrap_err(),
        Status::ACCESS_DENIED
    );

    assert_read!(proxy, "Read only test");
    assert_close!(proxy);
}

#[fuchsia::test]
async fn read_only_vmo_file() {
    let vmo = Vmo::create(1024).expect("create failed");
    let data = b"Read only str";
    vmo.write(data, 0).expect("write failed");
    vmo.set_content_size(&(data.len() as u64)).expect("set_content_size failed");
    let file = VmoFile::new(vmo);
    let proxy = file::serve_proxy(file, fio::PERM_READABLE);
    assert_read!(proxy, "Read only str");
    assert_close!(proxy);
}
