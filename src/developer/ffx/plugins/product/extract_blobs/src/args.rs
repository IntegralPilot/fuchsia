// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;

/// Extract all blobs from a product bundle to a target directory.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "extract-blobs")]
pub struct ExtractBlobsCommand {
    /// path to the product bundle directory (optional, queries configuration if not set).
    #[argh(option, short = 'p')]
    pub product_bundle: Option<Utf8PathBuf>,

    /// path to the output directory where the blobs will be extracted.
    #[argh(option, short = 'o')]
    pub out_dir: Utf8PathBuf,

    /// slot to extract from (defaults to A).
    #[argh(option, short = 's', default = "String::from(\"A\")")]
    pub slot: String,

    /// specify the delivery blob type to compress extracted blobs to.
    /// If not specified, raw uncompressed blobs are returned.
    #[argh(option, short = 'd')]
    pub delivery_blob_type: Option<u32>,
}
