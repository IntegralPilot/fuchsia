// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Device link layer configuration types.

use net_types::ip::Ip;
use netstack3_base::{Device, DeviceIdContext};
use netstack3_ip::nud::{NudUserConfig, NudUserConfigUpdate};

/// Device ARP configuration.
#[derive(Clone, Debug)]
pub struct ArpConfiguration {
    /// NUD over ARP configuration.
    pub nud: NudUserConfig,
}

/// Device NDP configuration.
#[derive(Clone, Debug)]
pub struct NdpConfiguration {
    /// NUD over NDP configuration.
    pub nud: NudUserConfig,
}

/// Device link layer configuration.
#[derive(Clone, Debug)]
pub struct DeviceConfiguration {
    /// ARP configurations.
    ///
    /// Only present if the device supports ARP.
    pub arp: Option<ArpConfiguration>,
    /// NDP configurations.
    ///
    /// Only present if the device supports NDP.
    pub ndp: Option<NdpConfiguration>,
}
/// An update to apply to ARP configurations.
///
/// Only fields with variant `Some` are requested to be updated.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArpConfigurationUpdate {
    /// NUD over ARP configuration update.
    pub nud: Option<NudUserConfigUpdate>,
}

/// An update to apply to NDP configurations.
///
/// Only fields with variant `Some` are requested to be updated.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NdpConfigurationUpdate {
    /// NUD over NDP configuration update.
    pub nud: Option<NudUserConfigUpdate>,
}

/// An update to apply to device configurations.
///
/// Only fields with variant `Some` are requested to be updated.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceConfigurationUpdate {
    /// ARP configuration update.
    pub arp: Option<ArpConfigurationUpdate>,
    /// NDP configuration update.
    pub ndp: Option<NdpConfigurationUpdate>,
}

/// Errors observed updating device configuration.
#[derive(Debug)]
pub enum DeviceConfigurationUpdateError {
    /// ARP is not supported for the requested device.
    ArpNotSupported,
    /// NDP is not supported for the requested device.
    NdpNotSupported,
}

/// A trait abstracting device configuration.
///
/// This trait allows the device API to perform device confiuration at the
/// device layer.
pub trait DeviceConfigurationContext<D: Device>: DeviceIdContext<D> {
    /// Calls the callback with a mutable reference to the NUD user
    /// configuration for IP version `I`.
    ///
    /// If the device does not support NUD, the callback is called with `None`,
    fn with_nud_config<I: Ip, O, F: FnOnce(Option<&NudUserConfig>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        f: F,
    ) -> O;

    /// Calls the callback with a mutable reference to the NUD user
    /// configuration for IP version `I`.
    ///
    /// If the device does not support NUD, the callback is called with `None`,
    fn with_nud_config_mut<I: Ip, O, F: FnOnce(Option<&mut NudUserConfig>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        f: F,
    ) -> O;
}
