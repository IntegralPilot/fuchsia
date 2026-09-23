// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "ota",
    description = "Runs OTA verification checks. Starting from a given update package hash, this fetches blobs from a delivery blob repository over HTTP. It follows the packages.json and packages' metadata to verify the presence and integrity of the full set of blobs that constitutes an update.",
    example = r#"To run the OTA verification checks against a build:

    $ ffx scrutiny verify ota \
        --update-package <merkle-hash-of-update-package> \
        --blob-server-url <url-of-blob-server>"#
)]
pub struct Command {
    /// the merkle hash of the update package
    #[argh(option)]
    pub update_package: String,

    /// url of the blob server to fetch artifacts from
    #[argh(option)]
    pub blob_server_url: String,

    /// optional delivery blob type to use (e.g. 1)
    #[argh(option)]
    pub delivery_blob_type: Option<u32>,
}
