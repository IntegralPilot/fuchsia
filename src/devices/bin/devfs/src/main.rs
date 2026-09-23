// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_process as fprocess;
use log::error;

/// This main() is for the standalone devfs binary, which is used by the test component.
#[fuchsia::main]
async fn main() {
    let ns = fdio::Namespace::installed().expect("Failed to get installed namespace");
    let ns_entries = ns.export().expect("Failed to export namespace entries");
    let ns_entries = ns_entries
        .into_iter()
        .map(|e| fprocess::NameInfo { path: e.path, directory: e.handle.into() })
        .collect();

    let Some(directory_request) =
        fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleType::DirectoryRequest.into())
    else {
        panic!("[devfs] could not obtain directory request handle");
    };
    let Some(lifecycle) =
        fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleType::Lifecycle.into())
    else {
        panic!("[devfs] could not obtain lifecycle handle");
    };
    if let Err(err) = devfs::main(ns_entries, directory_request.into(), lifecycle.into()).await {
        error!(err:%; "[devfs] error");
        std::process::exit(1);
    }
}
