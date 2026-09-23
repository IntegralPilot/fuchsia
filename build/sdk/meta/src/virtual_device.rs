// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Representation of the virtual_device metadata.

mod manifest;
mod v1;

pub use manifest::*;
pub use v1::*;

use anyhow::{bail, Context, Result};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::fs::File;

const VIRTUAL_DEVICE_SCHEMA_V1: &str = "http://fuchsia.com/schemas/sdk/virtual_device.json";

/// Private helper for deserializing the virtual device.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
enum SerializationHelper {
    V1 { schema_id: String, data: VirtualDeviceV1 },
}

/// Versioned virtual device specification.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum VirtualDevice {
    V1(VirtualDeviceV1),
}

impl VirtualDevice {
    /// Load a VirtualDevice from a path on disk.
    pub fn try_load_from(path: impl AsRef<Utf8Path>) -> Result<Self> {
        if !path.as_ref().is_file() {
            bail!("Value '{}' doesn't appear to be a valid file.", path.as_ref());
        }
        let file = File::open(path.as_ref())
            .with_context(|| format!("opening virtual device: {:?}", path.as_ref()))?;
        let file = std::io::BufReader::new(file);
        let helper: SerializationHelper =
            serde_json::from_reader(file).context("parsing virtual device")?;
        let device = match helper {
            SerializationHelper::V1 { schema_id: _, data } => VirtualDevice::V1(data),
        };
        Ok(device)
    }

    /// Write a virtual device to disk at `path`.
    pub fn write(&self, path: impl AsRef<Utf8Path>) -> Result<()> {
        let helper = match self {
            Self::V1(data) => SerializationHelper::V1 {
                schema_id: VIRTUAL_DEVICE_SCHEMA_V1.into(),
                data: data.clone(),
            },
        };
        let file = File::create(path.as_ref()).context("creating virtual device file")?;
        serde_json::to_writer(file, &helper).context("writing virtual device file")?;
        Ok(())
    }

    /// Returns VirtualDevice entry name.
    pub fn name(&self) -> &str {
        match self {
            Self::V1(device) => &device.name.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_nonexistent_file() {
        assert!(VirtualDevice::try_load_from(Utf8Path::new("SomeNameThatsNotAFile")).is_err());
    }

    #[test]
    fn test_parse_empty_file() {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let path = temp_dir.path().join("other_file.json");
        File::create(&path).unwrap();
        assert!(VirtualDevice::try_load_from(Utf8Path::from_path(&path).unwrap()).is_err());
    }

    #[test]
    fn test_parse_non_json() {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let path = temp_dir.path().join("device.json");
        let mut file = File::create(&path).unwrap();
        file.write_all("this is not json".as_bytes()).unwrap();
        assert!(VirtualDevice::try_load_from(Utf8Path::from_path(&path).unwrap()).is_err());
    }

    #[test]
    fn test_parse_random_json() {
        let temp_dir = tempfile::TempDir::new().expect("creating temp dir");
        let path = temp_dir.path().join("device.json");
        let mut file = File::create(&path).unwrap();
        file.write_all("{\"foo\": 123}".as_bytes()).unwrap();
        assert!(VirtualDevice::try_load_from(Utf8Path::from_path(&path).unwrap()).is_err());
    }

    #[test]
    fn test_parse_v1() {
        let tmp = TempDir::new().unwrap();
        let vd_path = Utf8Path::from_path(tmp.path()).unwrap().join("virtual_device.json");
        let vd_file = File::create(&vd_path).unwrap();
        serde_json::to_writer(
            &vd_file,
            &json!({
                "schema_id": "http://fuchsia.com/schemas/sdk/virtual_device.json",
                "data": {
                    "name": "generic-x64",
                    "type": "virtual_device",
                    "hardware": {
                        "audio": {
                            "model": "hda"
                        },
                        "cpu": {
                            "arch": "x64"
                        },
                        "inputs": {
                            "pointing_device": "touch"
                        },
                        "window_size": {
                            "width": 640,
                            "height": 480,
                            "units": "pixels"
                        },
                        "memory": {
                            "quantity": 1,
                            "units": "gigabytes"
                        },
                        "storage": {
                            "quantity": 1,
                            "units": "gigabytes"
                        },
                        "vsock": {
                            "enabled": false,
                            "cid": 0
                        }
                    },
                    "start_up_args_template": "/path/to/args"
                }
            }),
        )
        .unwrap();
        let vd = VirtualDevice::try_load_from(vd_path).unwrap();
        assert!(matches!(vd, VirtualDevice::V1 { .. }));
        assert_eq!(vd.name(), "generic-x64");
    }
}
