// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fho::subtool_suite::{FfxSubtoolSuite, Subtool, SubtoolBox, SubtoolSuite, ToolSuiteCommand};
use fho::{FfxTool, FhoEnvironment, Result};

mod args;
use args::{ProductCommand, ProductSubCommand};

impl ToolSuiteCommand for ProductCommand {
    type SubCommand = ProductSubCommand;
    fn into_subcommand(self) -> Self::SubCommand {
        self.subcommand
    }
}

pub struct ProductSuite;

#[async_trait::async_trait(?Send)]
impl SubtoolSuite for ProductSuite {
    type Command = ProductCommand;

    async fn new_subtool(
        env: FhoEnvironment,
        subcommand: ProductSubCommand,
    ) -> Result<Box<dyn SubtoolBox>> {
        Ok(match subcommand {
            ProductSubCommand::CreateOld(cmd) => {
                Subtool::new(ffx_product_create_old::ProductCreateTool::from_env(env, cmd).await?)
            }
            ProductSubCommand::Download(cmd) => {
                Subtool::new(ffx_product_download::PbDownloadTool::from_env(env, cmd).await?)
            }
            ProductSubCommand::ExtractBlobs(cmd) => Subtool::new(
                ffx_product_extract_blobs::PbExtractBlobsTool::from_env(env, cmd).await?,
            ),
            ProductSubCommand::GetArtifacts(cmd) => Subtool::new(
                ffx_product_get_artifacts::PbGetArtifactsTool::from_env(env, cmd).await?,
            ),
            ProductSubCommand::GetImagePath(cmd) => Subtool::new(
                ffx_product_get_image_path::PbGetImagePathTool::from_env(env, cmd).await?,
            ),
            ProductSubCommand::GetRepository(cmd) => Subtool::new(
                ffx_product_get_repository::ProductGetRepoTool::from_env(env, cmd).await?,
            ),
            ProductSubCommand::GetVersion(cmd) => {
                Subtool::new(ffx_product_get_version::PbGetVersionTool::from_env(env, cmd).await?)
            }
            ProductSubCommand::List(cmd) => {
                Subtool::new(ffx_product_list::ProductListTool::from_env(env, cmd).await?)
            }
            ProductSubCommand::Lookup(cmd) => {
                Subtool::new(ffx_product_lookup::PbLookupTool::from_env(env, cmd).await?)
            }
            ProductSubCommand::Show(cmd) => {
                Subtool::new(ffx_product_show::ProductShowTool::from_env(env, cmd).await?)
            }
        })
    }
}

pub type ProductSuiteTool = FfxSubtoolSuite<ProductSuite>;
