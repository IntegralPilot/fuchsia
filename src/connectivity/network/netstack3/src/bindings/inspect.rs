// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Inspect utilities.
//!
//! This module provides utilities for publishing netstack3 diagnostics data to
//! Inspect.

use std::string::ToString as _;

use diagnostics_traits::{
    FuchsiaInspector, InspectorDeviceIdProvider, InspectorRouteTableIdProvider,
};
use fuchsia_inspect::Node;
use net_types::ethernet::Mac;
use net_types::ip::{Ip, Ipv4, Ipv6};
use net_types::{UnicastAddr, Witness as _};
use netstack3_core::device::{DeviceId, EthernetLinkDevice, WeakDeviceId};
use netstack3_core::inspect::Inspector as _;
use netstack3_core::routes::{RoutingTableCookie, RoutingTableId};

use crate::bindings::devices::{
    DeviceIdAndName, DeviceSpecificInfo, DynamicCommonInfo, DynamicNetdeviceInfo, EthernetInfo,
};
use crate::bindings::routes::main_table_id;
use crate::bindings::{BindingsCtx, Ctx, DeviceIdExt as _};

/// An opaque struct that encapsulates the process of starting the Inspect
/// publisher task.
///
/// In tests where Inspect data should not be published, set `should_publish` to
/// false.
pub struct InspectPublisher<'a> {
    inspector: &'a fuchsia_inspect::Inspector,
    should_publish: bool,
}

impl<'a> InspectPublisher<'a> {
    pub(crate) fn new() -> Self {
        Self { inspector: fuchsia_inspect::component::inspector(), should_publish: true }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(inspector: &'a fuchsia_inspect::Inspector) -> Self {
        Self { inspector, should_publish: false }
    }

    pub(crate) fn inspector(&self) -> &'a fuchsia_inspect::Inspector {
        self.inspector
    }

    pub(crate) fn publish(self) -> Option<inspect_runtime::PublishedInspectController> {
        self.should_publish.then(|| {
            inspect_runtime::publish(self.inspector, inspect_runtime::PublishOptions::default())
                .expect("publish Inspect task")
        })
    }
}

impl InspectorDeviceIdProvider<DeviceId<BindingsCtx>> for BindingsCtx {
    fn device_id(id: &DeviceId<BindingsCtx>) -> u64 {
        id.bindings_id().id.into()
    }
}

impl InspectorDeviceIdProvider<WeakDeviceId<BindingsCtx>> for BindingsCtx {
    fn device_id(id: &WeakDeviceId<BindingsCtx>) -> u64 {
        id.bindings_id().id.into()
    }
}

impl<I: Ip> InspectorRouteTableIdProvider<RoutingTableId<I, DeviceId<BindingsCtx>, BindingsCtx>>
    for BindingsCtx
{
    fn route_table_id(id: &RoutingTableId<I, DeviceId<BindingsCtx>, BindingsCtx>) -> u32 {
        match id.bindings_id() {
            RoutingTableCookie::Main => main_table_id::<I>().into(),
            RoutingTableCookie::BindingsId(id) => *id,
        }
    }
}

/// Publishes netstack3 socket diagnostics data to Inspect.
pub(crate) fn sockets(ctx: &mut Ctx) -> fuchsia_inspect::Inspector {
    let inspector = fuchsia_inspect::Inspector::new(Default::default());
    let mut bindings_inspector = FuchsiaInspector::<BindingsCtx>::new(inspector.root());
    ctx.api().tcp::<Ipv4>().inspect(&mut bindings_inspector);
    ctx.api().tcp::<Ipv6>().inspect(&mut bindings_inspector);
    ctx.api().udp::<Ipv4>().inspect(&mut bindings_inspector);
    ctx.api().udp::<Ipv6>().inspect(&mut bindings_inspector);
    ctx.api().icmp_echo::<Ipv4>().inspect(&mut bindings_inspector);
    ctx.api().icmp_echo::<Ipv6>().inspect(&mut bindings_inspector);
    ctx.api().raw_ip_socket::<Ipv4>().inspect(&mut bindings_inspector);
    ctx.api().raw_ip_socket::<Ipv6>().inspect(&mut bindings_inspector);
    ctx.api().device_socket().inspect(&mut bindings_inspector);
    inspector
}

/// Publishes netstack3 routing table diagnostics data to Inspect.
pub(crate) fn routes(ctx: &mut Ctx) -> fuchsia_inspect::Inspector {
    let inspector = fuchsia_inspect::Inspector::new(Default::default());
    let mut bindings_inspector = FuchsiaInspector::<BindingsCtx>::new(inspector.root());
    // Group inspect data according to the IP version so that it's easier to decode.
    bindings_inspector.record_child("Ipv4", |inspector| {
        ctx.api().routes::<Ipv4>().inspect(inspector);
    });
    bindings_inspector.record_child("Ipv6", |inspector| {
        ctx.api().routes::<Ipv6>().inspect(inspector);
    });
    inspector
}

/// Publishes netstack3 multicast forwarding diagnostics data to Inspect.
pub(crate) fn multicast_forwarding(ctx: &mut Ctx) -> fuchsia_inspect::Inspector {
    let inspector = fuchsia_inspect::Inspector::new(Default::default());
    let mut bindings_inspector = FuchsiaInspector::<BindingsCtx>::new(inspector.root());
    bindings_inspector.record_child("IPv4", |inspector| {
        ctx.api().multicast_forwarding::<Ipv4>().inspect(inspector);
    });
    bindings_inspector.record_child("IPv6", |inspector| {
        ctx.api().multicast_forwarding::<Ipv6>().inspect(inspector);
    });
    inspector
}

pub(crate) fn devices(ctx: &mut Ctx) -> fuchsia_inspect::Inspector {
    // Snapshot devices out so we're not holding onto the devices lock for too
    // long.
    let devices =
        ctx.bindings_ctx().devices.with_devices(|devices| devices.cloned().collect::<Vec<_>>());
    let inspector = fuchsia_inspect::Inspector::new(Default::default());
    let node = inspector.root();
    for device_id in devices {
        let external_state = device_id.external_state();
        let DeviceIdAndName { id: binding_id, name } = device_id.bindings_id();
        node.record_child(format!("{binding_id}"), |node| {
            node.record_string("Name", name);
            node.record_uint("InterfaceId", (*binding_id).into());
            node.record_child("IPv4", |node| {
                ctx.api()
                    .device_ip::<Ipv4>()
                    .inspect(&device_id, &mut FuchsiaInspector::<BindingsCtx>::new(node))
            });
            node.record_child("IPv6", |node| {
                ctx.api()
                    .device_ip::<Ipv6>()
                    .inspect(&device_id, &mut FuchsiaInspector::<BindingsCtx>::new(node))
            });
            external_state.with_common_info(
                |DynamicCommonInfo {
                     admin_enabled,
                     ipv4_enabled,
                     ipv6_enabled,
                     mtu,
                     addresses: _,
                     control_hook: _,
                     events: _,
                 }| {
                    node.record_bool("AdminEnabled", *admin_enabled);
                    node.record_bool("IPv4Enabled", *ipv4_enabled);
                    node.record_bool("IPv6Enabled", *ipv6_enabled);
                    node.record_uint("MTU", mtu.get().into());
                },
            );
            match external_state {
                DeviceSpecificInfo::Ethernet(info @ EthernetInfo { mac, .. }) => {
                    node.record_bool("Loopback", false);
                    info.with_dynamic_info(|dynamic| {
                        record_network_device(node, &dynamic.netdevice, Some(mac))
                    });
                }
                DeviceSpecificInfo::Blackhole(_info) => {
                    node.record_bool("Loopback", false);
                    node.record_child("Blackhole", |_node| {});
                }
                DeviceSpecificInfo::Loopback(_info) => {
                    node.record_bool("Loopback", true);
                }
                DeviceSpecificInfo::PureIp(info) => {
                    node.record_bool("Loopback", false);
                    info.with_dynamic_info(|dynamic| record_network_device(node, dynamic, None));
                }
            }
            ctx.api()
                .device_any()
                .inspect(&device_id, &mut FuchsiaInspector::<BindingsCtx>::new(node));
        })
    }
    inspector
}

/// Record information about the Netdevice as a child of the given inspect node.
fn record_network_device(
    node: &Node,
    DynamicNetdeviceInfo { phy_up, common_info: _ }: &DynamicNetdeviceInfo,
    mac: Option<&UnicastAddr<Mac>>,
) {
    node.record_child("NetworkDevice", |node| {
        if let Some(mac) = mac {
            node.record_string("MacAddress", mac.get().to_string());
        }
        node.record_bool("PhyUp", *phy_up);
    })
}

pub(crate) fn neighbors(mut ctx: Ctx) -> fuchsia_inspect::Inspector {
    let inspector = fuchsia_inspect::Inspector::new(Default::default());

    // Get a snapshot of all supported devices. Ethernet is the only device type
    // that supports neighbors.
    let ethernet_devices = ctx.bindings_ctx().devices.with_devices(|devices| {
        devices
            .filter_map(|d| match d {
                DeviceId::Ethernet(d) => Some(d.clone()),
                // NUD is not supported on Loopback or pure IP devices.
                DeviceId::Loopback(_) | DeviceId::PureIp(_) | DeviceId::Blackhole(_) => None,
            })
            .collect::<Vec<_>>()
    });
    for device in ethernet_devices {
        inspector.root().record_child(&device.bindings_id().name, |node| {
            let mut inspector = FuchsiaInspector::<BindingsCtx>::new(node);
            ctx.api()
                .neighbor::<Ipv4, EthernetLinkDevice>()
                .inspect_neighbors(&device, &mut inspector);
            ctx.api()
                .neighbor::<Ipv6, EthernetLinkDevice>()
                .inspect_neighbors(&device, &mut inspector);
        });
    }
    inspector
}

pub(crate) fn counters(ctx: &mut Ctx) -> fuchsia_inspect::Inspector {
    let inspector = fuchsia_inspect::Inspector::new(Default::default());
    let mut fuchsia_inspector = FuchsiaInspector::<BindingsCtx>::new(inspector.root());
    ctx.api().counters().inspect_stack_counters(&mut fuchsia_inspector);
    fuchsia_inspector.record_inspectable("Bindings", &ctx.bindings_ctx().counters);
    inspector
}

pub(crate) fn filtering_state(ctx: &mut Ctx) -> fuchsia_inspect::Inspector {
    let inspector = fuchsia_inspect::Inspector::new(Default::default());
    ctx.api().filter().inspect_state(&mut FuchsiaInspector::<BindingsCtx>::new(inspector.root()));
    inspector
}
