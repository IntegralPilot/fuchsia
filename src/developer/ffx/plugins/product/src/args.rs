// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum ProductSubCommand {
    CreateOld(ffx_product_create_old::CreateCommand),
    Download(ffx_product_download::DownloadCommand),
    ExtractBlobs(ffx_product_extract_blobs::ExtractBlobsCommand),
    GetArtifacts(ffx_product_get_artifacts::GetArtifactsCommand),
    GetImagePath(ffx_product_get_image_path::GetImagePathCommand),
    GetRepository(ffx_product_get_repository::GetRepositoryCommand),
    GetVersion(ffx_product_get_version::GetVersionCommand),
    List(ffx_product_list::ListCommand),
    Lookup(ffx_product_lookup::LookupCommand),
    Show(ffx_product_show::ShowCommand),
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(
    subcommand,
    name = "product",
    description = "Discover and access product bundle metadata and image data."
)]
pub struct ProductCommand {
    #[argh(subcommand)]
    pub subcommand: ProductSubCommand,
}
