// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow};
use fidl_fuchsia_audio_device as fadevice;
use fuchsia_audio::Registry;
use fuchsia_audio::device::Type;
use fuchsia_component::client::connect_to_protocol;
use serde::Deserialize;
use std::str::FromStr;

#[derive(Deserialize, Debug)]
struct Config {
    expected_devices: Vec<ExpectedDevice>,
}

#[derive(Deserialize, Debug)]
struct ExpectedDevice {
    device_type: String,
    name: Option<String>,
    manufacturer: Option<String>,
    product: Option<String>,
}

const TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(45);
const POLL_INTERVAL: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(500);

#[fuchsia::test]
// Test Coverage: Audio Device Registry & Drivers (Enumeration)
async fn test_audio_device_enumeration() -> Result<(), Error> {
    // Read config
    let config_path = "/pkg/data/config.json";
    let config_str = std::fs::read_to_string(config_path)
        .context(format!("Failed to read config from {}", config_path))?;
    let config: Config =
        serde_json::from_str(&config_str).context("Failed to parse config.json")?;

    log::info!("Looking for {:?}", config.expected_devices);

    // Try Registry
    let registry_proxy = connect_to_protocol::<fadevice::RegistryMarker>()
        .context("Failed to connect to fuchsia.audio.device.Registry")?;
    let registry = Registry::new(registry_proxy);
    log::info!("Connected to Audio Device Registry");

    let deadline = zx::MonotonicInstant::after(TIMEOUT);
    let mut missing_devices = Vec::new();

    // TODO(https://fxbug.dev/479279117): Extend this to support checking topologies, element types
    // for the devices.
    loop {
        // Get all devices
        let infos = registry.device_infos().await;
        let found_devices: Vec<_> = infos.values().cloned().collect();

        missing_devices.clear();
        let mut matched_device_indices = std::collections::HashSet::new();
        let total_expected = config.expected_devices.len();
        for (i, expected) in config.expected_devices.iter().enumerate() {
            let expected_device_type = Type::from_str(&expected.device_type).map_err(|e| {
                anyhow!(
                    "Test config contains invalid device type '{}': {}",
                    expected.device_type,
                    e
                )
            })?;

            let mut found = false;
            for (dev_idx, info) in found_devices.iter().enumerate() {
                let type_match = info.device_type() == expected_device_type;
                if !type_match {
                    log::debug!(
                        "    Device '{}': type {} does not match expected {}",
                        info.device_name(),
                        info.device_type(),
                        expected_device_type
                    );
                }

                let name_match = match &expected.name {
                    Some(n) => info.device_name() == n,
                    None => true,
                };
                if !name_match {
                    log::debug!(
                        "    Device '{}': Name does not match expected {:?}",
                        info.device_name(),
                        expected.name,
                    );
                }

                let manufacturer_match = match &expected.manufacturer {
                    Some(m) => info.0.manufacturer.as_ref().map(|rm| rm == m).unwrap_or(false),
                    None => true,
                };
                if !manufacturer_match {
                    log::debug!(
                        "    Device '{}': Manufacturer {:?} does not match expected {:?}",
                        info.device_name(),
                        info.0.manufacturer,
                        expected.manufacturer,
                    );
                }

                let product_match = match &expected.product {
                    Some(p) => info.0.product.as_ref().map(|rp| rp == p).unwrap_or(false),
                    None => true,
                };
                if !product_match {
                    log::debug!(
                        "    Device '{}': Product {:?} does not match expected {:?}",
                        info.device_name(),
                        info.0.product,
                        expected.product,
                    );
                }

                if type_match && name_match && manufacturer_match && product_match {
                    found = true;
                    matched_device_indices.insert(dev_idx);
                    log::debug!(
                        "Successfully detected an expected device ({}/{}).",
                        i + 1,
                        total_expected
                    );
                    log::debug!("  Detected: {:?}", info);
                    log::debug!("  Expected: {:?}", expected);
                    break;
                }
            }

            if !found {
                missing_devices.push(expected);
            }
        }

        let unexpected_devices: Vec<_> = found_devices
            .iter()
            .enumerate()
            .filter(|(idx, _)| !matched_device_indices.contains(idx))
            .map(|(_, dev)| dev)
            .collect();

        if missing_devices.is_empty() {
            log::info!("Found devices:");
            for dev in &found_devices {
                log::info!(
                    "  Type: {:?}, Name: {:?}, Manufacturer: {:?}, Product: {:?}",
                    dev.device_type(),
                    dev.device_name(),
                    dev.0.manufacturer,
                    dev.0.product
                );
            }
            if !unexpected_devices.is_empty() {
                log::info!("Unexpected devices:");
                for dev in &unexpected_devices {
                    log::info!(
                        "  Type: {:?}, Name: {:?}, Manufacturer: {:?}, Product: {:?}",
                        dev.device_type(),
                        dev.device_name(),
                        dev.0.manufacturer,
                        dev.0.product
                    );
                }
            }
            log::info!("All expected devices found!");
            return Ok(());
        }

        if zx::MonotonicInstant::get() >= deadline {
            log::info!("Found devices at deadline:");
            for dev in &found_devices {
                log::info!(
                    "  Type: {:?}, Name: {:?}, Manufacturer: {:?}, Product: {:?}",
                    dev.device_type(),
                    dev.device_name(),
                    dev.0.manufacturer,
                    dev.0.product
                );
            }
            if !unexpected_devices.is_empty() {
                log::info!("Unexpected devices at deadline:");
                for dev in &unexpected_devices {
                    log::info!(
                        "  Type: {:?}, Name: {:?}, Manufacturer: {:?}, Product: {:?}",
                        dev.device_type(),
                        dev.device_name(),
                        dev.0.manufacturer,
                        dev.0.product
                    );
                }
            }
            for missing in &missing_devices {
                log::error!("Failed to find expected device: {:?}", missing);
            }
            panic!("Timed out waiting for the following expected devices: {:?}", missing_devices);
        }

        fuchsia_async::Timer::new(zx::MonotonicInstant::after(POLL_INTERVAL)).await;
    }
}
