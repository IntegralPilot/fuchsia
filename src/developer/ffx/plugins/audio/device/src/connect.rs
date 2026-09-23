// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::control::{self as control, DeviceControl};
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_audio_controller as fac;
use fdomain_fuchsia_audio_device as fadevice;
use fdomain_fuchsia_hardware_audio as fhaudio;
use fdomain_fuchsia_io as fio;
use ffx_command_error::{FfxContext as _, Result, bug, return_bug};
use fuchsia_audio_fdomain::device::Selector;

/// Connect to an instance of a FIDL protocol hosted in `directory` using the given `path`.
// This is the same as `fuchsia_component::client::connect_to_named_protocol_at_dir_root`, however
// the `fuchsia_component` library doesn't build on host.
fn connect_to_named_protocol_at_dir_root<P: fdomain_client::fidl::ProtocolMarker>(
    directory: &fio::DirectoryProxy,
    path: &str,
) -> Result<P::Proxy> {
    let (proxy, server_end) = directory.domain().create_proxy::<P>();
    directory
        .open(path, fio::Flags::PROTOCOL_SERVICE, &Default::default(), server_end.into_channel())
        .bug_context("Failed to call Directory.Open")?;
    Ok(proxy)
}

/// Connects to the `fuchsia.hardware.audio.Codec` protocol node in the `dev_class` directory
/// at `path`.
pub fn connect_hw_codec(
    dev_class: &fio::DirectoryProxy,
    path: &str,
) -> Result<fhaudio::CodecProxy> {
    let connector_proxy =
        connect_to_named_protocol_at_dir_root::<fhaudio::CodecConnectorMarker>(dev_class, path)
            .bug_context("Failed to connect to CodecConnector")?;

    let (proxy, server_end) = dev_class.domain().create_proxy::<fhaudio::CodecMarker>();
    connector_proxy.connect(server_end).bug_context("Failed to call Connect")?;

    Ok(proxy)
}

/// Connects to the `fuchsia.hardware.audio.Composite` protocol node in the `dev_class` directory
/// at `path`.
pub fn connect_hw_composite(
    dev_class: &fio::DirectoryProxy,
    path: &str,
) -> Result<fhaudio::CompositeProxy> {
    // DFv2 Composite drivers do not use a connector/trampoline like Codec/Dai/StreamConfig.
    connect_to_named_protocol_at_dir_root::<fhaudio::CompositeMarker>(dev_class, path)
        .bug_context("Failed to connect to Composite")
}

/// Connects to the `fuchsia.hardware.audio.Dai` protocol node in the `dev_class` directory
/// at `path`.
pub fn connect_hw_dai(dev_class: &fio::DirectoryProxy, path: &str) -> Result<fhaudio::DaiProxy> {
    let connector_proxy =
        connect_to_named_protocol_at_dir_root::<fhaudio::DaiConnectorMarker>(dev_class, path)
            .bug_context("Failed to connect to DaiConnector")?;

    let (proxy, server_end) = dev_class.domain().create_proxy::<fhaudio::DaiMarker>();
    connector_proxy.connect(server_end).bug_context("Failed to call Connect")?;

    Ok(proxy)
}

/// Connects to the `fuchsia.hardware.audio.StreamConfig` protocol node in the `dev_class` directory
/// at `path`.
pub fn connect_hw_streamconfig(
    dev_class: &fio::DirectoryProxy,
    path: &str,
) -> Result<fhaudio::StreamConfigProxy> {
    let connector_proxy = connect_to_named_protocol_at_dir_root::<
        fhaudio::StreamConfigConnectorMarker,
    >(dev_class, path)
    .bug_context("Failed to connect to StreamConfigConnector")?;

    let (proxy, server_end) = dev_class.domain().create_proxy::<fhaudio::StreamConfigMarker>();
    connector_proxy.connect(server_end).bug_context("Failed to call Connect")?;

    Ok(proxy)
}

/// Connects to the `fuchsia.audio.device.Control` protocol for a device in the registry.
pub async fn connect_registry_control(
    control_creator: &fadevice::ControlCreatorProxy,
    token_id: fadevice::TokenId,
) -> Result<fadevice::ControlProxy> {
    let (proxy, server_end) = control_creator.domain().create_proxy::<fadevice::ControlMarker>();

    control_creator
        .create(fadevice::ControlCreatorCreateRequest {
            token_id: Some(token_id),
            control_server: Some(server_end),
            ..Default::default()
        })
        .await
        .bug_context("Failed to call ControlCreator.Create")?
        .map_err(|err| bug!("Failed to create Control: {:?}", err))?;

    Ok(proxy)
}

/// Connects to the control protocol of the device identified by `selector`.
pub async fn connect_device_control(
    dev_class: &fio::DirectoryProxy,
    control_creator: Option<&fadevice::ControlCreatorProxy>,
    selector: Selector,
) -> Result<Box<dyn DeviceControl>> {
    let device_control: Box<dyn DeviceControl> = match selector {
        Selector::Devfs(devfs_selector) => {
            let protocol_path = devfs_selector.relative_path();

            match devfs_selector.0.device_type {
                fac::DeviceType::Codec => {
                    let codec = connect_hw_codec(dev_class, protocol_path.as_str())?;
                    Box::new(control::HardwareCodec(codec))
                }
                fac::DeviceType::Composite => {
                    let composite = connect_hw_composite(dev_class, protocol_path.as_str())?;
                    Box::new(control::HardwareComposite(composite))
                }
                fac::DeviceType::Dai => {
                    let dai = connect_hw_dai(dev_class, protocol_path.as_str())?;
                    Box::new(control::HardwareDai(dai))
                }
                fac::DeviceType::Input | fac::DeviceType::Output => {
                    let streamconfig = connect_hw_streamconfig(dev_class, protocol_path.as_str())?;
                    Box::new(control::HardwareStreamConfig(streamconfig))
                }
                _ => return_bug!("Unknown device type: {:?}", devfs_selector.0.device_type),
            }
        }
        Selector::Registry(registry_selector) => {
            let control_creator =
                control_creator.ok_or_else(|| bug!("ControlCreator is not available"))?;
            let control =
                connect_registry_control(control_creator, registry_selector.token_id()).await?;
            Box::new(control::Registry(control))
        }
    };
    Ok(device_control)
}
