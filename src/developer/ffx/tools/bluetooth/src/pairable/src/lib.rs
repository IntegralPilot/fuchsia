// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::async_trait::async_trait;
use ::ffx_bluetooth_pairable_args::{PairableCommand, PairableSubCommand};
use ::fho::{AvailabilityFlag, FfxMain, FfxTool, Result};
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_bluetooth_sys::{
    InputCapability, OutputCapability, PairingDelegateMarker, PairingProxy,
};
use ffx_bluetooth_common::handle_pairing_delegate_requests;
use ffx_writer::{SimpleWriter, ToolIO as _};
use target_holders::toolbox;

#[derive(FfxTool)]
#[check(AvailabilityFlag("bluetooth.enabled"))]
pub struct PairableTool {
    #[command]
    cmd: PairableCommand,
    #[with(toolbox())]
    pairing_proxy: PairingProxy,
}

fho::embedded_plugin!(PairableTool);
#[async_trait(?Send)]
impl FfxMain for PairableTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(mut self, mut writer: Self::Writer) -> Result<()> {
        match self.cmd.subcommand.clone() {
            // ffx bluetooth pairable once
            PairableSubCommand::Once(ref cmd) => {
                writer.line("Allowing pairing")?;
                self.allow_pairing(&cmd.input_capability, &cmd.output_capability, &mut writer)
                    .await?;
            }
        }
        Ok(())
    }
}

impl PairableTool {
    // Enable pairing for this device to allow incoming pairing requests.
    async fn allow_pairing(
        &self,
        input_cap: &InputCapability,
        output_cap: &OutputCapability,
        writer: &mut SimpleWriter,
    ) -> Result<()> {
        let (pairing_delegate_client, delegate_stream) =
            self.pairing_proxy.domain().create_request_stream::<PairingDelegateMarker>();

        if let Err(err) = self.pairing_proxy.set_pairing_delegate(
            *input_cap,
            *output_cap,
            pairing_delegate_client,
        ) {
            return Err(fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")));
        }
        handle_pairing_delegate_requests(delegate_stream, None, writer).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_bluetooth_pairable_args::{parse_input_capability, parse_output_capability};

    #[test]
    fn test_parse_input_capability() {
        let cases = vec![
            ("NONE", Ok(InputCapability::None)),
            ("Confirmation", Ok(InputCapability::Confirmation)),
            ("keyboard", Ok(InputCapability::Keyboard)),
            (
                "TEST",
                Err("input capability should be 'none', 'confirmation', or 'keyboard'".to_string()),
            ),
        ];
        for (input_str, expected) in cases {
            assert_eq!(parse_input_capability(input_str), expected);
        }
    }

    #[test]
    fn test_parse_output_capability() {
        let cases = vec![
            ("NONE", Ok(OutputCapability::None)),
            ("display", Ok(OutputCapability::Display)),
            ("TEST", Err("output capability should be 'none' or 'display'".to_string())),
        ];
        for (output_str, expected) in cases {
            assert_eq!(parse_output_capability(output_str), expected);
        }
    }
}
