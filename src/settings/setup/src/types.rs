// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};
use settings_common::inspect::event::Nameable;

#[derive(PartialEq, Default, Debug, Clone, Copy, Deserialize, Serialize)]
pub struct SetupInfo {
    pub configuration_interfaces: ConfigurationInterfaceFlags,
}

impl Nameable for SetupInfo {
    const NAME: &'static str = "setup";
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ConfigurationInterfaceFlags: u32 {
        const ETHERNET = 1 << 0;
        const WIFI = 1 << 1;
        const DEFAULT = Self::WIFI.bits();
    }
}

impl Default for ConfigurationInterfaceFlags {
    fn default() -> Self {
        Self::DEFAULT
    }
}

bitflags_serde_legacy::impl_traits!(ConfigurationInterfaceFlags);

#[derive(Debug, PartialEq, Copy, Clone)]
pub struct SetConfigurationInterfacesParams {
    pub config_interfaces_flags: ConfigurationInterfaceFlags,
    // Set true to reboot the device, otherwise, set it as false.
    pub should_reboot: bool,
}
