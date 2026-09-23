// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};

/// A mapping of a semantic label for a block device to board-specific identifiers for that block
/// device. The identifiers are used by fshost to match block devices as they come in, which then
/// are exported under the configured semantic label. Only the first block device that matches will
/// be exported, any others will be ignored.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BlockDeviceConfig {
    /// The semantic label, or name for the block device to be routed by.
    pub device: String,
    /// The identifiers to match against. The block device must match both the label and the parent
    /// identifiers to be selected for this label.
    pub from: BlockDeviceIdentifiers,
}

/// A set of identifiers used to match block devices.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BlockDeviceIdentifiers {
    /// The expected partition label for the block device.
    pub label: String,
    /// The expected parent for the block device.
    pub parent: BlockDeviceParent,
}

/// Possible parents for a block device.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BlockDeviceParent {
    /// The device is expected to be a child of the main system gpt.
    Gpt,
    /// The device is expected to be a child of a driver.
    Dev,
}
