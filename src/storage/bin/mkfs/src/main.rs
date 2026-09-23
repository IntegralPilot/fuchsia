// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use argh::FromArgs;
use component_debug::dirs::{OpenDirType, connect_to_instance_protocol};
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_device::ControllerMarker;
use fidl_fuchsia_fs_realm as fs_realm;
use fidl_fuchsia_sys2::RealmQueryMarker;
use fuchsia_component::client::{connect_channel_to_protocol_at_path, connect_to_protocol_at_path};

#[derive(FromArgs)]
#[argh(
    description = "A utility for formatting a block device with a new instance of a filesystem."
)]
struct Options {
    /// the device path of the block device
    #[argh(positional)]
    device_path: String,

    /// the name of the filesystem that will be formatted onto the block device
    #[argh(positional)]
    filesystem: String,

    /// verbose flag
    #[argh(option)]
    verbose: Option<bool>,

    /// number of fvm data slices
    #[argh(option)]
    fvm_data_slices: Option<u32>,
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let opt: Options = argh::from_env();
    let device_path = opt.device_path;
    let filesystem = opt.filesystem;

    let mut format_options = fs_realm::FormatOptions::default();
    format_options.verbose = opt.verbose;
    format_options.fvm_data_slices = opt.fvm_data_slices;

    let (client_end, server_end) = create_endpoints::<ControllerMarker>();
    connect_channel_to_protocol_at_path(server_end.into_channel(), &device_path)?;

    // Connect to fs_realm
    const REALM_QUERY_SERVICE_PATH: &str = "/svc/fuchsia.sys2.RealmQuery.root";
    let realm_query_proxy =
        connect_to_protocol_at_path::<RealmQueryMarker>(REALM_QUERY_SERVICE_PATH)?;
    let moniker = "./core/fs_realm".try_into().unwrap();
    let fs_realm_proxy = connect_to_instance_protocol::<fs_realm::ControllerMarker>(
        &moniker,
        OpenDirType::Exposed,
        &realm_query_proxy,
    )
    .await?;

    fs_realm_proxy
        .format(client_end, &filesystem, format_options)
        .await
        .context("Transport error on format")?
        .map_err(zx::Status::err_from_raw)
        .context("Failed to format block device")?;
    Ok(())
}
