// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow};
use argh::FromArgs;
use component_debug::dirs::{OpenDirType, connect_to_instance_protocol};
use fidl_fuchsia_fs_realm::ControllerMarker;
use fidl_fuchsia_sys2::RealmQueryMarker;
use fuchsia_component::client::connect_to_protocol_at_path;

use std::path::Path;

#[derive(FromArgs)]
#[argh(description = "A utility for unmounting a filesystem instance running on a block device.")]
struct Options {
    /// the path at which the filesystem instance was mounted on the block device
    #[argh(positional)]
    mount_path: String,
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let opt: Options = argh::from_env();
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

    // Connect to fs_realm
    const REALM_QUERY_SERVICE_PATH: &str = "/svc/fuchsia.sys2.RealmQuery.root";
    let realm_query_proxy =
        connect_to_protocol_at_path::<RealmQueryMarker>(REALM_QUERY_SERVICE_PATH)?;
    let moniker = "./core/fs_realm".try_into().unwrap();
    let fs_realm_proxy = connect_to_instance_protocol::<ControllerMarker>(
        &moniker,
        OpenDirType::Exposed,
        &realm_query_proxy,
    )
    .await?;

    fs_realm_proxy
        .unmount(filename_str)
        .await
        .context("Transport error on format")?
        .map_err(zx::Status::err_from_raw)
        .context("Failed to format block device")?;
    Ok(())
}
