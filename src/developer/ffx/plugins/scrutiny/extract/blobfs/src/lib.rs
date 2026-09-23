// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_scrutiny_blobfs_args::ScrutinyBlobfsCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool, Result};
use scrutiny_frontend::BlobFsExtractController;

#[derive(FfxTool)]
#[target(None)]
pub struct ScrutinyBlobfsTool {
    #[command]
    pub cmd: ScrutinyBlobfsCommand,
}

fho::embedded_plugin!(ScrutinyBlobfsTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ScrutinyBlobfsTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        let value = BlobFsExtractController::extract(self.cmd.input, self.cmd.output)?;
        let s = serde_json::to_string_pretty(&value).map_err(|e| fho::Error::User(e.into()))?;
        println!("{}", s);
        Ok(())
    }
}
