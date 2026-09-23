// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Display;

use fidl_fuchsia_hardware_network as fhwnet;

use anyhow::Context as _;

use crate::errors::{self, ContextExt as _};
use crate::exit_with_fidl_error;

/// An error when adding a device.
pub(super) enum AddDeviceError {
    AlreadyExists(String),
    Other(errors::Error),
}

impl From<errors::Error> for AddDeviceError {
    fn from(e: errors::Error) -> AddDeviceError {
        AddDeviceError::Other(e)
    }
}

impl errors::ContextExt for AddDeviceError {
    fn context<C>(self, context: C) -> AddDeviceError
    where
        C: Display + Send + Sync + 'static,
    {
        match self {
            AddDeviceError::AlreadyExists(name) => AddDeviceError::AlreadyExists(name),
            AddDeviceError::Other(e) => AddDeviceError::Other(e.context(context)),
        }
    }

    fn with_context<C, F>(self, f: F) -> AddDeviceError
    where
        C: Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        match self {
            AddDeviceError::AlreadyExists(name) => AddDeviceError::AlreadyExists(name),
            AddDeviceError::Other(e) => AddDeviceError::Other(e.with_context(f)),
        }
    }
}

// Cannot be completely replaced by `DeviceInfoRef` due to `get_device_info`
// not being able to return a struct of references.
#[derive(Debug, Clone)]
pub(super) struct DeviceInfo {
    pub(super) port_class: fhwnet::PortClass,
    pub(super) mac: Option<fidl_fuchsia_net_ext::MacAddress>,
    pub(super) topological_path: String,
}

/// An instance of a network device.
pub(super) struct NetworkDeviceInstance {
    port: fhwnet::PortProxy,
    port_id: fhwnet::PortId,
    device_control: fidl_fuchsia_net_interfaces_admin::DeviceControlProxy,
    topological_path: String,
}

impl std::fmt::Debug for NetworkDeviceInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let NetworkDeviceInstance { port: _, port_id, device_control: _, topological_path } = self;
        write!(
            f,
            "NetworkDeviceInstance{{topological_path={}, port={:?}}}",
            topological_path, port_id
        )
    }
}

impl NetworkDeviceInstance {
    pub async fn get_instance_stream(
        installer: &fidl_fuchsia_net_interfaces_admin::InstallerProxy,
        service: fhwnet::ServiceProxy,
    ) -> Result<impl futures::Stream<Item = Result<Self, errors::Error>> + use<>, errors::Error>
    {
        let device = service
            .connect_to_device()
            .context("cannot connect to device")
            .map_err(errors::Error::NonFatal)?;
        // TODO(https://fxbug.dev/501161302): Use
        // `fuchsia.driver.token.NodeBusTopology`.
        let device_topology = service
            .connect_to_device_topology()
            .context("cannot connect to device_topology")
            .map_err(errors::Error::NonFatal)?;

        let topological_path = device_topology
            .get_topological_path()
            .await
            .context("error sending get topological path request")
            .map_err(errors::Error::NonFatal)?
            .map_err(zx::Status::err_from_raw)
            .context("error getting topological path")
            .map_err(errors::Error::NonFatal)?;

        let (port_watcher, port_watcher_server_end) =
            fidl::endpoints::create_proxy::<fhwnet::PortWatcherMarker>();
        device
            .get_port_watcher(port_watcher_server_end)
            .context("calling Device get_port_watcher")
            .map_err(errors::Error::NonFatal)?;

        let (device_control, device_control_server_end) = fidl::endpoints::create_proxy::<
            fidl_fuchsia_net_interfaces_admin::DeviceControlMarker,
        >();

        let (device_for_netstack, device_server_end) =
            fidl::endpoints::create_endpoints::<fhwnet::DeviceMarker>();
        device
            .clone(device_server_end)
            .context("calling Device clone")
            .map_err(errors::Error::NonFatal)?;
        installer
            .install_device(device_for_netstack, device_control_server_end)
            // NB: Failing to communicate with installer is a fatal error, that
            // means the Netstack is gone, which we don't tolerate.
            .unwrap_or_else(|err| exit_with_fidl_error(err));

        Ok(futures::stream::try_unfold(
            (port_watcher, device_control, device, topological_path),
            |(port_watcher, device_control, device, topological_path)| async move {
                loop {
                    let port_event = match port_watcher.watch().await {
                        Ok(port_event) => port_event,
                        Err(err) => {
                            break if err.is_closed() {
                                Ok(None)
                            } else {
                                Err(errors::Error::Fatal(err.into()))
                                    .context("calling PortWatcher watch")
                            };
                        }
                    };
                    match port_event {
                        fhwnet::DevicePortEvent::Idle(fhwnet::Empty {}) => {}
                        fhwnet::DevicePortEvent::Removed(port_id) => {
                            let _: fhwnet::PortId = port_id;
                        }
                        fhwnet::DevicePortEvent::Added(port_id)
                        | fhwnet::DevicePortEvent::Existing(port_id) => {
                            let (port, port_server_end) =
                                fidl::endpoints::create_proxy::<fhwnet::PortMarker>();
                            device
                                .get_port(&port_id, port_server_end)
                                .context("calling Device get_port")
                                .map_err(errors::Error::NonFatal)?;
                            break Ok(Some((
                                NetworkDeviceInstance {
                                    port,
                                    port_id,
                                    device_control: device_control.clone(),
                                    topological_path: topological_path.clone(),
                                },
                                (port_watcher, device_control, device, topological_path),
                            )));
                        }
                    }
                }
            },
        ))
    }

    pub async fn get_device_info(&self) -> Result<DeviceInfo, errors::Error> {
        let NetworkDeviceInstance { port, port_id: _, device_control: _, topological_path } = self;
        let fhwnet::PortInfo { id: _, base_info, .. } = port
            .get_info()
            .await
            .context("error getting port info")
            .map_err(errors::Error::NonFatal)?;
        let port_class = base_info
            .ok_or_else(|| errors::Error::Fatal(anyhow::anyhow!("missing base info in port info")))?
            .port_class
            .ok_or_else(|| {
                errors::Error::Fatal(anyhow::anyhow!("missing port class in port base info"))
            })?;

        let (mac_addressing, mac_addressing_server_end) =
            fidl::endpoints::create_proxy::<fhwnet::MacAddressingMarker>();
        port.get_mac(mac_addressing_server_end)
            .context("calling Port get_mac")
            .map_err(errors::Error::NonFatal)?;

        let mac = mac_addressing
            .get_unicast_address()
            .await
            .map(Some)
            .or_else(|fidl_err| {
                if fidl_err.is_closed() { Ok(None) } else { Err(anyhow::Error::from(fidl_err)) }
            })
            .map_err(errors::Error::NonFatal)?;
        Ok(DeviceInfo {
            port_class,
            mac: mac.map(Into::into),
            topological_path: topological_path.clone(),
        })
    }

    pub async fn add_to_stack(
        &self,
        _netcfg: &super::NetCfg<'_>,
        config: crate::InterfaceConfig,
    ) -> Result<(u64, fidl_fuchsia_net_interfaces_ext::admin::Control), AddDeviceError> {
        let NetworkDeviceInstance { port: _, port_id, device_control, topological_path: _ } = self;
        let crate::InterfaceConfig { name, metric, netstack_managed_routes_designation } = config;

        let (control, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .context("create Control proxy")
                .map_err(errors::Error::NonFatal)?;

        device_control
            .create_interface(
                &port_id,
                control_server_end,
                fidl_fuchsia_net_interfaces_admin::Options {
                    name: Some(name.clone()),
                    metric: Some(metric),
                    netstack_managed_routes_designation: netstack_managed_routes_designation
                        .map(Into::into),
                    ..Default::default()
                },
            )
            .context("calling DeviceControl create_interface")
            .map_err(errors::Error::NonFatal)?;

        let interface_id = control.get_id().await.map_err(|err| {
            let other = match err {
                fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Fidl(err) => err.into(),
                fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Terminal(terminal_error) => {
                    match terminal_error {
                        fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::DuplicateName => {
                            return AddDeviceError::AlreadyExists(name);
                        }
                        reason => {
                            anyhow::anyhow!("received terminal event {:?}", reason)
                        }
                    }
                }
            };
            AddDeviceError::Other(
                errors::Error::NonFatal(other).context("calling Control get_id"),
            )
        })?;
        Ok((interface_id, control))
    }
}
