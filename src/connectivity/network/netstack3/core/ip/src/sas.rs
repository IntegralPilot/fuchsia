// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides common SAS (Source Address Selection) implementations.

use net_types::SpecifiedAddr;
use net_types::ip::{Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use netstack3_base::{IpAddressId, IpDeviceAddr, IpDeviceAddressIdContext};

use crate::internal::device::state::{IpAddressData, IpAddressFlags, IpDeviceStateBindingsTypes};
use crate::internal::device::{IpDeviceAddressContext as _, IpDeviceIpExt, IpDeviceStateContext};
use crate::internal::socket::ipv6_source_address_selection::{self, SasCandidate};

/// A handler for Source Address Selection.
///
/// This trait helps implement source address selection for a variety of traits,
/// like [`crate::IpDeviceStateContext`].
///
/// A blanket implementation on IPv4 and IPv6 is provided for all types
/// implementing [`IpDeviceStateContext`].
pub trait IpSasHandler<I: IpDeviceIpExt, BT>: IpDeviceAddressIdContext<I> {
    /// Returns the best local address on `device_id` for communicating with
    /// `remote`.
    fn get_local_addr_for_remote(
        &mut self,
        device_id: &Self::DeviceId,
        remote: Option<SpecifiedAddr<I::Addr>>,
    ) -> Option<IpDeviceAddr<I::Addr>> {
        self.get_local_addr_id_for_remote(device_id, remote).map(|id| id.addr())
    }

    /// Returns a strongly-held reference to the best local address on `device_id`
    /// for communicating with `remote`.
    fn get_local_addr_id_for_remote(
        &mut self,
        device_id: &Self::DeviceId,
        remote: Option<SpecifiedAddr<I::Addr>>,
    ) -> Option<Self::AddressId>;
}

impl<CC, BT> IpSasHandler<Ipv4, BT> for CC
where
    CC: IpDeviceStateContext<Ipv4, BT>,
    BT: IpDeviceStateBindingsTypes,
{
    fn get_local_addr_id_for_remote(
        &mut self,
        device_id: &Self::DeviceId,
        _remote: Option<SpecifiedAddr<Ipv4Addr>>,
    ) -> Option<CC::AddressId> {
        self.with_address_ids(device_id, |addrs, core_ctx| {
            // Tentative addresses are not considered available to the source
            // selection algorithm.
            addrs
                .filter(|addr_id| {
                    core_ctx.with_ip_address_data(
                        device_id,
                        addr_id,
                        |IpAddressData { flags: IpAddressFlags { assigned }, config: _ }| *assigned,
                    )
                })
                // Use the first viable candidate.
                .next()
        })
    }
}

impl<CC, BT> IpSasHandler<Ipv6, BT> for CC
where
    CC: IpDeviceStateContext<Ipv6, BT>,
    BT: IpDeviceStateBindingsTypes,
{
    fn get_local_addr_id_for_remote(
        &mut self,
        device_id: &Self::DeviceId,
        remote: Option<SpecifiedAddr<Ipv6Addr>>,
    ) -> Option<CC::AddressId> {
        self.with_address_ids(device_id, |addrs, core_ctx| {
            ipv6_source_address_selection::select_ipv6_source_address(
                remote,
                device_id,
                addrs,
                |addr_id| {
                    core_ctx.with_ip_address_data(
                        device_id,
                        addr_id,
                        |IpAddressData { flags: IpAddressFlags { assigned }, config }| {
                            // Assume an address is deprecated if config is
                            // not available. That means the address is
                            // going away, so we should not prefer it.
                            const ASSUME_DEPRECATED: bool = true;
                            // Assume an address is not temporary if config
                            // is not available. That means the address is
                            // going away and we should remove any
                            // preference on it.
                            const ASSUME_TEMPORARY: bool = false;
                            let (deprecated, temporary) = config
                                .map(|c| (c.is_deprecated(), c.is_temporary()))
                                .unwrap_or((ASSUME_DEPRECATED, ASSUME_TEMPORARY));
                            SasCandidate {
                                addr_sub: addr_id.addr_sub(),
                                assigned: *assigned,
                                temporary,
                                deprecated,
                                device: device_id.clone(),
                            }
                        },
                    )
                },
            )
        })
    }
}
