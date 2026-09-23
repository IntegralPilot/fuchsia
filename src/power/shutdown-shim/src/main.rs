// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints;
use fidl_fuchsia_io as fio;
use fuchsia_runtime::{take_startup_handle, HandleType};

#[fuchsia::main]
async fn main() {
    let Some(directory_request) = take_startup_handle(HandleType::DirectoryRequest.into()) else {
        eprintln!("[shutdown-shim]: could not obtain directory request handle");
        std::process::exit(1);
    };
    let config_vmo: Option<zx::Vmo> =
        take_startup_handle(HandleType::ComponentConfigVmo.into()).map(|handle| handle.into());
    let (svc, server) = endpoints::create_proxy::<fio::DirectoryMarker>();
    let Ok(()) = fdio::open("/svc", fio::PERM_READABLE, server.into()) else {
        eprintln!("[shutdown-shim]: could not obtain namespace");
        std::process::exit(1);
    };
    if let Err(err) = shutdown_shim::main(svc, directory_request.into(), config_vmo).await {
        eprintln!("[shutdown-shim]: {err}");
        std::process::exit(1);
    }
}
