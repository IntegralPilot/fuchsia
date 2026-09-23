// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use fidl_fuchsia_device as fdevice;
use log::{error, info};
use serde_derive::Deserialize;

#[derive(Deserialize)]
pub struct DriverAlias {
    /// Sensor name returned by GetSensorName of the driver.
    pub sensor_name: String,
    /// Alias for data logging and processing.
    pub alias: String,
}

/// Helper struct to deserialize the optional config file loaded from CONFIG_PATH
/// (//src/power/metrics-logger/src/main.rs). The config file maps the sensor name of the driver
/// to the alias used in data logging and processing.
#[derive(Deserialize)]
pub struct Config {
    pub temperature_drivers: Option<Vec<DriverAlias>>,
    pub power_drivers: Option<Vec<DriverAlias>>,
    pub gpu_drivers: Option<Vec<DriverAlias>>,
}

pub fn connect_proxy<T: fidl::endpoints::ProtocolMarker>(path: &str) -> Result<T::Proxy> {
    let (proxy, server) = fidl::endpoints::create_proxy::<T>();

    fdio::service_connect(path, server.into_channel())
        .map_err(|s| format_err!("Failed to connect to service at {}: {}", path, s))?;
    Ok(proxy)
}

pub async fn get_driver_topological_path(path: &str) -> Result<String> {
    let controller_path = path.to_owned() + "/device_controller";
    let proxy = connect_proxy::<fdevice::ControllerMarker>(&controller_path)?;
    proxy
        .get_topological_path()
        .await?
        .map_err(|raw| format_err!("zx error: {}", zx::Status::err_from_raw(raw)))
}

pub async fn list_directory_entries(path: &str) -> Vec<String> {
    let dir = match fuchsia_fs::directory::open_in_namespace(path, fuchsia_fs::PERM_READABLE) {
        Ok(s) => s,
        Err(err) => {
            info!(path:%, err:%; "Service directory doesn't exist or NodeProxy failed with error");
            return Vec::new();
        }
    };
    match fuchsia_fs::directory::readdir(&dir).await {
        Ok(s) => s.iter().map(|dir_entry| dir_entry.name.clone()).collect(),
        Err(err) => {
            error!(path:%, err:%; "Read service directory failed with error");
            Vec::new()
        }
    }
}

// Representation of an actively-used driver.
pub struct Driver<T> {
    pub alias: Option<String>,
    pub sensor_name: String,
    pub proxy: T,
}

impl<T> Driver<T> {
    pub fn name(&self) -> &str {
        &self.alias.as_ref().unwrap_or(&self.sensor_name)
    }
}
