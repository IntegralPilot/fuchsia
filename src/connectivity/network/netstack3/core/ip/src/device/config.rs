// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IP Device configuration.

use core::num::{NonZeroU8, NonZeroU16};

use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv6};
use netstack3_base::{AnyDevice, DeviceIdContext, DeviceIdentifier};

use crate::internal::device::route_discovery::RouteDiscoveryConfigurationUpdate;
use crate::internal::device::slaac::SlaacConfigurationUpdate;
use crate::internal::device::state::{
    IpDeviceConfiguration, IpDeviceFlags, Ipv4DeviceConfiguration,
};
use crate::internal::device::{
    self, IpDeviceBindingsContext, IpDeviceConfigurationContext, IpDeviceEvent, IpDeviceIpExt,
    Ipv6DeviceConfigurationContext, WithIpDeviceConfigurationMutInner as _,
    WithIpv6DeviceConfigurationMutInner as _,
};
use crate::internal::gmp::GmpHandler;
use crate::internal::gmp::igmp::IgmpConfigMode;
use crate::internal::gmp::mld::MldConfigMode;

/// A trait abstracting configuration between IPv4 and IPv6.
///
/// Configuration is different enough between IPv4 and IPv6 that the
/// implementations are completely disjoint. This trait allows us to implement
/// these completely separately but still offer a unified configuration update
/// API.
pub trait IpDeviceConfigurationHandler<I: IpDeviceIpExt, BC>: DeviceIdContext<AnyDevice> {
    /// Applies the [`PendingIpDeviceConfigurationUpdate`].
    fn apply_configuration(
        &mut self,
        bindings_ctx: &mut BC,
        config: PendingIpDeviceConfigurationUpdate<'_, I, Self::DeviceId>,
    ) -> I::ConfigurationUpdate;
}

/// An update to IP device configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, GenericOverIp)]
#[generic_over_ip()]
pub struct IpDeviceConfigurationUpdate {
    /// A change in IP enabled.
    pub ip_enabled: Option<bool>,
    /// A change in unicast forwarding enabled.
    pub unicast_forwarding_enabled: Option<bool>,
    /// A change in multicast forwarding enabled.
    pub multicast_forwarding_enabled: Option<bool>,
    /// A change in Group Messaging Protocol (GMP) enabled.
    pub gmp_enabled: Option<bool>,
    /// A change in DAD transmits.
    pub dad_transmits: Option<Option<NonZeroU16>>,
}

/// An update to IPv4 device configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ipv4DeviceConfigurationUpdate {
    /// A change in the IP device configuration.
    pub ip_config: IpDeviceConfigurationUpdate,
    /// A change in the IGMP mode.
    pub igmp_mode: Option<IgmpConfigMode>,
}

impl From<IpDeviceConfigurationUpdate> for Ipv4DeviceConfigurationUpdate {
    fn from(ip_config: IpDeviceConfigurationUpdate) -> Self {
        Self { ip_config, ..Default::default() }
    }
}

impl AsRef<IpDeviceConfigurationUpdate> for Ipv4DeviceConfigurationUpdate {
    fn as_ref(&self) -> &IpDeviceConfigurationUpdate {
        &self.ip_config
    }
}

/// Errors observed from updating a device's IP configuration.
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum UpdateIpConfigurationError {
    /// Unicast Forwarding is not supported in the target interface.
    UnicastForwardingNotSupported,
    /// Multicast Forwarding is not supported in the target interface.
    MulticastForwardingNotSupported,
}

/// A validated and pending IP device configuration update.
///
/// This type is a witness for a valid IP configuration for a device ID `D`.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct PendingIpDeviceConfigurationUpdate<'a, I: IpDeviceIpExt, D>(
    I::ConfigurationUpdate,
    &'a D,
);

impl<'a, I: IpDeviceIpExt, D: DeviceIdentifier> PendingIpDeviceConfigurationUpdate<'a, I, D> {
    /// Creates a new [`PendingIpDeviceConfigurationUpdate`] if `config` is
    /// valid for `device`.
    pub(crate) fn new(
        config: I::ConfigurationUpdate,
        device_id: &'a D,
    ) -> Result<Self, UpdateIpConfigurationError> {
        let IpDeviceConfigurationUpdate {
            ip_enabled: _,
            gmp_enabled: _,
            unicast_forwarding_enabled,
            multicast_forwarding_enabled,
            dad_transmits: _,
        } = config.as_ref();

        if device_id.is_loopback() {
            if unicast_forwarding_enabled.unwrap_or(false) {
                return Err(UpdateIpConfigurationError::UnicastForwardingNotSupported);
            }
            if multicast_forwarding_enabled.unwrap_or(false) {
                return Err(UpdateIpConfigurationError::MulticastForwardingNotSupported);
            }
        }

        Ok(Self(config, device_id))
    }

    /// Returns the configuration update.
    pub fn config_update(&self) -> &I::ConfigurationUpdate {
        &self.0
    }

    /// Returns the device ID.
    pub fn device_id(&self) -> &'a D {
        self.1
    }
}

impl<CC, BC> IpDeviceConfigurationHandler<Ipv4, BC> for CC
where
    CC: IpDeviceConfigurationContext<Ipv4, BC>,
    BC: IpDeviceBindingsContext<Ipv4, CC::DeviceId>,
{
    fn apply_configuration(
        &mut self,
        bindings_ctx: &mut BC,
        config: PendingIpDeviceConfigurationUpdate<'_, Ipv4, Self::DeviceId>,
    ) -> Ipv4DeviceConfigurationUpdate {
        let PendingIpDeviceConfigurationUpdate(
            Ipv4DeviceConfigurationUpdate { ip_config, igmp_mode },
            device_id,
        ) = config;
        let device_id: &CC::DeviceId = device_id;

        // NB: Extracted to prevent deep nesting which breaks rustfmt.
        let handle_config_and_flags =
            |config: &mut Ipv4DeviceConfiguration, flags: &mut IpDeviceFlags| {
                let IpDeviceConfigurationUpdate {
                    ip_enabled,
                    gmp_enabled,
                    unicast_forwarding_enabled,
                    multicast_forwarding_enabled,
                    dad_transmits,
                } = ip_config;
                (
                    get_prev_next_and_update(&mut flags.ip_enabled, ip_enabled),
                    get_prev_next_and_update(&mut config.ip_config.gmp_enabled, gmp_enabled),
                    get_prev_next_and_update(
                        &mut config.ip_config.unicast_forwarding_enabled,
                        unicast_forwarding_enabled,
                    ),
                    get_prev_next_and_update(
                        &mut config.ip_config.multicast_forwarding_enabled,
                        multicast_forwarding_enabled,
                    ),
                    get_prev_next_and_update(&mut config.ip_config.dad_transmits, dad_transmits),
                )
            };

        self.with_ip_device_configuration_mut(device_id, |mut inner| {
            let (
                ip_enabled_updates,
                gmp_enabled_updates,
                unicast_forwarding_enabled_updates,
                multicast_forwarding_enabled_updates,
                dad_transmits,
            ) = inner.with_configuration_and_flags_mut(device_id, handle_config_and_flags);

            let (config, mut core_ctx) = inner.ip_device_configuration_and_ctx();
            let core_ctx = &mut core_ctx;

            let ip_enabled = handle_change_and_get_prev(ip_enabled_updates, |next| {
                if next {
                    device::enable_ipv4_device_with_config(
                        core_ctx,
                        bindings_ctx,
                        device_id,
                        config,
                    )
                } else {
                    device::disable_ipv4_device_with_config(
                        core_ctx,
                        bindings_ctx,
                        device_id,
                        config,
                    )
                }
                bindings_ctx.on_event(IpDeviceEvent::EnabledChanged {
                    device: device_id.clone(),
                    ip_enabled: next,
                })
            });

            // NB: change GMP mode before enabling in case those are changed
            // atomically.
            let igmp_mode = igmp_mode
                .map(|igmp_mode| core_ctx.gmp_set_mode(bindings_ctx, device_id, igmp_mode));

            let gmp_enabled = handle_change_and_get_prev(gmp_enabled_updates, |next| {
                if next {
                    GmpHandler::gmp_handle_maybe_enabled(core_ctx, bindings_ctx, device_id)
                } else {
                    GmpHandler::gmp_handle_disabled(core_ctx, bindings_ctx, device_id)
                }
            });
            let unicast_forwarding_enabled =
                dont_handle_change_and_get_prev(unicast_forwarding_enabled_updates);
            let multicast_forwarding_enabled =
                dont_handle_change_and_get_prev(multicast_forwarding_enabled_updates);
            let dad_transmits = dont_handle_change_and_get_prev(dad_transmits);
            let ip_config = IpDeviceConfigurationUpdate {
                ip_enabled,
                gmp_enabled,
                unicast_forwarding_enabled,
                multicast_forwarding_enabled,
                dad_transmits,
            };
            Ipv4DeviceConfigurationUpdate { ip_config, igmp_mode }
        })
    }
}

/// An update to IPv6 device configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ipv6DeviceConfigurationUpdate {
    /// A change in maximum router solicitations.
    pub max_router_solicitations: Option<Option<NonZeroU8>>,
    /// A change in SLAAC configuration.
    pub slaac_config: SlaacConfigurationUpdate,
    /// A change in route discovery configuration.
    pub route_discovery_config: RouteDiscoveryConfigurationUpdate,
    /// A change in the IP device configuration.
    pub ip_config: IpDeviceConfigurationUpdate,
    /// A change in the MLD mode.
    pub mld_mode: Option<MldConfigMode>,
}

impl From<IpDeviceConfigurationUpdate> for Ipv6DeviceConfigurationUpdate {
    fn from(ip_config: IpDeviceConfigurationUpdate) -> Self {
        Self { ip_config, ..Default::default() }
    }
}

impl AsRef<IpDeviceConfigurationUpdate> for Ipv6DeviceConfigurationUpdate {
    fn as_ref(&self) -> &IpDeviceConfigurationUpdate {
        &self.ip_config
    }
}

impl<CC, BC> IpDeviceConfigurationHandler<Ipv6, BC> for CC
where
    CC: Ipv6DeviceConfigurationContext<BC>,
    BC: IpDeviceBindingsContext<Ipv6, CC::DeviceId>,
{
    fn apply_configuration(
        &mut self,
        bindings_ctx: &mut BC,
        config: PendingIpDeviceConfigurationUpdate<'_, Ipv6, Self::DeviceId>,
    ) -> Ipv6DeviceConfigurationUpdate {
        let PendingIpDeviceConfigurationUpdate(
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations,
                slaac_config,
                route_discovery_config,
                ip_config,
                mld_mode,
            },
            device_id,
        ) = config;
        self.with_ipv6_device_configuration_mut(device_id, |mut inner| {
            let (
                max_router_solicitations_updates,
                slaac_config_updates,
                route_discovery_config_updates,
                ip_enabled_updates,
                gmp_enabled_updates,
                unicast_forwarding_enabled_updates,
                multicast_forwarding_enabled_updates,
                dad_transmits,
            ) = inner.with_configuration_and_flags_mut(device_id, |config, flags| {
                let IpDeviceConfigurationUpdate {
                    ip_enabled,
                    gmp_enabled,
                    unicast_forwarding_enabled,
                    multicast_forwarding_enabled,
                    dad_transmits,
                } = ip_config;
                (
                    get_prev_next_and_update(
                        &mut config.max_router_solicitations,
                        max_router_solicitations,
                    ),
                    config.slaac_config.update(slaac_config),
                    config.route_discovery_config.update(route_discovery_config),
                    get_prev_next_and_update(&mut flags.ip_enabled, ip_enabled),
                    get_prev_next_and_update(&mut config.ip_config.gmp_enabled, gmp_enabled),
                    get_prev_next_and_update(
                        &mut config.ip_config.unicast_forwarding_enabled,
                        unicast_forwarding_enabled,
                    ),
                    get_prev_next_and_update(
                        &mut config.ip_config.multicast_forwarding_enabled,
                        multicast_forwarding_enabled,
                    ),
                    get_prev_next_and_update(&mut config.ip_config.dad_transmits, dad_transmits),
                )
            });

            let (config, mut core_ctx) = inner.ipv6_device_configuration_and_ctx();
            let core_ctx = &mut core_ctx;

            let max_router_solicitations =
                dont_handle_change_and_get_prev(max_router_solicitations_updates);

            // NB: change GMP mode before enabling in case those are changed
            // atomically.
            let mld_mode =
                mld_mode.map(|mld_mode| core_ctx.gmp_set_mode(bindings_ctx, device_id, mld_mode));

            let ip_config = IpDeviceConfigurationUpdate {
                ip_enabled: handle_change_and_get_prev(ip_enabled_updates, |next| {
                    if next {
                        device::enable_ipv6_device_with_config(
                            core_ctx,
                            bindings_ctx,
                            device_id,
                            config,
                        )
                    } else {
                        device::disable_ipv6_device_with_config(
                            core_ctx,
                            bindings_ctx,
                            device_id,
                            config,
                        )
                    }

                    bindings_ctx.on_event(IpDeviceEvent::EnabledChanged {
                        device: device_id.clone(),
                        ip_enabled: next,
                    })
                }),
                gmp_enabled: handle_change_and_get_prev(gmp_enabled_updates, |next| {
                    if next {
                        GmpHandler::gmp_handle_maybe_enabled(core_ctx, bindings_ctx, device_id)
                    } else {
                        GmpHandler::gmp_handle_disabled(core_ctx, bindings_ctx, device_id)
                    }
                }),
                unicast_forwarding_enabled: handle_change_and_get_prev(
                    unicast_forwarding_enabled_updates,
                    |next| {
                        if next {
                            device::join_ip_multicast_with_config(
                                core_ctx,
                                bindings_ctx,
                                device_id,
                                Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS,
                                config,
                            );
                        } else {
                            device::leave_ip_multicast_with_config(
                                core_ctx,
                                bindings_ctx,
                                device_id,
                                Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS,
                                config,
                            );
                        }
                    },
                ),
                multicast_forwarding_enabled: dont_handle_change_and_get_prev(
                    multicast_forwarding_enabled_updates,
                ),
                dad_transmits: dont_handle_change_and_get_prev(dad_transmits),
            };
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations,
                slaac_config: slaac_config_updates,
                route_discovery_config: route_discovery_config_updates,
                ip_config,
                mld_mode,
            }
        })
    }
}

struct Delta<T> {
    prev: T,
    next: T,
}

fn get_prev_next_and_update<T: Copy>(field: &mut T, next: Option<T>) -> Option<Delta<T>> {
    next.map(|next| Delta { prev: core::mem::replace(field, next), next })
}

fn handle_change_and_get_prev<T: PartialEq>(
    delta: Option<Delta<T>>,
    f: impl FnOnce(T),
) -> Option<T> {
    delta.map(|Delta { prev, next }| {
        if prev != next {
            f(next)
        }
        prev
    })
}

fn dont_handle_change_and_get_prev<T: PartialEq>(delta: Option<Delta<T>>) -> Option<T> {
    handle_change_and_get_prev(delta, |_: T| {})
}

/// The device configurations and flags.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IpDeviceConfigurationAndFlags<I: IpDeviceIpExt> {
    /// The device configuration.
    pub config: I::Configuration,
    /// The device flags.
    pub flags: IpDeviceFlags,
    /// The GMP mode.
    pub gmp_mode: I::GmpProtoConfigMode,
}

impl<I: IpDeviceIpExt> AsRef<IpDeviceConfiguration> for IpDeviceConfigurationAndFlags<I> {
    fn as_ref(&self) -> &IpDeviceConfiguration {
        self.config.as_ref()
    }
}

impl<I: IpDeviceIpExt> AsMut<IpDeviceConfiguration> for IpDeviceConfigurationAndFlags<I> {
    fn as_mut(&mut self) -> &mut IpDeviceConfiguration {
        self.config.as_mut()
    }
}

impl<I: IpDeviceIpExt> AsRef<IpDeviceFlags> for IpDeviceConfigurationAndFlags<I> {
    fn as_ref(&self) -> &IpDeviceFlags {
        &self.flags
    }
}
