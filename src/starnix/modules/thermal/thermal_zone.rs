// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_hardware_temperature as ftemperature;

#[derive(Clone)]
pub struct ThermalZone {
    pub id: u32,
    pub proxy: ftemperature::DeviceProxy,
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SensorProps {
    pub name: String,
}
