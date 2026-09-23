// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::fmt::Display;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetStateOption {
    Down,
    Up,
    Fastboot,
    Product,
}

impl FromStr for TargetStateOption {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "down" => Ok(TargetStateOption::Down),
            "up" => Ok(TargetStateOption::Up),
            "fastboot" => Ok(TargetStateOption::Fastboot),
            "product" => Ok(TargetStateOption::Product),
            _ => Err(format!(
                "invalid state '{s}'. Expected one of: 'down', 'up', 'fastboot', 'product'"
            )),
        }
    }
}

impl Display for TargetStateOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetStateOption::Down => write!(f, "down"),
            TargetStateOption::Up => write!(f, "up"),
            TargetStateOption::Fastboot => write!(f, "fastboot"),
            TargetStateOption::Product => write!(f, "product"),
        }
    }
}

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "wait",
    description = "Wait for a target to reach a specific state.",
    error_code(1, "Timeout while waiting for target")
)]
pub struct WaitOptions {
    #[argh(option, short = 't', default = "120")]
    /// the timeout in seconds [default = 120]. A value of 0 implies no timeout.
    pub timeout: u64,

    #[argh(switch, short = 'd', description = "wait for target to go down")]
    /// wait for the target to go down
    pub down: bool,

    #[argh(option)]
    /// wait for the target to reach a specific state ('down', 'up', 'fastboot', 'product')
    pub state: Option<TargetStateOption>,
}

impl WaitOptions {
    pub fn get_target_state(&self) -> Result<TargetStateOption, String> {
        if self.down && self.state.is_some() {
            return Err("Cannot specify both --down and --state".to_string());
        }
        if self.down {
            Ok(TargetStateOption::Down)
        } else {
            Ok(self.state.unwrap_or(TargetStateOption::Up))
        }
    }
}

impl Default for WaitOptions {
    fn default() -> Self {
        Self { timeout: 120, down: false, state: None }
    }
}
