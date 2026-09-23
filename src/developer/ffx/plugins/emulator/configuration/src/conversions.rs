// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module contains code for converting between the sdk_metadata types and the engine
//! interface types. We perform the conversion here to keep dependencies on the sdk_metadata
//! to a minimum, while improving our ability to fully test the conversion code.

use anyhow::{Result, anyhow};
use assembled_system::Image;
use emulator_instance::{
    DeviceConfig, DiskImage, EmulatorConfiguration, GuestConfig, PortMapping, Ramdisk, RamdiskKind,
    VirtualCpu,
};

use product_bundle::{ProductBundle, ProductBundleV2};
use sdk_metadata::VirtualDeviceV1;
use std::path::PathBuf;

pub async fn convert_bundle_to_configs(
    product_bundle: &ProductBundle,
    device_name: Option<String>,
    uefi: bool,
) -> Result<EmulatorConfiguration> {
    let virtual_device = product_bundle.get_device(&device_name)?;
    log::debug!("Found PBM: {:#?}\nVirtual Device: {:#?}", product_bundle, virtual_device);
    match &product_bundle {
        ProductBundle::V2(product_bundle) => {
            convert_v2_bundle_to_configs(&product_bundle, &virtual_device, uefi)
        }
    }
}

fn convert_v2_bundle_to_configs(
    product_bundle: &ProductBundleV2,
    virtual_device: &VirtualDeviceV1,
    uefi: bool,
) -> Result<EmulatorConfiguration> {
    let mut emulator_configuration: EmulatorConfiguration = EmulatorConfiguration::default();

    // Map the product and device specifications to the Device, and Guest configs.
    emulator_configuration.device = DeviceConfig {
        audio: virtual_device.hardware.audio.clone(),
        cpu: VirtualCpu {
            architecture: virtual_device.hardware.cpu.arch.clone(),
            count: virtual_device.hardware.cpu.count,
        },
        memory: virtual_device.hardware.memory.clone(),
        pointing_device: virtual_device.hardware.inputs.pointing_device.clone(),
        screen: virtual_device.hardware.window_size.clone(),
        storage: virtual_device.hardware.storage.clone(),
        vsock: Some(virtual_device.hardware.vsock.clone()),
        avx2_enabled: virtual_device.avx2_enabled,
    };

    emulator_configuration.runtime.template = None;

    if let Some(ports) = &virtual_device.ports {
        for (name, port) in ports {
            emulator_configuration
                .host
                .port_map
                .insert(name.to_owned(), PortMapping { guest: port.to_owned(), host: None });
        }
    }

    // Check if a single `efi-shell` bootloader partition has been provided, if so there is no
    // other payload.
    let bootloader_partitions = &product_bundle.partitions.bootloader_partitions;
    if bootloader_partitions.len() == 1
        && bootloader_partitions[0].partition_type.as_str() == "efi-shell"
    {
        let disk_image = Some(DiskImage::Fat(bootloader_partitions[0].image.clone().into()));
        emulator_configuration.guest = GuestConfig { disk_image, ..Default::default() };
        return Ok(emulator_configuration);
    }

    // Try to find images in system_a.
    let system = product_bundle
        .system_a
        .as_ref()
        .ok_or_else(|| anyhow!("No systems to boot in the product bundle"))?;

    let kernel_image: Option<PathBuf> = system.iter().find_map(|i| match i {
        Image::QemuKernel(path) => Some(path.clone().into()),
        _ => None,
    });

    let disk_image: Option<DiskImage> = system.iter().find_map(|i| match (uefi, i) {
        (false, Image::FVM(path)) => Some(DiskImage::Fvm(path.clone().into())),
        (false, Image::FxfsSparse { path, .. }) => Some(DiskImage::Fxfs(path.clone().into())),
        (true, Image::FxfsSparse { path, .. }) => Some(DiskImage::Gpt(path.clone().into())),
        _ => None,
    });

    // Some kernels do not have separate zbi images.
    let ramdisk: Option<Ramdisk> = system.iter().find_map(|i| {
        let (path, kind) = match i {
            Image::ZBI { path, .. } => (path, RamdiskKind::Zbi),
            Image::TestRamdisk(path) => (path, RamdiskKind::Test),
            _ => return None,
        };
        Some(Ramdisk { path: path.clone().into(), kind })
    });

    emulator_configuration.guest =
        GuestConfig { disk_image, kernel_image, ramdisk, ..Default::default() };

    Ok(emulator_configuration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembled_system::BlobfsContents;
    use assembly_partitions_config::{BootloaderPartition, PartitionsConfig};
    use camino::Utf8PathBuf;
    use sdk_metadata::virtual_device::{Cpu, Hardware};
    use sdk_metadata::{
        AudioDevice, AudioModel, CpuArchitecture, DataAmount, DataUnits, ElementType, InputDevice,
        PointingDevice, Screen, ScreenUnits, VsockDevice,
    };
    use std::collections::HashMap;

    #[test]
    fn test_convert_v2_bundle_to_configs() {
        let temp_dir = tempfile::TempDir::new().expect("creating sdk_root temp dir");
        let sdk_root = temp_dir.path();
        let template_path = sdk_root.join("fake_template");
        std::fs::write(&template_path, b"").expect("create fake template file");

        // Set up some test data to pass into the conversion routine.
        let expected_kernel = Utf8PathBuf::from_path_buf(sdk_root.join("kernel"))
            .expect("couldn't convert kernel to utf8");
        let expected_disk_image_path =
            Utf8PathBuf::from_path_buf(sdk_root.join("fvm")).expect("couldn't convert fvm to utf8");
        let expected_zbi =
            Utf8PathBuf::from_path_buf(sdk_root.join("zbi")).expect("couldn't convert zbi to utf8");

        let mut pb = ProductBundleV2 {
            product_name: String::default(),
            product_version: String::default(),
            partitions: PartitionsConfig::default(),
            sdk_version: String::default(),
            system_a: Some(vec![
                // By the time we call convert_, these should be canonicalized.
                Image::ZBI { path: expected_zbi.clone(), signed: false },
                Image::QemuKernel(expected_kernel.clone()),
                Image::FVM(expected_disk_image_path.clone()),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        };
        let mut device = VirtualDeviceV1 {
            name: "FakeDevice".to_string(),
            description: Some("A fake virtual device".to_string()),
            kind: ElementType::VirtualDevice,
            hardware: Hardware {
                cpu: Cpu { arch: CpuArchitecture::X64, count: 4 },
                audio: AudioDevice { model: AudioModel::Hda },
                storage: DataAmount { quantity: 512, units: DataUnits::Megabytes },
                inputs: InputDevice { pointing_device: PointingDevice::Mouse },
                memory: DataAmount { quantity: 4, units: DataUnits::Gigabytes },
                window_size: Screen { height: 480, width: 640, units: ScreenUnits::Pixels },
                vsock: VsockDevice { enabled: false, cid: 0 },
            },
            avx2_enabled: true,
            ports: None,
        };

        // Run the conversion, then assert everything in the config matches the manifest data.
        let config = convert_v2_bundle_to_configs(&pb, &device, false)
            .expect("convert_v2_bundle_to_configs");
        assert_eq!(config.device.audio, device.hardware.audio);
        assert_eq!(config.device.cpu.architecture, device.hardware.cpu.arch);
        assert_eq!(config.device.memory, device.hardware.memory);
        assert_eq!(config.device.pointing_device, device.hardware.inputs.pointing_device);
        assert_eq!(config.device.screen, device.hardware.window_size);
        assert_eq!(config.device.storage, device.hardware.storage);
        assert_eq!(config.device.vsock, Some(device.hardware.vsock));
        assert_eq!(config.device.avx2_enabled, device.avx2_enabled);

        assert!(config.guest.disk_image.is_some());

        assert_eq!(
            config.guest.disk_image.unwrap(),
            DiskImage::Fvm(expected_disk_image_path.into())
        );
        assert_eq!(config.guest.kernel_image, Some(expected_kernel.into()));
        assert_eq!(
            config.guest.ramdisk,
            Some(Ramdisk { path: expected_zbi.into_std_path_buf(), kind: RamdiskKind::Zbi })
        );

        assert_eq!(config.host.port_map.len(), 0);

        // Adjust all of the values that affect the config, then run it again.
        let expected_kernel = Utf8PathBuf::from_path_buf(sdk_root.join("some/new_kernel"))
            .expect("couldn't convert kernel to utf8");
        let expected_disk_image_path = Utf8PathBuf::from_path_buf(sdk_root.join("fxfs.sparse.blk"))
            .expect("couldn't convert fxfs sparse to utf8");
        let expected_zbi = Utf8PathBuf::from_path_buf(sdk_root.join("path/to/new_zbi"))
            .expect("couldn't convert zbi to utf8");

        pb.system_a = Some(vec![
            Image::ZBI { path: expected_zbi.clone(), signed: false },
            Image::QemuKernel(expected_kernel.clone()),
            Image::FxfsSparse {
                path: expected_disk_image_path.clone(),
                contents: BlobfsContents::default(),
            },
        ]);
        device.hardware = Hardware {
            cpu: Cpu { arch: CpuArchitecture::Arm64, count: 4 },
            audio: AudioDevice { model: AudioModel::None },
            storage: DataAmount { quantity: 8, units: DataUnits::Gigabytes },
            inputs: InputDevice { pointing_device: PointingDevice::Touch },
            memory: DataAmount { quantity: 2048, units: DataUnits::Megabytes },
            window_size: Screen { height: 1024, width: 1280, units: ScreenUnits::Pixels },
            vsock: VsockDevice { enabled: false, cid: 0 },
        };

        let mut ports = HashMap::new();
        ports.insert("ssh".to_string(), 22);
        ports.insert("debug".to_string(), 2345);
        device.ports = Some(ports);
        device.avx2_enabled = false;

        let mut config = convert_v2_bundle_to_configs(&pb, &device, false)
            .expect("convert_bundle_v2_to_configs");

        // Verify that all of the new values are loaded and match the new manifest data.
        assert_eq!(config.device.audio, device.hardware.audio);
        assert_eq!(config.device.cpu.architecture, device.hardware.cpu.arch);
        assert_eq!(config.device.memory, device.hardware.memory);
        assert_eq!(config.device.pointing_device, device.hardware.inputs.pointing_device);
        assert_eq!(config.device.screen, device.hardware.window_size);
        assert_eq!(config.device.storage, device.hardware.storage);
        assert_eq!(config.device.vsock, Some(device.hardware.vsock));
        assert_eq!(config.device.avx2_enabled, device.avx2_enabled);

        assert!(config.guest.disk_image.is_some());

        assert_eq!(
            config.guest.disk_image.unwrap(),
            DiskImage::Fxfs(expected_disk_image_path.into())
        );
        assert_eq!(config.guest.kernel_image, Some(expected_kernel.into()));
        assert_eq!(
            config.guest.ramdisk,
            Some(Ramdisk { path: expected_zbi.into_std_path_buf(), kind: RamdiskKind::Zbi })
        );

        assert_eq!(config.host.port_map.len(), 2);
        assert!(config.host.port_map.contains_key("ssh"));
        assert_eq!(
            config.host.port_map.remove("ssh").unwrap(),
            PortMapping { host: None, guest: 22 }
        );
        assert!(config.host.port_map.contains_key("debug"));
        assert_eq!(
            config.host.port_map.remove("debug").unwrap(),
            PortMapping { host: None, guest: 2345 }
        );
    }

    #[test]
    fn test_convert_v2_bundle_to_uefi_config() {
        let temp_dir = tempfile::TempDir::new().expect("creating sdk_root temp dir");
        let sdk_root = temp_dir.path();
        let template_path = sdk_root.join("fake_template");
        std::fs::write(&template_path, b"").expect("create fake template file");

        let expected_kernel = Utf8PathBuf::from_path_buf(sdk_root.join("some/new_kernel"))
            .expect("couldn't convert kernel to utf8");
        let expected_disk_image_path = Utf8PathBuf::from_path_buf(sdk_root.join("fxfs.sparse"))
            .expect("couldn't convert fxfs sprase to utf8");
        let expected_zbi = Utf8PathBuf::from_path_buf(sdk_root.join("path/to/new_zbi"))
            .expect("couldn't convert zbi to utf8");

        let pb = ProductBundleV2 {
            product_name: String::default(),
            product_version: String::default(),
            partitions: PartitionsConfig::default(),
            sdk_version: String::default(),
            system_a: Some(vec![
                Image::ZBI { path: expected_zbi.clone(), signed: false },
                Image::QemuKernel(expected_kernel.clone()),
                Image::FxfsSparse {
                    path: expected_disk_image_path.clone(),
                    contents: BlobfsContents::default(),
                },
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        };
        let mut device = VirtualDeviceV1 {
            name: "FakeDevice".to_string(),
            description: Some("A fake virtual device".to_string()),
            kind: ElementType::VirtualDevice,
            hardware: Hardware {
                cpu: Cpu { arch: CpuArchitecture::X64, count: 4 },
                audio: AudioDevice { model: AudioModel::Hda },
                storage: DataAmount { quantity: 512, units: DataUnits::Megabytes },
                inputs: InputDevice { pointing_device: PointingDevice::Mouse },
                memory: DataAmount { quantity: 4, units: DataUnits::Gigabytes },
                window_size: Screen { height: 480, width: 640, units: ScreenUnits::Pixels },
                vsock: VsockDevice { enabled: false, cid: 0 },
            },
            avx2_enabled: true,
            ports: None,
        };
        let mut ports = HashMap::new();
        ports.insert("ssh".to_string(), 22);
        ports.insert("debug".to_string(), 2345);
        device.ports = Some(ports);

        let mut config =
            convert_v2_bundle_to_configs(&pb, &device, true).expect("convert_bundle_v2_to_configs");

        assert_eq!(config.device.audio, device.hardware.audio);
        assert_eq!(config.device.cpu.architecture, device.hardware.cpu.arch);
        assert_eq!(config.device.memory, device.hardware.memory);
        assert_eq!(config.device.pointing_device, device.hardware.inputs.pointing_device);
        assert_eq!(config.device.screen, device.hardware.window_size);
        assert_eq!(config.device.storage, device.hardware.storage);
        assert_eq!(config.device.vsock, Some(device.hardware.vsock));

        assert!(config.guest.disk_image.is_some());

        // After converting the v2 bundle to config, the resulting guest config should contain the
        // newly assembled GPT full disk image, not the source Fxfs image.
        assert_eq!(
            config.guest.disk_image.unwrap(),
            DiskImage::Gpt(expected_disk_image_path.into())
        );
        assert_eq!(config.guest.kernel_image, Some(expected_kernel.into()));
        assert_eq!(
            config.guest.ramdisk,
            Some(Ramdisk { path: expected_zbi.into_std_path_buf(), kind: RamdiskKind::Zbi })
        );

        assert_eq!(config.host.port_map.len(), 2);
        assert!(config.host.port_map.contains_key("ssh"));
        assert_eq!(
            config.host.port_map.remove("ssh").unwrap(),
            PortMapping { host: None, guest: 22 }
        );
        assert!(config.host.port_map.contains_key("debug"));
        assert_eq!(
            config.host.port_map.remove("debug").unwrap(),
            PortMapping { host: None, guest: 2345 }
        );
    }

    #[test]
    fn test_efi_product_bundle() {
        let temp_dir = tempfile::TempDir::new().expect("creating sdk_root temp dir");
        let sdk_root = temp_dir.path();

        // Set up some test data to pass into the conversion routine.
        let expected_kernel = Utf8PathBuf::from_path_buf(sdk_root.join("kernel.efi"))
            .expect("couldn't convert kernel to utf8");

        let pb = ProductBundleV2 {
            product_name: String::default(),
            product_version: String::default(),
            partitions: PartitionsConfig::default(),
            sdk_version: String::default(),
            system_a: Some(vec![Image::QemuKernel(expected_kernel.clone())]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        };
        let device = VirtualDeviceV1 {
            name: "FakeDevice".to_string(),
            description: Some("A fake virtual device".to_string()),
            kind: ElementType::VirtualDevice,
            hardware: Hardware {
                cpu: Cpu { arch: CpuArchitecture::X64, count: 4 },
                audio: AudioDevice { model: AudioModel::Hda },
                storage: DataAmount { quantity: 512, units: DataUnits::Megabytes },
                inputs: InputDevice { pointing_device: PointingDevice::Mouse },
                memory: DataAmount { quantity: 4, units: DataUnits::Gigabytes },
                window_size: Screen { height: 480, width: 640, units: ScreenUnits::Pixels },
                vsock: VsockDevice { enabled: false, cid: 0 },
            },
            avx2_enabled: true,
            ports: None,
        };

        // Run the conversion, then assert everything in the config matches the manifest data.
        let config = convert_v2_bundle_to_configs(&pb, &device, false)
            .expect("convert_v2_bundle_to_configs");
        assert_eq!(config.device.audio, device.hardware.audio);
        assert_eq!(config.device.cpu.architecture, device.hardware.cpu.arch);
        assert_eq!(config.device.memory, device.hardware.memory);
        assert_eq!(config.device.pointing_device, device.hardware.inputs.pointing_device);
        assert_eq!(config.device.screen, device.hardware.window_size);
        assert_eq!(config.device.storage, device.hardware.storage);
        assert_eq!(config.device.vsock, Some(device.hardware.vsock));

        assert!(config.guest.disk_image.is_none());
        assert!(config.guest.ramdisk.is_none());

        assert_eq!(config.guest.kernel_image, Some(expected_kernel.into()));

        assert_eq!(config.host.port_map.len(), 0);
    }

    #[test]
    fn test_bootloader_efi_shell_product_bundle() {
        let pb = ProductBundleV2 {
            product_name: String::default(),
            product_version: String::default(),
            partitions: PartitionsConfig {
                bootloader_partitions: vec![BootloaderPartition {
                    partition_type: "efi-shell".into(),
                    name: Some("some-efi-shell.fatfs".into()),
                    image: "partitions/bootloaders/some-efi-shell.fat".into(),
                }],
                hardware_revision: "x64".into(),
                ..Default::default()
            },

            sdk_version: String::default(),
            system_a: Some(vec![]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        };
        let device = VirtualDeviceV1 {
            name: "FakeDevice".to_string(),
            description: Some("A fake virtual device".to_string()),
            kind: ElementType::VirtualDevice,
            hardware: Hardware {
                cpu: Cpu { arch: CpuArchitecture::X64, count: 4 },
                audio: AudioDevice { model: AudioModel::Hda },
                storage: DataAmount { quantity: 512, units: DataUnits::Megabytes },
                inputs: InputDevice { pointing_device: PointingDevice::Mouse },
                memory: DataAmount { quantity: 4, units: DataUnits::Gigabytes },
                window_size: Screen { height: 480, width: 640, units: ScreenUnits::Pixels },
                vsock: VsockDevice { enabled: false, cid: 0 },
            },
            avx2_enabled: true,
            ports: None,
        };

        // Run the conversion, then assert everything in the config matches the manifest data.
        let config = convert_v2_bundle_to_configs(&pb, &device, false)
            .expect("convert_v2_bundle_to_configs");
        assert_eq!(config.device.audio, device.hardware.audio);
        assert_eq!(config.device.cpu.architecture, device.hardware.cpu.arch);
        assert_eq!(config.device.memory, device.hardware.memory);
        assert_eq!(config.device.pointing_device, device.hardware.inputs.pointing_device);
        assert_eq!(config.device.screen, device.hardware.window_size);
        assert_eq!(config.device.storage, device.hardware.storage);
        assert_eq!(config.device.vsock, Some(device.hardware.vsock));

        assert_eq!(
            config.guest.disk_image,
            Some(DiskImage::Fat("partitions/bootloaders/some-efi-shell.fat".into()))
        );
        assert!(config.guest.ramdisk.is_none());

        assert!(config.guest.kernel_image.is_none());

        assert_eq!(config.host.port_map.len(), 0);
    }

    #[test]
    fn test_test_ramdisk_product_bundle() {
        let temp_dir = tempfile::TempDir::new().expect("creating sdk_root temp dir");
        let sdk_root = temp_dir.path();

        // Set up some test data to pass into the conversion routine.
        let expected_kernel = Utf8PathBuf::from_path_buf(sdk_root.join("kernel"))
            .expect("couldn't convert kernel to utf8");
        let expected_test_ramdisk = Utf8PathBuf::from_path_buf(sdk_root.join("test-ramdisk"))
            .expect("couldn't convert kernel to utf8");

        let pb = ProductBundleV2 {
            product_name: String::default(),
            product_version: String::default(),
            partitions: PartitionsConfig::default(),
            sdk_version: String::default(),
            system_a: Some(vec![
                Image::QemuKernel(expected_kernel.clone()),
                Image::TestRamdisk(expected_test_ramdisk.clone()),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        };
        let device = VirtualDeviceV1 {
            name: "FakeDevice".to_string(),
            description: Some("A fake virtual device".to_string()),
            kind: ElementType::VirtualDevice,
            hardware: Hardware {
                cpu: Cpu { arch: CpuArchitecture::X64, count: 4 },
                audio: AudioDevice { model: AudioModel::Hda },
                storage: DataAmount { quantity: 512, units: DataUnits::Megabytes },
                inputs: InputDevice { pointing_device: PointingDevice::Mouse },
                memory: DataAmount { quantity: 4, units: DataUnits::Gigabytes },
                window_size: Screen { height: 480, width: 640, units: ScreenUnits::Pixels },
                vsock: VsockDevice { enabled: false, cid: 0 },
            },
            avx2_enabled: true,
            ports: None,
        };

        // Run the conversion, then assert everything in the config matches the manifest data.
        let config = convert_v2_bundle_to_configs(&pb, &device, false)
            .expect("convert_v2_bundle_to_configs");

        assert_eq!(config.guest.kernel_image, Some(expected_kernel.into()));
        assert_eq!(
            config.guest.ramdisk,
            Some(Ramdisk { path: expected_test_ramdisk.into(), kind: RamdiskKind::Test })
        );
    }
}
