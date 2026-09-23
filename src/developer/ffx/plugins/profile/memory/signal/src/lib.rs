// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of the `signal` subcommand.

use anyhow::Result;
use async_trait::async_trait;
use errors::ffx_error;
use fdomain_fuchsia_memory_debug::MemoryPressureProxy;
use ffx_profile_memory_signal_args::SignalCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::moniker;

#[derive(FfxTool)]
pub struct MemorySignalTool {
    #[command]
    cmd: SignalCommand,
    #[with(moniker("/core/memory_pressure_signaler"))]
    debugger_proxy: MemoryPressureProxy,
}

fho::embedded_plugin!(MemorySignalTool);

#[async_trait(?Send)]
impl FfxMain for MemorySignalTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    /// Forwards the specified memory pressure level to the fuchsia.memory.debug.MemoryPressure FIDL
    /// interface.
    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        self.debugger_proxy
            .signal(self.cmd.level)
            .map_err(|err| ffx_error!("Failed to call MemoryPressure/Signal: {err}"))?;
        Ok(())
    }
}
