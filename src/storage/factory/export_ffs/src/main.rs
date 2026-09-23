// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use argh::FromArgs;
use export_ffs::export_directory;
use fidl::endpoints::{ClientEnd, Proxy as _};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::{BlockMarker, BlockProxy};

/// A command line tool for generating factoryfs partitions by flattening existing directory
/// structures.
#[derive(Debug, FromArgs)]
struct Args {
    /// path to the directory to flatten into a factoryfs partition.
    #[argh(positional)]
    directory: String,

    /// block device to write the factoryfs partition to. THIS IS DESTRUCTIVE!
    #[argh(positional)]
    device: String,
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let Args { directory, device } = argh::from_env();

    // open directory
    let dir = fuchsia_fs::directory::open_in_namespace(&directory, fio::PERM_READABLE)?;

    // open block device
    let proxy = fuchsia_component::client::connect_to_protocol_at_path::<BlockMarker>(&device)
        .with_context(|| format!("failed to open {}", device))?;
    let channel = proxy
        .into_channel()
        .map_err(|_: BlockProxy| anyhow::anyhow!("failed to get block channel"))?;
    let client_end = ClientEnd::<BlockMarker>::new(channel.into());

    // call the exporter
    let () = export_directory(&dir, client_end).await.with_context(|| {
        format!("failed to export '{}' to block device '{}'", directory, device)
    })?;

    Ok(())
}
