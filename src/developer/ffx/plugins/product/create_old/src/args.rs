// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;

/// Create a Product Bundle using the outputs of Product Assembly.
#[derive(Clone, ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "create-old")]
pub struct CreateCommand {
    /// product.board label. e.g. "workstation_eng.x64".
    #[argh(option)]
    pub product_name: String,

    /// path to an assembly manifest, which specifies images to put in slot A.
    #[argh(option)]
    pub system_a: Option<Utf8PathBuf>,

    /// path to an assembly manifest, which specifies images to put in slot B.
    #[argh(option)]
    pub system_b: Option<Utf8PathBuf>,

    /// path to an assembly manifest, which specifies images to put in slot R.
    #[argh(option)]
    pub system_r: Option<Utf8PathBuf>,

    /// path to the directory of TUF keys, which should include root.json, snapshot.json,
    /// targets.json, and timestamp.json. If provided, then a TUF repository will be created inside
    /// the product bundle and filled with the product blobs.
    #[argh(option)]
    pub tuf_keys: Option<Utf8PathBuf>,

    /// file containing the version of the Product to put in the update package.
    #[argh(option)]
    pub update_package_version_file: Option<Utf8PathBuf>,

    /// backstop OTA version.
    /// Fuchsia will reject updates with a lower epoch.
    #[argh(option)]
    pub update_package_epoch: Option<u64>,

    /// path to a Virtual Device Specification file to include in the product bundle. May be
    /// repeated to include multiple Virtual Devices.
    #[argh(option)]
    pub virtual_device: Vec<Utf8PathBuf>,

    /// name of a Virtual Device Specification file to mark as the "recommended" device
    /// for emulation.
    #[argh(option)]
    pub recommended_device: Option<String>,

    /// directory to write the product bundle.
    #[argh(option)]
    pub out_dir: Utf8PathBuf,

    /// the type of delivery blob the product needs.
    #[argh(option)]
    pub delivery_blob_type: Option<u32>,

    /// create a deprecated flash manifest for backward compatibility.
    #[argh(switch)]
    pub with_deprecated_flash_manifest: bool,

    /// an optional image size report to generate that indicates the sizes of the images when
    /// mapped to partitions.
    #[argh(option)]
    pub gerrit_size_report: Option<Utf8PathBuf>,
}
