// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fuchsia io conformance testing harness for Fxfs

// TODO(https://fxbug.dev/439053417): Investigate why the recursion limit was bumped
// to unblock the toolchain.
#![recursion_limit = "256"]

use anyhow::{Context as _, Error};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_io_test::{
    self as io_test, HarnessConfig, TestHarnessRequest, TestHarnessRequestStream,
};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use fxfs_testing::{TestFixture, open_dir, open_file};
use log::error;
use std::sync::atomic::{AtomicU64, Ordering};

struct Harness(TestHarnessRequestStream);

const FLAGS: fio::Flags =
    fio::Flags::FLAG_MAYBE_CREATE.union(fio::PERM_READABLE).union(fio::PERM_WRITABLE);

async fn add_entries(
    dest: fio::DirectoryProxy,
    entries: Vec<Option<Box<io_test::DirectoryEntry>>>,
) -> Result<(), Error> {
    let mut queue = vec![(dest, entries)];
    while let Some((dest, entries)) = queue.pop() {
        for entry in entries {
            match *entry.unwrap() {
                io_test::DirectoryEntry::Directory(io_test::Directory {
                    name, entries, ..
                }) => {
                    let new_dir = open_dir(
                        &dest,
                        &name,
                        FLAGS | fio::Flags::PROTOCOL_DIRECTORY,
                        &Default::default(),
                    )
                    .await
                    .context(format!("failed to create directory {name}"))?;
                    queue.push((new_dir, entries));
                }
                io_test::DirectoryEntry::File(io_test::File { name, contents, .. }) => {
                    let file = open_file(&dest, &name, FLAGS, &Default::default())
                        .await
                        .context(format!("failed to create file {name}"))?;
                    if !contents.is_empty() {
                        fuchsia_fs::file::write(&file, contents)
                            .await
                            .context(format!("failed to write contents for {name}"))?;
                    }
                }
                io_test::DirectoryEntry::Symlink(io_test::Symlink { name, target, .. }) => {
                    dest.create_symlink(&name, &target, None)
                        .await
                        .context(format!("failed to create symlink {name}"))?
                        .map_err(zx::Status::err_from_raw)
                        .context(format!("failed to create symlink {name}"))?;
                }
                _ => panic!("Not supported"),
            }
        }
    }
    Ok(())
}

async fn run(mut stream: TestHarnessRequestStream, fixture: &TestFixture) -> Result<(), Error> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    while let Some(request) = stream.try_next().await.context("error running harness server")? {
        match request {
            TestHarnessRequest::GetConfig { responder } => {
                responder.send(&HarnessConfig {
                    supports_executable_file: false,
                    supports_get_backing_memory: true,
                    supports_remote_dir: false,
                    supports_get_token: true,
                    supports_link_into: true,
                    supports_append: true,
                    supports_truncate: true,
                    supports_modify_directory: true,
                    supports_mutable_file: true,
                    supports_unnamed_temporary_file: true,
                    supported_attributes: fio::NodeAttributesQuery::PROTOCOLS
                        | fio::NodeAttributesQuery::ABILITIES
                        | fio::NodeAttributesQuery::CONTENT_SIZE
                        | fio::NodeAttributesQuery::STORAGE_SIZE
                        | fio::NodeAttributesQuery::LINK_COUNT
                        | fio::NodeAttributesQuery::ID
                        | fio::NodeAttributesQuery::CREATION_TIME
                        | fio::NodeAttributesQuery::MODIFICATION_TIME
                        | fio::NodeAttributesQuery::MODE
                        | fio::NodeAttributesQuery::UID
                        | fio::NodeAttributesQuery::GID
                        | fio::NodeAttributesQuery::RDEV
                        | fio::NodeAttributesQuery::ACCESS_TIME
                        | fio::NodeAttributesQuery::CASEFOLD
                        | fio::NodeAttributesQuery::SELINUX_CONTEXT
                        | fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
                    supports_services: false,
                    supports_xattrs: true,
                    supports_symlinks: true,
                })?;
            }
            TestHarnessRequest::CreateDirectory {
                contents,
                flags,
                object_request,
                control_handle: _,
            } => {
                let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
                let dir = open_dir(
                    fixture.root(),
                    &format!("test.{}", counter),
                    FLAGS | fio::Flags::PROTOCOL_DIRECTORY,
                    &Default::default(),
                )
                .await
                .unwrap();
                add_entries(fuchsia_fs::directory::clone(&dir).expect("clone failed"), contents)
                    .await
                    .expect("add_entries failed");
                dir.open(".", flags, &Default::default(), object_request.into_channel().into())
                    .unwrap();
            }
            TestHarnessRequest::OpenServiceDirectory { responder: _ } => {
                panic!("fxfs does not support service directories")
            }
        };
    }

    Ok(())
}

#[fuchsia::main(threads = 4)]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(Harness);
    fs.take_and_serve_directory_handle()?;

    let fixture = TestFixture::new().await;

    fs.for_each_concurrent(10_000, |Harness(stream)| {
        run(stream, &fixture).unwrap_or_else(|e| error!("Error processing request: {e:?}"))
    })
    .await;

    fixture.close().await;

    Ok(())
}
