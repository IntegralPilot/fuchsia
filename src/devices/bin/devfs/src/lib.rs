// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, bail, format_err};
use fidl::AsHandleRef;
use fidl::endpoints::ServerEnd;
use fuchsia_component::directory::AsRefDirectory;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use log::info;
use std::collections::HashSet;
use vfs::remote;
use {
    fidl_fuchsia_io as fio, fidl_fuchsia_process as fprocess, fidl_fuchsia_process_lifecycle as fpl,
};

pub async fn main(
    ns_entries: Vec<fprocess::NameInfo>,
    directory_request: ServerEnd<fio::DirectoryMarker>,
    lifecycle: ServerEnd<fpl::LifecycleMarker>,
) -> Result<(), anyhow::Error> {
    if lifecycle.as_handle_ref().is_invalid() {
        bail!("No valid handle found for lifecycle events");
    }
    if directory_request.as_handle_ref().is_invalid() {
        bail!("No valid handle found for outgoing directory");
    }
    let Some(svc) = ns_entries.iter().find(|e| e.path == "/svc") else {
        bail!("No /svc in namespace");
    };
    svc.directory
        .as_ref_directory()
        .open("fuchsia.device.fs.lifecycle.Lifecycle", fio::Flags::empty(), lifecycle.into())
        .context("Failed to connect to fuchsia.device.fs.Lifecycle")?;

    let mut fs = ServiceFs::new();

    let mut expose = HashSet::new();
    expose.insert("dev");
    for entry in ns_entries {
        let path = entry
            .path
            .strip_prefix("/")
            .ok_or_else(|| format_err!("Encountered illegal namespace path: {}", entry.path))?;
        if !expose.remove(path) {
            continue;
        }
        fs.add_entry_at(path, remote::remote_dir(entry.directory.into_proxy()));
    }
    if !expose.is_empty() {
        let missing = expose.into_iter().collect::<Vec<_>>().join(",");
        bail!("Failed to expose all entries: {missing}");
    }

    info!("[devfs] Initialized.");

    fs.serve_connection(directory_request).context("failed to serve outgoing namespace")?;
    fs.collect::<()>().await;
    Err(format_err!("devfs ended unexpectedly"))
}
