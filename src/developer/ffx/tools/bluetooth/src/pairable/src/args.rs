// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fdomain_fuchsia_bluetooth_sys::{InputCapability, OutputCapability};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "pairable",
    description = "Allow pairing with this device.",
    example = "ffx bluetooth pairable"
)]
pub struct PairableCommand {
    #[argh(subcommand)]
    pub subcommand: PairableSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand)]
pub enum PairableSubCommand {
    Once(OnceCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "once",
    description = "Allow one incoming pairing request. Auto-accepts any valid pairing request and \
stops accepting additional pairing requests after a successful pairing. If a pairing delegate is \
already running (e.g. in normal operation of a production device), this operation will fail. \
Note: Because this command auto-accepts requests, specifying capabilities other than `none` may \
cause pairing to fail if the negotiated method requires interactive passkey entry or display.",
    example = "Basic usage:

    $ ffx bluetooth pairable once

To specify this device's input capability, use `--input-capability` or `-i`. To specify this \
device's output capability, use `--output-capability` or `-o`.

    $ ffx bluetooth pairable once -i none -o none"
)]
pub struct OnceCommand {
    /// set this value based on this device's ability to respond to pairing requests. Allowed values
    /// are "none", "confirmation", and "keyboard". These values mean that this device cannot
    /// respond, can respond "yes" or "no", or can type a numerical code and send a signal,
    /// respectively. Default value is "none"
    #[argh(
        option,
        long = "input-capability",
        short = 'i',
        default = "InputCapability::None",
        from_str_fn(parse_input_capability)
    )]
    pub input_capability: InputCapability,

    /// set this value based on this device's ability to display info for pairing requests. Allowed
    /// values are "none" and "display". These values mean that this device has no display or has a
    /// display that can show a six-digit decimal number, respectively. Default value is "none"
    #[argh(
        option,
        long = "output-capability",
        short = 'o',
        default = "OutputCapability::None",
        from_str_fn(parse_output_capability)
    )]
    pub output_capability: OutputCapability,
}

pub fn parse_input_capability(s: &str) -> Result<InputCapability, String> {
    match s.to_ascii_lowercase().as_str() {
        "none" => Ok(InputCapability::None),
        "confirmation" => Ok(InputCapability::Confirmation),
        "keyboard" => Ok(InputCapability::Keyboard),
        _ => Err("input capability should be 'none', 'confirmation', or 'keyboard'".to_string()),
    }
}

pub fn parse_output_capability(s: &str) -> Result<OutputCapability, String> {
    match s.to_ascii_lowercase().as_str() {
        "none" => Ok(OutputCapability::None),
        "display" => Ok(OutputCapability::Display),
        _ => Err("output capability should be 'none' or 'display'".to_string()),
    }
}
