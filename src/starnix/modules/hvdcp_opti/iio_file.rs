// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::utils::InvalidFile;
use fidl_fuchsia_hardware_qcom_hvdcpopti as fhvdcpopti;
use starnix_core::device::kobject::Device;
use starnix_core::fs::sysfs::build_device_directory;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps, serialize_for_file};
use starnix_core::vfs::pseudo::stub_empty_file::StubEmptyFile;
use starnix_logging::{bug_ref, log_error};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use std::borrow::Cow;
use std::sync::Arc;

pub fn build_iio0_directory(
    device: &Device,
    proxy: &Arc<fhvdcpopti::IioSynchronousProxy>,
    dir: &SimpleDirectoryMutator,
) {
    build_device_directory(device, dir);
    dir.entry("name", BytesFile::new_node(b"qpnp-smblite\n".to_vec()), mode!(IFREG, 0o444));
    dir.entry(
        "in_current_battery_input_current_limited_input",
        IioFile::new_node(proxy.clone(), "input_current_limited"),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "in_current_usb_input_current_settled_input",
        IioFile::new_node(proxy.clone(), "input_current_settled"),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "in_index_battery_die_health_input",
        IioFile::new_node(proxy.clone(), "die_health"),
        mode!(IFREG, 0o444),
    );
    dir.entry("in_index_usb_connector_health_input", InvalidFile::new_node(), mode!(IFREG, 0o444));
    dir.entry(
        "in_index_usb_real_type_input",
        IioFile::new_node(proxy.clone(), "real_charger_type"),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "in_index_usb_typec_mode_input",
        BytesFile::new_node(b"0\n".to_vec()),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "in_resistance_esr_actual_input",
        IioFile::new_node(proxy.clone(), "esr_actual"),
        mode!(IFREG, 0o444),
    );
}

pub fn build_iio1_directory(
    device: &Device,
    proxy: &Arc<fhvdcpopti::IioSynchronousProxy>,
    dir: &SimpleDirectoryMutator,
) {
    build_device_directory(device, dir);
    dir.entry(
        "name",
        BytesFile::new_node(b"1c40000.qcom,spmi:qcom,pm5100@0:qpnp,qbg@4f00\n".to_vec()),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "in_energy_charge_full_design_input",
        IioFile::new_node(proxy.clone(), "design_capacity"),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "in_energy_charge_full_input",
        IioFile::new_node(proxy.clone(), "learned_capacity"),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "in_index_debug_battery_input",
        BytesFile::new_node(b"0\n".to_vec()),
        mode!(IFREG, 0o444),
    );
    dir.entry("in_index_soh_input", IioFile::new_node(proxy.clone(), "soh"), mode!(IFREG, 0o444));
    dir.entry(
        "in_resistance_resistance_id_input",
        IioFile::new_node(proxy.clone(), "resistance_id"),
        mode!(IFREG, 0o444),
    );
}

pub fn build_usb_power_supply_directory(
    device: &Device,
    proxy: &Arc<fhvdcpopti::IioSynchronousProxy>,
    dir: &SimpleDirectoryMutator,
) {
    build_device_directory(device, dir);
    dir.entry("present", IioFile::new_node(proxy.clone(), "usb_present"), mode!(IFREG, 0o444));
    dir.entry("voltage_max", IioFile::new_node(proxy.clone(), "voltage_max"), mode!(IFREG, 0o444));
    dir.entry("voltage_now", IioFile::new_node(proxy.clone(), "voltage_now"), mode!(IFREG, 0o444));
    dir.entry(
        "capacity",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "capacity_level",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "status",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
        mode!(IFREG, 0o444),
    );
}

pub fn build_battery_power_supply_directory(
    device: &Device,
    proxy: &Arc<fhvdcpopti::IioSynchronousProxy>,
    dir: &SimpleDirectoryMutator,
) {
    build_device_directory(device, dir);
    dir.entry(
        "capacity",
        IioFile::new_node(proxy.clone(), "battery_capacity"),
        mode!(IFREG, 0o444),
    );
    dir.entry(
        "charge_type",
        IioFile::new_node(proxy.clone(), "battery_charge_type"),
        mode!(IFREG, 0o444),
    );
    dir.entry("health", IioFile::new_node(proxy.clone(), "battery_health"), mode!(IFREG, 0o444));
    dir.entry("status", IioFile::new_node(proxy.clone(), "battery_status"), mode!(IFREG, 0o444));
    dir.entry(
        "capacity_level",
        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
        mode!(IFREG, 0o444),
    );
}

struct IioFile {
    proxy: Arc<fhvdcpopti::IioSynchronousProxy>,
    label: &'static str,
}

impl IioFile {
    fn new_node(
        proxy: Arc<fhvdcpopti::IioSynchronousProxy>,
        label: &'static str,
    ) -> impl FsNodeOps {
        BytesFile::new_node(IioFile { proxy, label })
    }
}

impl BytesFileOps for IioFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let value = self
            .proxy
            .get_iio_value(self.label, zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log_error!("Failed to GetIioValue: {:?}", e);
                errno!(EINVAL)
            })?
            .map_err(|e| {
                log_error!("GetIioValue failed: {:?}", e);
                errno!(EINVAL)
            })?;

        Ok(serialize_for_file(value).into())
    }
}
