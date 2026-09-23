// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use camino::Utf8PathBuf;
use delivery_blob::DeliveryBlobType;
use ffx_core::ffx_command;

#[ffx_command]
#[derive(ArgsInfo, FromArgs, Debug, Eq, PartialEq)]
#[argh(subcommand, name = "compress", description = "Compress one or more blobs")]
pub struct CompressCommand {
    #[argh(
        option,
        short = 't',
        long = "type",
        description = "delivery blob type",
        default = "DeliveryBlobType::Type1",
        from_str_fn(parse_delivery_blob)
    )]
    pub delivery_type: DeliveryBlobType,

    #[argh(option, short = 'o', description = "output compressed blobs into this directory")]
    pub output: Utf8PathBuf,

    #[argh(switch, description = "use the merkle root of each file as its output name")]
    pub hash_as_name: bool,

    #[argh(positional, description = "compress these files")]
    pub paths: Vec<Utf8PathBuf>,
}

fn parse_delivery_blob(value: &str) -> Result<DeliveryBlobType, String> {
    let value: u32 = value.parse().map_err(|_| "Delivery blob must be a u32 integer")?;

    DeliveryBlobType::try_from(value).map_err(|err| err.to_string())
}
