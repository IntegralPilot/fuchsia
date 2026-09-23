// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use fdomain_fuchsia_settings::AccessibilityProxy;
use ffx_setui_accessibility_args::{Accessibility, SubCommandEnum};
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use target_holders::moniker;

pub use utils;

mod add_caption;
mod set;
mod watch;

#[derive(FfxTool)]
#[check(AvailabilityFlag("setui"))]
pub struct AccessibilityTool {
    #[command]
    cmd: Accessibility,
    #[with(moniker("/core/setui_service"))]
    accessibility_proxy: AccessibilityProxy,
}

fho::embedded_plugin!(AccessibilityTool);

#[async_trait(?Send)]
impl FfxMain for AccessibilityTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        run_command(self.accessibility_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

async fn run_command<W: std::io::Write>(
    accessibility_proxy: AccessibilityProxy,
    accessibility: Accessibility,
    writer: &mut W,
) -> Result<()> {
    match accessibility.subcommand {
        SubCommandEnum::AddCaption(args) => {
            add_caption::add_caption(accessibility_proxy, args, writer).await
        }
        SubCommandEnum::Set(args) => set::set(accessibility_proxy, args, writer).await,
        SubCommandEnum::Watch(_) => watch::watch(accessibility_proxy, writer).await,
    }
}
