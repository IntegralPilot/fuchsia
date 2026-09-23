// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_fuchsia_pkg_garbagecollector as fpkg_gc;
use ffx_target_package_gc_args::GcCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{Error, FfxMain, FfxTool, Result, bug, user_error};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use target_holders::toolbox;

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successfully collected all garbage.
    Ok {},
    /// Unexpected error with string denoting error message.
    UnexpectedError { message: String },
    /// A known error that can be reported to the user.
    UserError { message: String },
}

#[derive(FfxTool)]
pub struct GcTool {
    #[command]
    _cmd: GcCommand,
    #[with(toolbox())]
    space_manager_proxy: fpkg_gc::ManagerProxy,
}

fho::embedded_plugin!(GcTool);

#[async_trait(?Send)]
impl FfxMain for GcTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        match self.gc_cmd().await {
            Ok(()) => {
                writer.machine(&CommandStatus::Ok {})?;
                Ok(())
            }
            Err(e @ Error::User(_)) => {
                writer.machine(&CommandStatus::UserError { message: e.to_string() })?;
                Err(e)
            }
            Err(e) => {
                writer.machine(&CommandStatus::UnexpectedError { message: e.to_string() })?;
                Err(e)
            }
        }
    }
}

impl GcTool {
    pub async fn gc_cmd(&self) -> Result<()> {
        let space_manager = &self.space_manager_proxy;
        space_manager
            .gc()
            .await
            .map_err(|err| bug!("Garbage collection failed with error: {:?}", err))?
            .map_err(|err| match err {
                fpkg_gc::GcError::Internal => {
                    user_error!("Garbage collection failed with an internal error.")
                }
                fpkg_gc::GcError::PendingCommit => {
                    user_error!(
                        "Garbage collection is blocked until the current system is committed."
                    )
                }
                fpkg_gc::GcError::__SourceBreaking { unknown_ordinal } => {
                    bug!("Unexpected ordinal in FIDL: {}", unknown_ordinal)
                }
            })
    }
}
