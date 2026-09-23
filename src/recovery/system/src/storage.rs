// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fdr_lib::execute_reset;
use fidl_fuchsia_io as fio;

// Wipe and re-provision FVM. This will wipe both data and blobfs. This should only
// be used in OTA flows, since wiping blobfs will result in an unusable primary system.
// TODO(https://fxbug.dev/395155386): Remove workflows that invoke this function, since we do not
// use recovery OTA functionality in production.
pub async fn wipe_storage() -> Result<fio::DirectoryProxy, Error> {
    Err(zx::Status::NOT_SUPPORTED)
        .context("WipeStorage is not supported, see https://fxbug.dev/395155386.")
}

// Instead of formatting the data partition directly, reset it via the factory reset service.
// The data partition is reformatted on first boot under normal circumstances and will do so
// after a reboot following being reset.
// This immediately reboots the device and needs to run separately from wipe_storage for now.
pub async fn wipe_data() -> Result<(), Error> {
    execute_reset().await.context("Failed to factory reset data")
}
