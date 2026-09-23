// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow, format_err};
use fidl::endpoints::SynchronousProxy;
use fidl_fuchsia_power_battery as fbattery;
use fidl_fuchsia_power_cpu as fcpu;
use starnix_core::device::kobject::Device;
use starnix_core::fs::sysfs::build_device_directory;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use starnix_logging::{log_error, log_warn};
use starnix_sync::{LockDepMutex, ThermalChargeLevelLock};
use starnix_uapi::errors::{Errno, errno};
use starnix_uapi::file_mode::mode;
use std::borrow::Cow;
use std::sync::Arc;
use zx::MonotonicInstant;

const BATTERY_CHARGER_SERVICE_DIRECTORY: &str = "/svc/fuchsia.power.battery.ChargerService";

trait CoolingOps: Send + Sync + 'static {
    fn get_max_state(&self) -> u32;
    fn get_state(&self) -> Result<u32, Errno>;
    fn set_state(&self, state: u32) -> Result<(), Errno>;
}

impl<T: CoolingOps> CoolingOps for Arc<T> {
    fn get_max_state(&self) -> u32 {
        self.as_ref().get_max_state()
    }

    fn get_state(&self) -> Result<u32, Errno> {
        self.as_ref().get_state()
    }

    fn set_state(&self, state: u32) -> Result<(), Errno> {
        self.as_ref().set_state(state)
    }
}

struct CoolingDevice<T: CoolingOps> {
    device_id: u32,
    device_type: String,
    ops: T,
}

impl<T: CoolingOps> CoolingDevice<T> {
    fn get_device_name(&self) -> String {
        format!("cooling_device{}", self.device_id)
    }

    fn build_device_dir(self: Arc<Self>, device: &Device, dir: &SimpleDirectoryMutator) {
        build_device_directory(device, dir);
        dir.entry(
            "max_state",
            BytesFile::new_node(format!("{}\n", self.ops.get_max_state()).into_bytes()),
            mode!(IFREG, 0o444),
        );
        dir.entry(
            "type",
            BytesFile::new_node(format!("{}\n", self.device_type).into_bytes()),
            mode!(IFREG, 0o444),
        );
        dir.entry("cur_state", CurStateFile::new_node(self), mode!(IFREG, 0o644));
    }
}

/// Current state file, which proxies integral reads and writes to [`CoolingOps`].
struct CurStateFile<T: CoolingOps> {
    cooling_device: Arc<CoolingDevice<T>>,
}

impl<T: CoolingOps> CurStateFile<T> {
    fn new_node(cooling_device: Arc<CoolingDevice<T>>) -> impl FsNodeOps {
        BytesFile::new_node(Self { cooling_device })
    }
}

impl<T: CoolingOps> BytesFileOps for CurStateFile<T> {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let state = self.cooling_device.ops.get_state()?;
        Ok(format!("{}\n", state).into_bytes().into())
    }

    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let input = str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;
        let input_num = input.trim().parse().map_err(|_| errno!(EINVAL))?;
        self.cooling_device.ops.set_state(input_num)
    }
}

/// Registrar for cooling devices, which are sequentially numbered.
struct CoolingDeviceRegistrar {
    next_id: u32,
}

impl CoolingDeviceRegistrar {
    fn new() -> Self {
        Self { next_id: 0 }
    }

    fn get_next_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Register a device in the virtual thermal class.
    fn register<T: CoolingOps>(&mut self, kernel: &Kernel, device_type: String, ops: T) -> Device {
        let device_registry = &kernel.device_registry;
        let device_class = device_registry.objects.virtual_thermal_class();

        let cooling_device =
            Arc::new(CoolingDevice::<T> { device_id: self.get_next_id(), device_type, ops });

        device_registry.add_numberless_device(
            cooling_device.get_device_name().as_str().into(),
            device_class,
            |device, dir| cooling_device.build_device_dir(device, dir),
        )
    }
}

struct CpuCoolingOps {
    domain_controller: Arc<fcpu::DomainControllerSynchronousProxy>,
    domain_id: u64,
    available_frequencies_hz: Vec<u64>,
}

impl CoolingOps for CpuCoolingOps {
    fn get_max_state(&self) -> u32 {
        (self.available_frequencies_hz.len() - 1) as u32
    }
    fn get_state(&self) -> Result<u32, Errno> {
        let max_frequency_index = self
            .domain_controller
            .get_max_frequency(self.domain_id, MonotonicInstant::INFINITE)
            .map_err(|e| errno!(EIO, anyhow!("Failed to send get_max_frequency call: {:?}", e)))?
            .map_err(|e| errno!(EIO, anyhow!("Failed response from get_max_frequency: {:?}", e)))?;
        Ok(max_frequency_index as u32)
    }
    fn set_state(&self, state: u32) -> Result<(), Errno> {
        if state == 0 {
            self.domain_controller
                .clear_max_frequency(self.domain_id, MonotonicInstant::INFINITE)
                .map_err(|e| {
                    errno!(EIO, anyhow!("Failed to send clear_max_frequency call: {:?}", e))
                })?
                .map_err(|e| {
                    errno!(EIO, anyhow!("Failed response from clear_max_frequency: {:?}", e))
                })
        } else {
            self.domain_controller
                .set_max_frequency(self.domain_id, state.into(), MonotonicInstant::INFINITE)
                .map_err(|e| {
                    errno!(EIO, anyhow!("Failed to send set_max_frequency call: {:?}", e))
                })?
                .map_err(|e| {
                    errno!(EIO, anyhow!("Failed response from set_max_frequency: {:?}", e))
                })
        }
    }
}

fn register_cpu_domains(
    kernel: &Kernel,
    registrar: &mut CoolingDeviceRegistrar,
) -> Result<(), Error> {
    let domain_controller = Arc::new(
        fuchsia_component::client::connect_to_protocol_sync::<fcpu::DomainControllerMarker>()
            .map_err(|error| anyhow!("Failed to connect to DomainController: {:?}", error))?,
    );
    let domains = domain_controller
        .list_domains(MonotonicInstant::INFINITE)
        .map_err(|e| anyhow!("list_domains failed: {}", e))?;

    // Each domain is tunable, so expose each as a separate cooling device.
    for domain in domains {
        let device_type = domain.name.expect("name not provided");
        let ops = CpuCoolingOps {
            domain_controller: domain_controller.clone(),
            domain_id: domain.id.expect("id not provided"),
            available_frequencies_hz: domain
                .available_frequencies_hz
                .expect("available_frequencies_hz not provided"),
        };
        registrar.register(kernel, device_type, ops);
    }
    Ok(())
}

/// Initializes the cooling devices specified in the device list.
///
/// Device strings are of the form `type[=param]`. Not all device types support a parameter.
///
/// Supported devices:
/// * `fcc=N`: Fast charge current, where N is the maximum charge level.
pub fn cooling_device_init(kernel: &Kernel, devices: Vec<String>) -> Result<(), Error> {
    let mut registrar = CoolingDeviceRegistrar::new();
    for device_spec in devices.into_iter() {
        let (device_type, device_param) = device_spec
            .split_once('=')
            .map_or_else(|| (device_spec.as_str(), None), |(t, p)| (t, Some(p)));
        match device_type {
            "fcc" => {
                // TODO(b/460321934): Return errors rather than logging them.
                if let Err(e) = register_fcc_device(
                    kernel,
                    &mut registrar,
                    device_param
                        .ok_or_else(|| format_err!("Missing parameter for 'fcc' cooling device"))?,
                ) {
                    log_error!("Failed to register 'fcc' cooling device: {e:?}");
                }
            }
            "cpu" => {
                // TODO(b/460321934): Return errors rather than logging them.
                if let Err(e) = register_cpu_domains(kernel, &mut registrar) {
                    log_error!("Failed to register 'cpu' cooling device: {e:?}");
                }
            }
            t => {
                return Err(format_err!("Unknown cooling device: {t:?}"));
            }
        };
    }

    Ok(())
}

struct FccCoolingOps {
    proxy: fbattery::ChargerSynchronousProxy,
    max_charge_level: u32,
    charge_level: LockDepMutex<u32, ThermalChargeLevelLock>,
}

impl FccCoolingOps {
    fn new(
        proxy: fbattery::ChargerSynchronousProxy,
        max_charge_level: u32,
        charge_level: u32,
    ) -> Self {
        Self { proxy, max_charge_level, charge_level: charge_level.into() }
    }
}

impl CoolingOps for FccCoolingOps {
    fn get_max_state(&self) -> u32 {
        self.max_charge_level
    }

    fn get_state(&self) -> Result<u32, Errno> {
        let locked_charge_level = self.charge_level.lock();
        Ok(*locked_charge_level)
    }

    fn set_state(&self, state: u32) -> Result<(), Errno> {
        let mut locked_charge_level = self.charge_level.lock();

        // Attempting to set a charge level greater than the maximum results in 0 being set.
        // This is based on observations of how this node behaves on Linux.
        // See b/446016549#comment4 for details.
        let charge_level = if state > self.max_charge_level {
            log_warn!(
                "FCC charge_level of {} exceeds {}; setting to 0",
                state,
                self.max_charge_level
            );
            0
        } else {
            state
        };

        // When the charge level goes to the maximum, disable charging. Otherwise, when dropping
        // below the maximum, enable charging.
        if charge_level == self.max_charge_level {
            self.proxy
                .enable(false, zx::MonotonicInstant::INFINITE)
                .map_err(|e| errno!(EIO, e))?
                .map_err(|e| errno!(EIO, e))?;
        } else if *locked_charge_level == self.max_charge_level {
            self.proxy
                .enable(true, zx::MonotonicInstant::INFINITE)
                .map_err(|e| errno!(EIO, e))?
                .map_err(|e| errno!(EIO, e))?;
        }

        *locked_charge_level = charge_level;
        Ok(())
    }
}

fn register_fcc_device(
    kernel: &Kernel,
    registrar: &mut CoolingDeviceRegistrar,
    param: &str,
) -> Result<(), Error> {
    let proxy = connect_to_battery_charger().context("Failed to connect to battery Charger")?;
    let max_charge_level: u32 = param.parse().context("Invalid max_charge_level")?;
    let ops = FccCoolingOps::new(proxy, max_charge_level, 0);

    registrar.register(kernel, "fcc".to_string(), ops);
    Ok(())
}

fn connect_to_battery_charger() -> Result<fbattery::ChargerSynchronousProxy, Error> {
    // Attempt to manually locate the charger service instance. The instance name is not static, so
    // we connect to the first one routed into the namespace.
    // TODO(b/460242910): Simplify this process.
    let mut dir = std::fs::read_dir(BATTERY_CHARGER_SERVICE_DIRECTORY)
        .context("Failed to read ChargerService directory")?;
    let entry = dir
        .next()
        .ok_or_else(|| anyhow::format_err!("Missing ChargerService instance"))?
        .context("Unable to read ChargerService instance")?;
    let path = entry
        .path()
        .join("device")
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow::format_err!("Failed to get device path"))?;

    let (client_end, server_end) = zx::Channel::create();
    fdio::service_connect(&path, server_end)?;
    Ok(fbattery::ChargerSynchronousProxy::from_channel(client_end))
}
