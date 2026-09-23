// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use ext4_parser::{FsSourceType, construct_fs};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block as fblock;
use fuchsia_async as fasync;
use fuchsia_runtime::{HandleType, take_startup_handle};
use log::info;
use test_vmo_backed_block_server::VmoBackedServer;
use vfs::execution_scope::ExecutionScope;

#[fuchsia::main(threads = 10)]
async fn main() -> Result<(), Error> {
    info!("Starting ext4 test server");

    let server = VmoBackedServer::from_file(512, "/pkg/data/ext4_image.img");
    // Create a channel for the block client.
    let (block_client_end, block_server_end) =
        fidl::endpoints::create_endpoints::<fblock::BlockMarker>();

    // Serve the block device in a background task.
    fasync::Task::spawn(async move {
        if let Err(e) = server.serve(block_server_end.into_stream()).await {
            log::error!("Block server error: {:?}", e);
        }
    })
    .detach();

    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    // Construct the ext4 FS in RW mode.
    let tree = construct_fs(
        FsSourceType::BlockDevice(block_client_end),
        /* read_only= */ false,
        &inspector,
    )
    .map_err(|e| format_err!("Failed to construct file system: {:?}", e))?;

    let export_handle = take_startup_handle(HandleType::DirectoryRequest.into())
        .context("Missing startup handle")?;
    let scope = ExecutionScope::new();
    vfs::directory::serve_on(
        vfs::pseudo_directory! {
            "root" => tree,
        },
        fio::PERM_READABLE | fio::PERM_WRITABLE,
        scope.clone(),
        ServerEnd::new(export_handle.into()),
    );

    scope.wait().await;
    Ok(())
}
