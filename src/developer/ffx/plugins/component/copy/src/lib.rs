// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug::copy::copy_cmd;
use component_debug_fdomain as component_debug;
use errors::ffx_bail;
use ffx_component::rcs::connect_to_realm_query;
use ffx_component_copy_args::CopyComponentCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct CopyTool {
    #[command]
    cmd: CopyComponentCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(CopyTool);

#[async_trait(?Send)]
impl FfxMain for CopyTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let query_proxy = connect_to_realm_query(&self.rcs).await?;
        let CopyComponentCommand { paths, verbose } = self.cmd;

        match copy_cmd(&query_proxy, paths, verbose, writer).await {
            Ok(_) => Ok(()),
            Err(e) => ffx_bail!("{}", e),
        }
    }
}
