// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Context as _, Result};
use fidl_fuchsia_buildinfo as buildinfo;
use fidl_fuchsia_developer_remotecontrol as rcs;
use fidl_fuchsia_device as fdevice;
use fidl_fuchsia_hwinfo as hwinfo;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_sysinfo as sysinfo;
use log::*;
use std::collections::HashMap;

#[async_trait::async_trait]
pub trait Identifier {
    async fn identify(&self) -> Result<rcs::IdentifyHostResponse, rcs::IdentifyHostError>;
}

pub struct DefaultIdentifier {
    pub(crate) boot_timestamp_nanos: u64,
}

impl DefaultIdentifier {
    pub fn new() -> Self {
        let boot_timestamp_nanos = (fuchsia_runtime::utc_time().into_nanos()
            - zx::MonotonicInstant::get().into_nanos()) as u64;
        Self { boot_timestamp_nanos }
    }
}

#[async_trait::async_trait]
impl Identifier for DefaultIdentifier {
    async fn identify(&self) -> Result<rcs::IdentifyHostResponse, rcs::IdentifyHostError> {
        Ok(rcs::IdentifyHostResponse {
            nodename: Some("fuchsia-default-nodename".into()),
            serial_number: Some("fuchsia-default-serial-number".into()),
            boot_timestamp_nanos: Some(self.boot_timestamp_nanos),
            ..Default::default()
        })
    }
}

pub struct HostIdentifier {
    pub(crate) interface_state_proxy: fnet_interfaces::StateProxy,
    pub(crate) name_provider_proxy: fdevice::NameProviderProxy,
    pub(crate) device_info_proxy: hwinfo::DeviceProxy,
    pub(crate) system_info_proxy: sysinfo::SysInfoProxy,
    pub(crate) build_info_proxy: buildinfo::ProviderProxy,
    pub(crate) boot_timestamp_nanos: u64,
    pub(crate) boot_id: u64,
}

fn connect_to_protocol<P: fidl::endpoints::DiscoverableProtocolMarker>() -> Result<P::Proxy> {
    fuchsia_component::client::connect_to_protocol::<P>().context(P::DEBUG_NAME)
}

impl HostIdentifier {
    pub fn new(boot_id: u64) -> Result<Self> {
        let interface_state_proxy = connect_to_protocol::<fnet_interfaces::StateMarker>()?;
        let name_provider_proxy = connect_to_protocol::<fdevice::NameProviderMarker>()?;
        let device_info_proxy = connect_to_protocol::<hwinfo::DeviceMarker>()?;
        let system_info_proxy = connect_to_protocol::<sysinfo::SysInfoMarker>()?;
        let build_info_proxy = connect_to_protocol::<buildinfo::ProviderMarker>()?;
        let boot_timestamp_nanos =
            (fuchsia_runtime::utc_time().into_nanos() - zx::BootInstant::get().into_nanos()) as u64;
        return Ok(Self {
            interface_state_proxy,
            name_provider_proxy,
            device_info_proxy,
            system_info_proxy,
            build_info_proxy,
            boot_timestamp_nanos,
            boot_id,
        });
    }
}

#[async_trait::async_trait]
impl Identifier for HostIdentifier {
    async fn identify(&self) -> Result<rcs::IdentifyHostResponse, rcs::IdentifyHostError> {
        let stream = fnet_interfaces_ext::event_stream_from_state::<
            fnet_interfaces_ext::DefaultInterest,
        >(&self.interface_state_proxy, Default::default())
        .map_err(|e| {
            error!(e:%; "Getting interface watcher failed");
            rcs::IdentifyHostError::ListInterfacesFailed
        })?;
        let ilist = fnet_interfaces_ext::existing(
            stream,
            HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
        )
        .await
        .map_err(|e| {
            error!(e:%; "Getting existing interfaces failed");
            rcs::IdentifyHostError::ListInterfacesFailed
        })?;

        let serial_number = 'serial: {
            match self.system_info_proxy.get_serial_number().await {
                Ok(Ok(serial)) => break 'serial Some(serial),
                Ok(Err(status)) => {
                    let status = zx::Status::err_from_raw(status);
                    warn!(status:%; "Failed to get serial from SysInfo")
                }
                Err(err) => error!(err:%; "SysInfoProxy internal err"),
            }

            match self.device_info_proxy.get_info().await {
                Ok(info) => break 'serial info.serial_number,
                Err(err) => error!(err:%; "DeviceProxy internal err"),
            }

            None
        };

        let (product_config, board_config) = self
            .build_info_proxy
            .get_build_info()
            .await
            .map_err(|e| error!(e:%; "buildinfo::ProviderProxy internal err"))
            .ok()
            .and_then(|i| Some((i.product_config, i.board_config)))
            .unwrap_or((None, None));

        let addresses = ilist
            .into_iter()
            .map(|(_, v): (u64, _)| v)
            .flat_map(|properties_and_state| {
                properties_and_state.properties.addresses.into_iter().filter_map(
                    |fnet_interfaces_ext::Address { addr, assignment_state, .. }| {
                        match assignment_state {
                            fnet_interfaces::AddressAssignmentState::Assigned => Some(addr),
                            fnet_interfaces::AddressAssignmentState::Tentative
                            | fnet_interfaces::AddressAssignmentState::Unavailable => None,
                        }
                    },
                )
            })
            .collect::<Vec<_>>();

        let addresses = Some(addresses);

        let nodename = match self.name_provider_proxy.get_device_name().await {
            Ok(result) => match result {
                Ok(name) => Some(name),
                Err(err) => {
                    error!(err:%; "NameProvider internal error");
                    return Err(rcs::IdentifyHostError::GetDeviceNameFailed);
                }
            },
            Err(err) => {
                error!(err:%; "Getting nodename failed");
                return Err(rcs::IdentifyHostError::GetDeviceNameFailed);
            }
        };

        let boot_timestamp_nanos = Some(self.boot_timestamp_nanos);

        let boot_id = Some(self.boot_id);

        Ok(rcs::IdentifyHostResponse {
            nodename,
            addresses,
            serial_number,
            boot_timestamp_nanos,
            product_config,
            board_config,
            boot_id,
            ..Default::default()
        })
    }
}
