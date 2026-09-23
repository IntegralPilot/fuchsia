// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net_stack::{
    self as fidl_net_stack, ForwardingEntry, StackRequest, StackRequestStream,
};
use futures::{TryFutureExt as _, TryStreamExt as _};
use log::{debug, error};
use net_types::SpecifiedAddr;
use net_types::ip::{Ip, IpAddr, Ipv4, Ipv6};
use netstack3_core::device::DeviceId;
use netstack3_core::routes::{AddableEntry, AddableEntryEither};

use super::util::{IntoCore as _, ResultExt as _, TryFromFidlWithContext as _, TryIntoCore as _};
use super::{BindingId, Ctx, routes};

pub(crate) struct StackFidlWorker {
    netstack: crate::bindings::Netstack,
}

impl StackFidlWorker {
    pub(crate) async fn serve(
        netstack: crate::bindings::Netstack,
        stream: StackRequestStream,
    ) -> Result<(), fidl::Error> {
        stream
            .try_fold(Self { netstack }, |mut worker, req| async {
                match req {
                    StackRequest::AddForwardingEntry { entry, responder } => {
                        responder
                            .send(worker.fidl_add_forwarding_entry(entry).await)
                            .unwrap_or_log("failed to respond");
                    }
                    StackRequest::DelForwardingEntry { entry, responder } => {
                        responder
                            .send(worker.fidl_del_forwarding_entry(entry).await)
                            .unwrap_or_log("failed to respond");
                    }
                    StackRequest::SetDhcpClientEnabled { responder, id: _, enable } => {
                        // TODO(https://fxbug.dev/42162065): Remove this once
                        // DHCPv4 client is implemented out-of-stack.
                        if enable {
                            error!(
                                "TODO(https://fxbug.dev/42062356): Support starting DHCP client"
                            );
                        }
                        responder.send(Ok(())).unwrap_or_log("failed to respond");
                    }
                    StackRequest::BridgeInterfaces { interfaces: _, bridge, control_handle: _ } => {
                        error!("bridging is not supported in netstack3");
                        bridge
                            .close_with_epitaph(zx::Status::NOT_SUPPORTED)
                            .unwrap_or_else(|e| debug!("failed to close bridge control {:?}", e));
                    }
                }
                Ok(worker)
            })
            .map_ok(|Self { netstack: _ }| ())
            .await
    }

    async fn fidl_add_forwarding_entry(
        &mut self,
        entry: ForwardingEntry,
    ) -> Result<(), fidl_net_stack::Error> {
        let bindings_ctx = self.netstack.ctx.bindings_ctx();
        let entry = match AddableEntryEither::try_from_fidl_with_ctx(bindings_ctx, entry) {
            Ok(entry) => entry,
            Err(e) => return Err(e.into()),
        };

        type DeviceId = netstack3_core::device::DeviceId<crate::bindings::BindingsCtx>;
        fn try_to_addable_entry<I: Ip>(
            ctx: &mut Ctx,
            entry: AddableEntry<I::Addr, Option<DeviceId>>,
        ) -> Option<AddableEntry<I::Addr, DeviceId>> {
            let AddableEntry { subnet, device, gateway, metric, route_preference } = entry;
            let (device, gateway) = match (device, gateway) {
                (Some(device), gateway) => (device, gateway),
                (None, gateway) => {
                    let gateway = gateway?;
                    let device =
                        ctx.api().routes_any().select_device_for_gateway(gateway.into())?;
                    (device, Some(gateway))
                }
            };
            Some(AddableEntry { subnet, device, gateway, metric, route_preference })
        }

        let entry = match entry {
            AddableEntryEither::V4(entry) => {
                try_to_addable_entry::<Ipv4>(&mut self.netstack.ctx, entry)
                    .ok_or(fidl_net_stack::Error::BadState)?
                    .map_device_id(|d| d.downgrade())
                    .into()
            }
            AddableEntryEither::V6(entry) => {
                try_to_addable_entry::<Ipv6>(&mut self.netstack.ctx, entry)
                    .ok_or(fidl_net_stack::Error::BadState)?
                    .map_device_id(|d| d.downgrade())
                    .into()
            }
        };

        self.netstack
            .ctx
            .bindings_ctx()
            .apply_route_change_either(routes::ChangeEither::global_add(entry))
            .await
            .map_err(|err| match err {
                routes::ChangeError::DeviceRemoved => fidl_net_stack::Error::InvalidArgs,
                routes::ChangeError::TableRemoved => panic!(
                    "can't apply route change because route change runner has been shut down"
                ),
                routes::ChangeError::SetRemoved => {
                    unreachable!("fuchsia.net.stack only uses the global route set")
                }
            })
            .and_then(|outcome| match outcome {
                routes::ChangeOutcome::NoChange => Err(fidl_net_stack::Error::AlreadyExists),
                routes::ChangeOutcome::Changed => Ok(()),
            })
    }

    async fn fidl_del_forwarding_entry(
        &mut self,
        fidl_net_stack::ForwardingEntry {
            subnet,
            device_id,
            next_hop,
            metric: _,
        }: fidl_net_stack::ForwardingEntry,
    ) -> Result<(), fidl_net_stack::Error> {
        let bindings_ctx = self.netstack.ctx.bindings_ctx();
        // There are no routes that have a device ID of 0. Instead of returning an error,
        // return success because no routes will be removed similar to netstack2.
        let Ok(binding_id) = BindingId::try_from(device_id) else {
            return Ok(());
        };
        let device = DeviceId::try_from_fidl_with_ctx(bindings_ctx, binding_id)
            .map_err(|_| fidl_net_stack::Error::InvalidArgs)?;
        let subnet = subnet.try_into_core().map_err(|_| fidl_net_stack::Error::InvalidArgs)?;
        let next_hop = next_hop.map(|nh| (*nh).into_core());
        // This API ignores metric for matching.
        let metric = routes::Matcher::Any;

        let route_change = match subnet {
            net_types::ip::SubnetEither::V4(subnet) => {
                let gateway = match next_hop {
                    Some(IpAddr::V4(addr)) => routes::Matcher::Exact(SpecifiedAddr::new(addr)),
                    Some(IpAddr::V6(_)) => return Err(fidl_net_stack::Error::InvalidArgs),
                    None => routes::Matcher::Any,
                };
                let device = device.downgrade();
                routes::Change::<Ipv4>::RouteOp(
                    routes::RouteOp::RemoveMatching { subnet, device, gateway, metric },
                    routes::SetMembership::Global,
                )
                .into()
            }
            net_types::ip::SubnetEither::V6(subnet) => {
                let gateway = match next_hop {
                    Some(IpAddr::V4(_)) => return Err(fidl_net_stack::Error::InvalidArgs),
                    Some(IpAddr::V6(addr)) => routes::Matcher::Exact(SpecifiedAddr::new(addr)),
                    None => routes::Matcher::Any,
                };
                let device = device.downgrade();
                routes::Change::<Ipv6>::RouteOp(
                    routes::RouteOp::RemoveMatching { subnet, device, gateway, metric },
                    routes::SetMembership::Global,
                )
                .into()
            }
        };
        bindings_ctx
            .apply_route_change_either(route_change)
            .await
            .map_err(|err| match err {
                routes::ChangeError::DeviceRemoved => fidl_net_stack::Error::InvalidArgs,
                routes::ChangeError::TableRemoved => panic!(
                    "can't apply route change because route change runner has been shut down"
                ),
                super::routes::ChangeError::SetRemoved => {
                    unreachable!("fuchsia.net.stack only uses the global route set")
                }
            })
            .and_then(|outcome| match outcome {
                routes::ChangeOutcome::NoChange => Err(fidl_net_stack::Error::NotFound),
                routes::ChangeOutcome::Changed => Ok(()),
            })
    }
}
