// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_scrutiny_structured_config_args::ScrutinyStructuredConfigCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool, Result, bug};
use scrutiny_frontend::Scrutiny;
use scrutiny_utils::path::relativize_path;

#[derive(FfxTool)]
#[target(None)]
pub struct ScrutinyStructuredConfigTool {
    #[command]
    pub cmd: ScrutinyStructuredConfigCommand,
}

fho::embedded_plugin!(ScrutinyStructuredConfigTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ScrutinyStructuredConfigTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        let mut scrutiny = if self.cmd.recovery {
            Scrutiny::from_product_bundle_recovery(&self.cmd.product_bundle)
        } else {
            Scrutiny::from_product_bundle(&self.cmd.product_bundle)
        }?;

        scrutiny.set_component_tree_config_paths(&self.cmd.component_tree_config);

        let artifacts = scrutiny.collect()?;
        let response = artifacts.extract_structured_config()?;
        let result = serde_json::to_string_pretty(&response.components)
            .map_err(|e| bug!("prettifying response JSON: {e}"))?;
        std::fs::write(&self.cmd.output, result)
            .map_err(|e| bug!("writing output to file: {e}"))?;

        let relative_dep_paths = response
            .deps
            .iter()
            .map(|dep_path| relativize_path(&self.cmd.build_path, dep_path).display().to_string())
            .collect::<Vec<_>>();
        let depfile_contents =
            format!("{}: {}", self.cmd.output.display(), relative_dep_paths.join(" "));
        std::fs::write(&self.cmd.depfile, depfile_contents)
            .map_err(|e| bug!("writing depfile: {e}"))?;
        Ok(())
    }
}
