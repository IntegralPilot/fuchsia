// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth_sys as sys;
use std::str::FromStr;

use crate::error::Error;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputCapability {
    None,
    Confirmation,
    Keyboard,
}

impl FromStr for InputCapability {
    type Err = Error;

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        match src {
            "none" => Ok(InputCapability::None),
            "confirmation" => Ok(InputCapability::Confirmation),
            "keyboard" => Ok(InputCapability::Keyboard),
            cap => Err(Error::other(&format!("invalid input capability: {cap}"))),
        }
    }
}

impl Into<sys::InputCapability> for InputCapability {
    fn into(self) -> sys::InputCapability {
        match self {
            InputCapability::None => sys::InputCapability::None,
            InputCapability::Confirmation => sys::InputCapability::Confirmation,
            InputCapability::Keyboard => sys::InputCapability::Keyboard,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutputCapability {
    None,
    Display,
}

impl FromStr for OutputCapability {
    type Err = Error;

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        match src {
            "none" => Ok(OutputCapability::None),
            "display" => Ok(OutputCapability::Display),
            cap => Err(Error::other(&format!("invalid output capability: {cap}"))),
        }
    }
}

impl Into<sys::OutputCapability> for OutputCapability {
    fn into(self) -> sys::OutputCapability {
        match self {
            OutputCapability::None => sys::OutputCapability::None,
            OutputCapability::Display => sys::OutputCapability::Display,
        }
    }
}
