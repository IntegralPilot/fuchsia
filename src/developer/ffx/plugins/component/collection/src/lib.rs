// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug::cli::{collection_list_cmd, collection_show_cmd};
use component_debug_fdomain as component_debug;
use errors::ffx_error;
use ffx_component::rcs::connect_to_realm_query;
use ffx_component_collection_args::{CollectionCommand, ShowArgs, SubCommandEnum};
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;
#[derive(FfxTool)]
pub struct CollectionTool {
    #[command]
    cmd: CollectionCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(CollectionTool);

#[async_trait(?Send)]
impl FfxMain for CollectionTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let realm_query = connect_to_realm_query(&self.rcs).await?;

        // All errors from component_debug library are user-visible.
        match self.cmd.subcommand {
            SubCommandEnum::List(_) => collection_list_cmd(realm_query, writer).await,
            SubCommandEnum::Show(ShowArgs { query }) => {
                collection_show_cmd(query, realm_query, writer).await
            }
        }
        .map_err(|e| ffx_error!(e))?;

        Ok(())
    }
}
