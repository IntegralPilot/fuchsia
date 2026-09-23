// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_developer_remotecontrol as rcs;
use serde::Serialize;
use std::fmt::Write as _;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HostIdentityJson {
    pub nodename: Option<String>,
    pub boot_timestamp_nanos: Option<u64>,
    pub serial_number: Option<String>,
    pub product_config: Option<String>,
    pub board_config: Option<String>,
    pub boot_id: Option<u64>,
}

impl From<&rcs::IdentifyHostResponse> for HostIdentityJson {
    fn from(res: &rcs::IdentifyHostResponse) -> Self {
        Self {
            nodename: res.nodename.clone(),
            boot_timestamp_nanos: res.boot_timestamp_nanos,
            serial_number: res.serial_number.clone(),
            product_config: res.product_config.clone(),
            board_config: res.board_config.clone(),
            boot_id: res.boot_id,
        }
    }
}

pub fn format_text(res: &rcs::IdentifyHostResponse) -> String {
    let mut out = String::new();

    let nodename = res.nodename.as_deref().unwrap_or("N/A");
    let serial_number = res.serial_number.as_deref().unwrap_or("N/A");
    let product_config = res.product_config.as_deref().unwrap_or("N/A");
    let board_config = res.board_config.as_deref().unwrap_or("N/A");
    let boot_id = res.boot_id.map(|id| id.to_string()).unwrap_or_else(|| "N/A".to_string());
    let boot_timestamp_nanos =
        res.boot_timestamp_nanos.map(|ts| ts.to_string()).unwrap_or_else(|| "N/A".to_string());

    let _ = writeln!(out, "Nodename:            {nodename}");
    let _ = writeln!(out, "Serial Number:       {serial_number}");
    let _ = writeln!(out, "Product Config:      {product_config}");
    let _ = writeln!(out, "Board Config:        {board_config}");
    let _ = writeln!(out, "Boot ID:             {boot_id}");
    let _ = writeln!(out, "Boot Timestamp (ns): {boot_timestamp_nanos}");

    out
}

pub fn format_json(res: &rcs::IdentifyHostResponse) -> Result<String, serde_json::Error> {
    let json_obj = HostIdentityJson::from(res);
    serde_json::to_string(&json_obj)
}

pub fn format_pretty_json(res: &rcs::IdentifyHostResponse) -> Result<String, serde_json::Error> {
    let json_obj = HostIdentityJson::from(res);
    serde_json::to_string_pretty(&json_obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_text_complete() {
        let response = rcs::IdentifyHostResponse {
            nodename: Some("test-device".to_string()),
            boot_timestamp_nanos: Some(1700000000000000000),
            serial_number: Some("SN-12345".to_string()),
            product_config: Some("workbench_eng".to_string()),
            board_config: Some("x64".to_string()),
            boot_id: Some(999888777),
            ..Default::default()
        };

        let text = format_text(&response);
        assert!(text.contains("Nodename:            test-device"));
        assert!(text.contains("Serial Number:       SN-12345"));
        assert!(text.contains("Product Config:      workbench_eng"));
        assert!(text.contains("Board Config:        x64"));
        assert!(text.contains("Boot ID:             999888777"));
        assert!(text.contains("Boot Timestamp (ns): 1700000000000000000"));
    }

    #[test]
    fn test_format_text_missing_fields() {
        let response = rcs::IdentifyHostResponse::default();
        let text = format_text(&response);
        assert!(text.contains("Nodename:            N/A"));
        assert!(text.contains("Serial Number:       N/A"));
        assert!(text.contains("Product Config:      N/A"));
        assert!(text.contains("Board Config:        N/A"));
        assert!(text.contains("Boot ID:             N/A"));
        assert!(text.contains("Boot Timestamp (ns): N/A"));
    }

    #[test]
    fn test_format_json_complete() {
        let response = rcs::IdentifyHostResponse {
            nodename: Some("test-device".to_string()),
            boot_timestamp_nanos: Some(1700000000000000000),
            serial_number: Some("SN-12345".to_string()),
            product_config: Some("workbench_eng".to_string()),
            board_config: Some("x64".to_string()),
            boot_id: Some(999888777),
            ..Default::default()
        };

        let json_str = format_json(&response).expect("format json");
        assert!(!json_str.contains('\n'), "compact json must not contain newlines");
        let value: serde_json::Value =
            serde_json::from_str(&json_str).expect("parse generated json");

        assert_eq!(value["nodename"], "test-device");
        assert_eq!(value["serial_number"], "SN-12345");
        assert_eq!(value["product_config"], "workbench_eng");
        assert_eq!(value["board_config"], "x64");
        assert_eq!(value["boot_id"], 999888777);
        assert_eq!(value["boot_timestamp_nanos"], 1700000000000000000u64);
    }

    #[test]
    fn test_format_pretty_json_complete() {
        let response = rcs::IdentifyHostResponse {
            nodename: Some("test-device".to_string()),
            boot_timestamp_nanos: Some(1700000000000000000),
            serial_number: Some("SN-12345".to_string()),
            product_config: Some("workbench_eng".to_string()),
            board_config: Some("x64".to_string()),
            boot_id: Some(999888777),
            ..Default::default()
        };

        let json_str = format_pretty_json(&response).expect("format pretty json");
        assert!(json_str.contains('\n'), "pretty json must contain newlines");
        let value: serde_json::Value =
            serde_json::from_str(&json_str).expect("parse generated pretty json");

        assert_eq!(value["nodename"], "test-device");
        assert_eq!(value["serial_number"], "SN-12345");
        assert_eq!(value["product_config"], "workbench_eng");
        assert_eq!(value["board_config"], "x64");
        assert_eq!(value["boot_id"], 999888777);
        assert_eq!(value["boot_timestamp_nanos"], 1700000000000000000u64);
    }
}
