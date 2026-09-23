// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

mod cooling;
mod family;
mod thermal_zone;

use crate::thermal_zone::{SensorProps, ThermalZone};
use anyhow::{Error, anyhow};
use family::ThermalFamily;
use fidl_fuchsia_hardware_temperature as ftemperature;
use fidl_fuchsia_thermal as fthermal;
use starnix_core::device::kobject::Device;
use starnix_core::fs::sysfs::build_device_directory;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use starnix_logging::{log_error, log_warn};

use starnix_uapi::errors::{Errno, errno, error};
use starnix_uapi::file_mode::mode;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use thermal_netlink::{celsius_to_millicelsius, millicelsius_to_celsius};
use zx::MonotonicInstant;

pub use cooling::cooling_device_init;

fn build_thermal_zone_directory(
    device: &Device,
    proxy: ftemperature::DeviceSynchronousProxy,
    sensor_manager: fthermal::SensorManagerSynchronousProxy,
    device_type: String,
    dir: &SimpleDirectoryMutator,
) {
    build_device_directory(device, dir);
    dir.entry(
        "emul_temp",
        EmulTempFile::new_node(device_type.clone(), sensor_manager),
        mode!(IFREG, 0o200),
    );
    dir.entry("temp", TemperatureFile::new_node(proxy), mode!(IFREG, 0o664));
    dir.entry(
        "type",
        BytesFile::new_node(format!("{}\n", device_type).into_bytes()),
        mode!(IFREG, 0o444),
    );
    dir.entry("policy", BytesFile::new_node(b"step_wise\n".to_vec()), mode!(IFREG, 0o444));
    dir.entry(
        "available_policies",
        BytesFile::new_node(b"step_wise\n".to_vec()),
        mode!(IFREG, 0o444),
    );
}

struct TemperatureFile {
    proxy: ftemperature::DeviceSynchronousProxy,
}

impl TemperatureFile {
    pub fn new_node(proxy: ftemperature::DeviceSynchronousProxy) -> impl FsNodeOps {
        BytesFile::new_node(Self { proxy })
    }
}

impl BytesFileOps for TemperatureFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let (zx_status, temp) =
            self.proxy.get_temperature_celsius(MonotonicInstant::INFINITE).map_err(|e| {
                log_error!("get_temperature_celsius failed: {}", e);
                errno!(ENOENT)
            })?;
        let _ = zx::Status::ok(zx_status).map_err(|e| {
            log_error!("get_temperature_celsius driver returned error: {}", e);
            errno!(ENOENT)
        })?;

        let out = format!("{}\n", celsius_to_millicelsius(temp) as i32);
        Ok(out.as_bytes().to_owned().into())
    }
}

struct EmulTempFile {
    device_type: String,
    proxy: fthermal::SensorManagerSynchronousProxy,
}

impl EmulTempFile {
    pub fn new_node(
        device_type: String,
        proxy: fthermal::SensorManagerSynchronousProxy,
    ) -> impl FsNodeOps {
        BytesFile::new_node(Self { device_type, proxy })
    }
}

impl BytesFileOps for EmulTempFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let num_str = str::from_utf8(&data).map_err(|e| {
            log_warn!("Failed to convert input temp to utf-8: {:?}", e);
            errno!(EINVAL)
        })?;

        let temp_milli_c: i32 = num_str.trim().parse().map_err(|e| {
            log_warn!("Failed to parse input temp as i32: {:?}", e);
            errno!(EINVAL)
        })?;

        if temp_milli_c == 0 {
            match self
                .proxy
                .clear_temperature_override(&self.device_type, zx::MonotonicInstant::INFINITE)
            {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(error)) => {
                    log_warn!(
                        "Failed to clear temperature override for sensor {}: {:?}",
                        self.device_type,
                        error
                    );
                    error!(EINVAL)
                }
                Err(error) => {
                    log_warn!(
                        "Failed to call clear_temperature_override for sensor {}: {:?}",
                        self.device_type,
                        error
                    );
                    error!(EIO)
                }
            }
        } else {
            match self.proxy.set_temperature_override(
                &self.device_type,
                millicelsius_to_celsius(temp_milli_c as f32).into(),
                zx::MonotonicInstant::INFINITE,
            ) {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(error)) => {
                    log_warn!(
                        "Failed to set temperature override for sensor {}: {:?}",
                        self.device_type,
                        error
                    );
                    error!(EINVAL)
                }
                Err(error) => {
                    log_warn!(
                        "Failed to call set_temperature_override for sensor {}: {:?}",
                        self.device_type,
                        error
                    );
                    error!(EIO)
                }
            }
        }
    }
}

pub fn thermal_device_init(kernel: &Kernel) -> Result<(), Error> {
    let sensor_manager =
        fuchsia_component::client::connect_to_protocol_sync::<fthermal::SensorManagerMarker>()
            .map_err(|error| anyhow!("Failed to connect to SensorManager: {:?}", error))?;

    let sensors = sensor_manager.list_sensors(zx::MonotonicInstant::INFINITE)?;

    let registry = &kernel.device_registry;
    let virtual_thermal_class = registry.objects.virtual_thermal_class();
    let mut sensor_proxies = HashMap::new();

    for (thermal_zone_id, sensor_info) in sensors.into_iter().enumerate() {
        let Some(sensor_name) = sensor_info.name else {
            log_warn!("No sensor name for thermal zone {}, skipping.", thermal_zone_id);
            continue;
        };
        let sensor_name_clone = sensor_name.clone();

        let thermal_zone_id = thermal_zone_id as u32;
        let thermal_zone = format!("thermal_zone{}", thermal_zone_id);

        // Create a synchronous proxy for file requests.
        // Reads and writes are expected to block until data or an error is returned.
        let (sensor_sync, sensor_server_sync) = fidl::endpoints::create_sync_proxy();

        if let Err(error) = sensor_manager.connect(
            fthermal::SensorManagerConnectRequest {
                name: Some(sensor_name.clone()),
                server_end: Some(fthermal::SensorServer_::Temperature(sensor_server_sync)),
                ..Default::default()
            },
            zx::MonotonicInstant::INFINITE,
        ) {
            log_error!("Failed to connect to sensor {} (sync): {:?}", sensor_name, error);
            continue;
        }

        registry.add_numberless_device(thermal_zone.as_str().into(),
            virtual_thermal_class.clone(),
            move |device, dir|{
                match fuchsia_component::client::connect_to_protocol_sync::<fthermal::SensorManagerMarker>() {
                    Ok(sensor_manager) => build_thermal_zone_directory(device, sensor_sync, sensor_manager, sensor_name_clone, dir),
                    Err(error) => log_warn!("Failed to connect to SensorManager when building thermal zone for sensor {}: {:?}", sensor_name_clone, error),
                }
            },
        );

        // Create an asynchronous proxy for netlink calls.
        // The thermal netlink server periodically gets temperature data from the thermal sensors in
        // the background.
        let (sensor, sensor_server) = fidl::endpoints::create_proxy();

        if let Err(error) = sensor_manager.connect(
            fthermal::SensorManagerConnectRequest {
                name: Some(sensor_name.clone()),
                server_end: Some(fthermal::SensorServer_::Temperature(sensor_server)),
                ..Default::default()
            },
            zx::MonotonicInstant::INFINITE,
        ) {
            log_error!("Failed to connect to sensor {} (async): {:?}", sensor_name, error);
            continue;
        }

        sensor_proxies.insert(
            SensorProps { name: sensor_name },
            ThermalZone { id: thermal_zone_id, proxy: sensor },
        );
    }

    let (thermal_family, thermal_family_worker) = ThermalFamily::new(sensor_proxies);
    kernel.generic_netlink().add_family(Arc::new(thermal_family));
    kernel
        .kthreads
        .spawn_future(move || async move { thermal_family_worker.await }, "thermal_netlink_worker");

    Ok(())
}
