// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec::Vec;
use core::num::{NonZeroU8, NonZeroU16};
use core::time::Duration;

use assert_matches::assert_matches;
use net_declare::net_mac;
use net_types::ip::{AddrSubnet, Ip, Ipv4, Ipv6, Mtu};
use net_types::{SpecifiedAddr, UnicastAddr, Witness as _};
use netstack3_base::WorkQueueReport;
use netstack3_base::testutil::{FakeInstant, TestIpExt};
use netstack3_core::device::{
    DeviceId, EthernetCreationProperties, EthernetDeviceId, EthernetLinkDevice,
    LoopbackCreationProperties, LoopbackDevice, LoopbackDeviceId, MaxEthernetFrameSize,
    PureIpDevice,
};
use netstack3_core::error::NotFoundError;
use netstack3_core::ip::{
    AddIpAddrSubnetError, CommonAddressConfig, CommonAddressProperties,
    IpDeviceConfigurationUpdate, Ipv4AddrConfig, Ipv4DeviceConfigurationUpdate,
    Ipv6AddrManualConfig, Ipv6DeviceConfigurationUpdate, Lifetime, PreferredLifetime,
    SlaacConfigurationUpdate, StableSlaacAddressConfiguration,
};
use netstack3_core::testutil::{
    CtxPairExt as _, DEFAULT_INTERFACE_METRIC, FakeBindingsCtx, FakeCtx,
};
use netstack3_core::{IpExt, for_any_device_id};
use netstack3_device::testutil::IPV6_MIN_IMPLIED_MAX_FRAME_SIZE;
use test_case::test_case;

#[test]
fn test_no_default_routes() {
    let mut ctx = FakeCtx::default();
    let _loopback_device: LoopbackDeviceId<_> =
        ctx.core_api().device::<LoopbackDevice>().add_device_with_default_state(
            LoopbackCreationProperties { mtu: Mtu::new(55) },
            DEFAULT_INTERFACE_METRIC,
        );

    assert_eq!(ctx.core_api().routes_any().get_all_routes_in_main_table(), []);
    let _ethernet_device: EthernetDeviceId<_> =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: UnicastAddr::new(net_mac!("aa:bb:cc:dd:ee:ff")).expect("MAC is unicast"),
                max_frame_size: MaxEthernetFrameSize::MIN,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    assert_eq!(ctx.core_api().routes_any().get_all_routes_in_main_table(), []);
}

#[test]
fn remove_ethernet_device_disables_timers() {
    let mut ctx = FakeCtx::default();

    let ethernet_device =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: UnicastAddr::new(net_mac!("aa:bb:cc:dd:ee:ff")).expect("MAC is unicast"),
                max_frame_size: MaxEthernetFrameSize::from_mtu(Mtu::new(1500)).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        );

    {
        let device = ethernet_device.clone().into();
        // Enable the device, turning on a bunch of features that install
        // timers.
        let ip_config = IpDeviceConfigurationUpdate {
            ip_enabled: Some(true),
            gmp_enabled: Some(true),
            ..Default::default()
        };
        let _: Ipv4DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv4>()
            .update_configuration(&device, ip_config.into())
            .unwrap();
        let _: Ipv6DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv6>()
            .update_configuration(
                &device,
                Ipv6DeviceConfigurationUpdate {
                    max_router_solicitations: Some(Some(NonZeroU8::new(2).unwrap())),
                    slaac_config: SlaacConfigurationUpdate {
                        stable_address_configuration: Some(
                            StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                        ),
                        ..Default::default()
                    },
                    ip_config,
                    ..Default::default()
                },
            )
            .unwrap();
    }

    ctx.core_api().device().remove_device(ethernet_device).into_removed();
    assert_eq!(ctx.bindings_ctx.timer_ctx().timers(), &[]);
}

fn add_ethernet(ctx: &mut FakeCtx) -> DeviceId<FakeBindingsCtx> {
    ctx.core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: Ipv6::TEST_ADDRS.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into()
}

fn add_loopback(ctx: &mut FakeCtx) -> DeviceId<FakeBindingsCtx> {
    let device = ctx
        .core_api()
        .device::<LoopbackDevice>()
        .add_device_with_default_state(
            LoopbackCreationProperties { mtu: Ipv6::MINIMUM_LINK_MTU },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(
            &device,
            AddrSubnet::from_witness(Ipv6::LOOPBACK_ADDRESS, Ipv6::LOOPBACK_SUBNET.prefix())
                .unwrap(),
        )
        .unwrap();
    device
}

fn check_transmitted_ethernet(
    bindings_ctx: &mut FakeBindingsCtx,
    _device_id: &DeviceId<FakeBindingsCtx>,
    count: usize,
) {
    assert_eq!(bindings_ctx.take_ethernet_frames().len(), count);
}

fn check_transmitted_loopback(
    bindings_ctx: &mut FakeBindingsCtx,
    device_id: &DeviceId<FakeBindingsCtx>,
    count: usize,
) {
    // Loopback frames leave the stack; outgoing frames land in
    // its RX queue.
    let rx_available = core::mem::take(&mut bindings_ctx.state_mut().rx_available);
    if count == 0 {
        assert_eq!(rx_available, <[LoopbackDeviceId::<_>; 0]>::default());
    } else {
        assert_eq!(
            rx_available.into_iter().map(DeviceId::Loopback).collect::<Vec<_>>(),
            [device_id.clone()]
        );
    }
}

#[test_case(add_ethernet, check_transmitted_ethernet, true; "ethernet with queue")]
#[test_case(add_ethernet, check_transmitted_ethernet, false; "ethernet without queue")]
#[test_case(add_loopback, check_transmitted_loopback, false; "loopback without queue")]
fn tx_queue(
    add_device: fn(&mut FakeCtx) -> DeviceId<FakeBindingsCtx>,
    check_transmitted: fn(&mut FakeBindingsCtx, &DeviceId<FakeBindingsCtx>, usize),
    with_tx_queue: bool,
) {
    let mut ctx = FakeCtx::default();
    let device = add_device(&mut ctx);

    if with_tx_queue {
        match &device {
            DeviceId::Loopback(device) => {
                ctx.core_api().transmit_queue::<LoopbackDevice>().set_configuration(
                    device,
                    netstack3_core::device::TransmitQueueConfiguration::Fifo,
                );
            }
            DeviceId::PureIp(device) => {
                ctx.core_api().transmit_queue::<PureIpDevice>().set_configuration(
                    device,
                    netstack3_core::device::TransmitQueueConfiguration::Fifo,
                );
            }
            DeviceId::Ethernet(device) => {
                ctx.core_api().transmit_queue::<EthernetLinkDevice>().set_configuration(
                    device,
                    netstack3_core::device::TransmitQueueConfiguration::Fifo,
                );
            }
            DeviceId::Blackhole(_device) => {
                // Blackhole devices do not support transmission.
            }
        }
    }

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                // Enable stable addresses so the link-local address is auto-
                // generated.
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    // Enable DAD so that the auto-generated address triggers a DAD
                    // message immediately on interface enable.
                    dad_transmits: Some(Some(NonZeroU16::new(1).unwrap())),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    if with_tx_queue {
        check_transmitted(&mut ctx.bindings_ctx, &device, 0);
        assert_eq!(
            core::mem::take(&mut ctx.bindings_ctx.state_mut().tx_available),
            [device.clone()]
        );
        let ethernet_id = assert_matches!(&device, DeviceId::Ethernet(e) => e);
        let result = ctx.core_api().transmit_queue::<EthernetLinkDevice>().transmit_queued_frames(
            &ethernet_id,
            Default::default(),
            &mut (),
        );
        assert_eq!(result, Ok(WorkQueueReport::AllDone));
    }

    check_transmitted(&mut ctx.bindings_ctx, &device, 1);
    assert_eq!(ctx.bindings_ctx.state_mut().tx_available, <[DeviceId::<_>; 0]>::default());
    for_any_device_id!(
        DeviceId,
        device,
        device => ctx.core_api().device().remove_device(device).into_removed()
    )
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
fn test_add_remove_ip_addresses<I: Ip + TestIpExt + IpExt>(
    addr_config: Option<I::ManualAddressConfig<FakeInstant>>,
) {
    let config = I::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    ctx.test_api().enable_device(&device);

    let ip = I::get_other_ip_address(1).get();
    let prefix = config.subnet.prefix();
    let addr_subnet = AddrSubnet::new(ip, prefix).unwrap();

    let check_contains_addr = |ctx: &mut FakeCtx| {
        ctx.core_api().device_ip::<I>().get_assigned_ip_addr_subnets(&device).contains(&addr_subnet)
    };

    // IP doesn't exist initially.
    assert_eq!(check_contains_addr(&mut ctx), false);

    // Add IP (OK).
    ctx.core_api()
        .device_ip::<I>()
        .add_ip_addr_subnet_with_config(&device, addr_subnet, addr_config.unwrap_or_default())
        .unwrap();
    assert_eq!(check_contains_addr(&mut ctx), true);

    // Add IP again (already exists).
    assert_eq!(
        ctx.core_api().device_ip::<I>().add_ip_addr_subnet(&device, addr_subnet),
        Err(AddIpAddrSubnetError::Exists),
    );
    assert_eq!(check_contains_addr(&mut ctx), true);

    // Add IP with different subnet (already exists).
    let wrong_addr_subnet = AddrSubnet::new(ip, prefix - 1).unwrap();
    assert_eq!(
        ctx.core_api().device_ip::<I>().add_ip_addr_subnet(&device, wrong_addr_subnet),
        Err(AddIpAddrSubnetError::Exists),
    );
    assert_eq!(check_contains_addr(&mut ctx), true);

    let ip = SpecifiedAddr::new(ip).unwrap();
    // Del IP (ok).
    let removed = ctx.core_api().device_ip::<I>().del_ip_addr(&device, ip).unwrap().into_removed();
    assert_eq!(removed, addr_subnet);
    assert_eq!(check_contains_addr(&mut ctx), false);

    // Del IP again (not found).
    assert_matches!(ctx.core_api().device_ip::<I>().del_ip_addr(&device, ip), Err(NotFoundError));

    assert_eq!(check_contains_addr(&mut ctx), false);
}

#[test_case(None; "with no AddressConfig specified")]
#[test_case(Some(Ipv4AddrConfig {
        config: CommonAddressConfig {should_perform_dad: None},
        properties: CommonAddressProperties {
            valid_until: Lifetime::Finite(FakeInstant::from(Duration::from_secs(1))),
            preferred_lifetime: PreferredLifetime::preferred_until(
                FakeInstant::from(Duration::from_secs(1))
            )
        },
    }); "with AddressConfig specified")]
fn test_add_remove_ipv4_addresses(addr_config: Option<Ipv4AddrConfig<FakeInstant>>) {
    test_add_remove_ip_addresses::<Ipv4>(addr_config);
}

#[test_case(None; "with no AddressConfig specified")]
#[test_case(Some(Ipv6AddrManualConfig {
        config: CommonAddressConfig {should_perform_dad: None},
        properties: CommonAddressProperties {
            valid_until: Lifetime::Finite(FakeInstant::from(Duration::from_secs(2))),
            preferred_lifetime: PreferredLifetime::preferred_until(
                FakeInstant::from(Duration::from_secs(1))
            )
        },
        temporary: false
    }); "with AddressConfig specified")]
fn test_add_remove_ipv6_addresses(addr_config: Option<Ipv6AddrManualConfig<FakeInstant>>) {
    test_add_remove_ip_addresses::<Ipv6>(addr_config);
}
