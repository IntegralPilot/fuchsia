// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Construct and parse a product description entry.

use serde::{Deserialize, Serialize};

/// A versioned manifest describing what to upload or download.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "schema_version")]
pub enum ProductDescription {
    /// The version of file schema.
    #[serde(rename = "1")]
    V1(ProductDescriptionV1),
}

/// Version 1 of the product description.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct ProductDescriptionV1 {
    /// Application binary interface (ABI) used in this product. An eight digit hex
    /// string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abi: Option<String>,

    /// Board name and architecture. E.g. "x64".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub board_name: Option<String>,

    /// The OS version as a platform this product runs on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_version: Option<String>,

    /// A human readable name for this product. E.g. "workstation_eng".
    pub product_name: String,

    /// The version of this product (independent of the sdk version).
    pub product_version: String,

    /// The version of the SDK used to create this product.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdk_version: Option<String>,

    /// Where to find the transfer manifest json file to download the product
    /// bundle.
    pub transfer_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialization() {
        let expected = ProductDescription::V1(ProductDescriptionV1 {
            product_name: "foo.bar".to_string(),
            product_version: "blah".to_string(),
            transfer_url: "gs://fake-example/test.json".to_string(),
            abi: Some("fake_abi".to_string()),
            ..Default::default()
        });
        let value = serde_json::json!({
            "schema_version": "1",
            "abi": "fake_abi".to_string(),
            "product_name": "foo.bar".to_string(),
            "product_version": "blah".to_string(),
            "transfer_url": "gs://fake-example/test.json".to_string(),
        });
        let manifest: ProductDescription = serde_json::from_value(value).unwrap();
        assert_eq!(expected, manifest);
    }
}
