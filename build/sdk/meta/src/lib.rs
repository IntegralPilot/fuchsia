// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[macro_use]
pub mod testing;
mod common;
mod ffx_tool;
mod host_tool;
mod json;
mod manifest;

// These need to be addressable from external code, because they have conflicting types
// named "Hardware" and "Cpu". In order to use one of these types in external code, it
// needs to specify which version of the type to use, e.g. virtual_device::Hardware, or
// the import will fail to locate the type.
pub mod virtual_device;

pub use crate::common::*;
pub use crate::ffx_tool::*;
pub use crate::host_tool::*;
pub use crate::json::JsonObject;
pub use crate::manifest::*;
pub use crate::virtual_device::{
    AudioDevice, DataAmount, InputDevice, Screen, VirtualDevice, VirtualDeviceManifest,
    VirtualDeviceV1, VsockDevice,
};
