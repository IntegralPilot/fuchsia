// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_selftest_experiment_args::ExperimentCommand;
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool, Result};

#[derive(FfxTool)]
#[check(AvailabilityFlag("selftest.experiment"))]
pub struct ExperimentTool {
    #[command]
    pub cmd: ExperimentCommand,
}

fho::embedded_plugin!(ExperimentTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ExperimentTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> Result<()> {
        Ok(())
    }
}
