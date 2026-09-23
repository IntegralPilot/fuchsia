// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow};
use argh::FromArgs;
use component_debug::dirs::{OpenDirType, connect_to_instance_protocol};
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_device::ControllerMarker;
use fidl_fuchsia_fs_realm as fs_realm;
use fidl_fuchsia_sys2::RealmQueryMarker;
use fuchsia_component::client::{connect_channel_to_protocol_at_path, connect_to_protocol_at_path};
use std::path::Path;

#[derive(FromArgs)]
#[argh(description = "A utility for mounting a filesystem instance on a block device.")]
struct Options {
    /// the device path of the block device
    #[argh(positional)]
    device_path: String,

    /// the path at which the filesystem instance will be mounted
    #[argh(positional)]
    mount_path: String,

    /// read only flag
    #[argh(option)]
    read_only: Option<bool>,

    /// verbose flag
    #[argh(option)]
    verbose: Option<bool>,
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let opt: Options = argh::from_env();
    let device_path = opt.device_path;
    let mount_path = Path::new(&opt.mount_path);
    // Make sure that path can be canonicalized and starts with mnt
    if let Some(parent_path) = mount_path.parent() {
        if parent_path.to_str() != Some("/mnt") {
            let canonical_parent_path = parent_path.canonicalize().context("Bad mount path")?;
            if canonical_parent_path.to_str() != Some("/mnt") {
                return Err(anyhow!("Only mounts in /mnt are supported."));
            }
        }
    } else {
        return Err(anyhow!("Only mounts with a parent path are supported"));
    }

    let filename = mount_path.file_name().ok_or_else(|| anyhow!("Failed to get file name"))?;
    let filename_str =
        filename.to_str().ok_or_else(|| anyhow!("Failed to convert filename from OsStr to str"))?;

    let mut mount_options = fs_realm::MountOptions::default();
    mount_options.read_only = opt.read_only;
    mount_options.verbose = opt.verbose;

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
        .mount(client_end, filename_str, mount_options)
        .await
        .context("Transport error on mount")?
        .map_err(zx::Status::err_from_raw)
        .context("Failed to mount block device")?;
    Ok(())
}
