// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_scrutiny_packages_list_args::ScrutinyPackagesCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool, Result};
use scrutiny_frontend::Scrutiny;

#[derive(FfxTool)]
#[target(None)]
pub struct ScrutinyPackagesTool {
    #[command]
    pub cmd: ScrutinyPackagesCommand,
}

fho::embedded_plugin!(ScrutinyPackagesTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ScrutinyPackagesTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        let artifacts = if self.cmd.recovery {
            Scrutiny::from_product_bundle_recovery(&self.cmd.product_bundle)
        } else {
            Scrutiny::from_product_bundle(&self.cmd.product_bundle)
        }?
        .collect()?;
        let packages = artifacts.get_package_urls()?;
        let s = serde_json::to_string_pretty(&packages).map_err(|e| fho::Error::User(e.into()))?;
        println!("{}", s);
        Ok(())
    }
}
