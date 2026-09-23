// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::storage_info::StorageInfo;
use fidl_fuchsia_hwinfo::{Architecture, ArchitectureUnknown, BoardInfo, DeviceInfo, ProductInfo};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::collections::HashMap;

// In order to make BoardInfo (a type from another crate) Serializable,
// wrap it with a local type GumshoeBoardInfo that represents "only the
// fields Gumshoe is allowed to externalize."
struct GumshoeBoardInfo(BoardInfo);

impl Serialize for GumshoeBoardInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Only include Security-approved board fields for Gumshoe here.
        let mut board_info = serializer.serialize_struct("GumshoeBoardInfo", 3)?;
        board_info.serialize_field("name", &self.0.name)?;
        board_info.serialize_field("revision", &self.0.revision)?;
        board_info.serialize_field(
            "cpu_architecture",
            &match self.0.cpu_architecture {
                Some(Architecture::X64) => Some("X64"),
                Some(Architecture::Arm64) => Some("Arm64"),
                Some(ArchitectureUnknown!()) => Some("Unknown"),
                None => None,
            },
        )?;
        board_info.end()
    }
}

struct GumshoeDeviceInfo(DeviceInfo);
impl Serialize for GumshoeDeviceInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Only include Security-approved device fields for Gumshoe here.
        let mut device_info = serializer.serialize_struct("GumshoeDeviceInfo", 2)?;
        device_info.serialize_field("retail_sku", &self.0.retail_sku)?;
        device_info.serialize_field("serial_number", &self.0.serial_number)?;
        device_info.end()
    }
}

#[derive(Serialize, Default)]
pub struct DeviceInfoImpl {
    board_info: Option<GumshoeBoardInfo>,
    device_info: Option<GumshoeDeviceInfo>,
    product_info: HashMap<String, Option<String>>,
    storage_info: Option<StorageInfo>,
}

/// Store of "stable" Device information, like product name and serial.
/// Used as a base for data passed to Handlebars template renderer.
impl DeviceInfoImpl {
    fn build_product_map(product_info: Option<ProductInfo>) -> HashMap<String, Option<String>> {
        let mut product_map = HashMap::new();
        match product_info {
            Some(info) => {
                println!("Product Information provided.");
                product_map.insert("SKU".to_string(), info.sku);
                product_map.insert("LANGUAGE".to_string(), info.language);
                product_map.insert("NAME".to_string(), info.name);
                product_map.insert("MODEL".to_string(), info.model);
                product_map.insert("MANUFACTURER".to_string(), info.manufacturer);
                product_map.insert("BUILD_DATE".to_string(), info.build_date);
                product_map.insert("BUILD_NAME".to_string(), info.build_name);
                product_map.insert("COLORWAY".to_string(), info.colorway);
                product_map.insert("DISPLAY".to_string(), info.display);
                product_map.insert("MEMORY".to_string(), info.memory);
                product_map.insert("NAND_STORAGE".to_string(), info.nand_storage);
                product_map.insert("EMMC_STORAGE".to_string(), info.emmc_storage);
                product_map.insert("MICROPHONE".to_string(), info.microphone);
                product_map.insert("AUDIO_AMPLIFIER".to_string(), info.audio_amplifier);
            }
            None => {
                println!("Product Information not provided");
            }
        }
        product_map
    }

    pub fn new(
        board_info: Option<BoardInfo>,
        device_info: Option<DeviceInfo>,
        product_info: Option<ProductInfo>,
        storage_info: Option<StorageInfo>,
    ) -> Self {
        DeviceInfoImpl {
            board_info: board_info.map(|b| GumshoeBoardInfo(b)),
            device_info: device_info.map(|d| GumshoeDeviceInfo(d)),
            product_info: Self::build_product_map(product_info),
            storage_info,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Verifies placeholder implementation of DeviceInfo contains nothing.
    fn no_product_values_when_not_provided() {
        let device = DeviceInfoImpl::default();

        assert_eq!(0, device.product_info.len());
    }

    #[test]
    fn product_values_present_when_provided() {
        let device = DeviceInfoImpl::new(None, None, Some(ProductInfo::default()), None);

        // Gumshoe extracts 14 fields from ProductInfo.
        assert_eq!(14, device.product_info.len());
    }
}
