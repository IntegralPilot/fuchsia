// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// ext4_server reads, writes, and exposes (incomplete) read-write ext4 filesystems to clients.
// WARNING: This server currently serves a simple version of ext4 that only supports overwriting
// allocated files. The implementation is inefficient and there is room for improvement.

use anyhow::{Context as _, Error, format_err};
use ext4_parser::FsSourceType;
use fidl::endpoints::{DiscoverableProtocolMarker as _, ServerEnd};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::BlockMarker;
use fuchsia_runtime::{HandleType, take_startup_handle};
use log::info;
use std::env;
use vfs::execution_scope::ExecutionScope;

#[fuchsia::main(threads = 10)]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = env::args().collect();
    let read_only = args.iter().any(|arg| arg == "--read-only");

    info!("Starting ext4_server, read_only={read_only}");

    let (block_device, server) = fidl::endpoints::create_endpoints();
    fuchsia_component::client::connect_channel_to_protocol_at_path(
        server.into_channel(),
        &format!("/block/{}", BlockMarker::PROTOCOL_NAME),
    )
    .context("Failed to connect to Volume")?;

    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    let tree = match ext4_parser::construct_fs(
        FsSourceType::BlockDevice(block_device),
        read_only,
        &inspector,
    ) {
        Ok(tree) => tree,
        Err(err) => return Err(format_err!("Failed to construct file system: {:?}", err)),
    };

    let directory_handle = take_startup_handle(HandleType::DirectoryRequest.into()).unwrap();
    let scope = ExecutionScope::new();

    let rights =
        if read_only { fio::PERM_READABLE } else { fio::PERM_READABLE | fio::PERM_WRITABLE };

    vfs::directory::serve_on(
        vfs::pseudo_directory! {
            "root" => tree,
        },
        rights,
        scope.clone(),
        ServerEnd::new(directory_handle.into()),
    );

    // Wait until the directory connection is closed by the client before exiting.
    scope.wait().await;
    info!("ext4 directory connection dropped, exiting");
    Ok(())
}
