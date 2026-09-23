// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use termion::{color, style};

pub use error::DoctorUtilsError;
pub use recorder::{DoctorRecorder, Recorder};

mod error;
mod recorder;

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
/// The result of the check.
pub enum CheckResult {
    /// The check passed.
    Passed,
    /// The check failed.
    Failed,
    /// The check is presenting info.
    Info,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
/// Message from a doctor check run by a plugin.
pub struct DoctorCheck {
    /// The name of the check to run
    pub name: String,
    /// Message returned by the check
    pub message: String,
    /// The result of the check. Info level ignores name in output.
    pub result: CheckResult,
}

impl fmt::Display for DoctorCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let preamble = match self.result {
            CheckResult::Passed => {
                format!("[{}✓{}] {}", color::Fg(color::Green), style::Reset, self.name)
            }
            CheckResult::Failed => {
                format!("[{}✗{}] {}", color::Fg(color::Red), style::Reset, self.name)
            }
            CheckResult::Info => format!("[{}i{}]", color::Fg(color::Yellow), style::Reset),
        };
        write!(f, "{}: {}", preamble, self.message)
    }
}
