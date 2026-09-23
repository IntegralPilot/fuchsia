// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A Netstack3 worker to serve fuchsia.net.root.Interfaces API requests.

use async_utils::channel::TrySend as _;
use fidl::endpoints::{ProtocolMarker as _, ServerEnd};
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_root as fnet_root;
use fuchsia_async as fasync;
use futures::TryStreamExt as _;
use log::debug;
use net_types::ip::{Ipv4, Ipv6};

use crate::bindings::devices::{BindingId, DeviceSpecificInfo, LOOPBACK_MAC};
use crate::bindings::routes::admin::{GlobalRouteSet, serve_route_set};
use crate::bindings::util::{IntoFidl as _, ResultExt as _, ScopeExt as _};
use crate::bindings::{DeviceIdExt as _, Netstack, interfaces_admin};

// Serve a stream of fuchsia.net.root.Interfaces API requests for a single
// channel (e.g. a single client connection).
pub(crate) async fn serve_interfaces(
    ns: Netstack,
    rs: fnet_root::InterfacesRequestStream,
) -> Result<(), fidl::Error> {
    debug!("serving {}", fnet_root::InterfacesMarker::DEBUG_NAME);
    rs.try_for_each(|req| async {
        match req {
            fnet_root::InterfacesRequest::GetAdmin { id, control, control_handle: _ } => {
                handle_get_admin(&ns, id, control).await;
            }
            fnet_root::InterfacesRequest::GetMac { id, responder } => {
                responder
                    .send(handle_get_mac(&ns, id).as_ref().map(Option::as_deref).map_err(|e| *e))
                    .unwrap_or_log("failed to respond");
            }
        }
        Ok(())
    })
    .await
}

async fn handle_get_admin(
    ns: &Netstack,
    interface_id: u64,
    control: ServerEnd<fnet_interfaces_admin::ControlMarker>,
) {
    debug!("handling fuchsia.net.root.Interfaces::GetAdmin for {interface_id}");
    let core_id =
        BindingId::new(interface_id).and_then(|id| ns.ctx.bindings_ctx().devices.get_core_id(id));
    let core_id = match core_id {
        Some(c) => c,
        None => {
            control
                .close_with_epitaph(zx::Status::NOT_FOUND)
                .unwrap_or_log("failed to send epitaph");
            return;
        }
    };

    let mut sender = core_id.external_state().with_common_info(|i| i.control_hook.clone());

    match sender.try_send_fut(interfaces_admin::OwnedControlHandle::new_unowned(control)).await {
        Ok(()) => {}
        Err(owned_control_handle) => {
            owned_control_handle.into_control_handle().shutdown_with_epitaph(zx::Status::NOT_FOUND)
        }
    }
}

fn handle_get_mac(ns: &Netstack, interface_id: u64) -> fnet_root::InterfacesGetMacResult {
    debug!("handling fuchsia.net.root.Interfaces::GetMac for {interface_id}");
    BindingId::new(interface_id)
        .and_then(|id| ns.ctx.bindings_ctx().devices.get_core_id(id))
        .ok_or(fnet_root::InterfacesGetMacError::NotFound)
        .map(|core_id| {
            let mac = match core_id.external_state() {
                DeviceSpecificInfo::Loopback(_) => Some(LOOPBACK_MAC),
                DeviceSpecificInfo::Ethernet(info) => Some(info.mac.into()),
                DeviceSpecificInfo::PureIp(_) => None,
                DeviceSpecificInfo::Blackhole(_) => None,
            };
            mac.map(|mac| Box::new(mac.into_fidl()))
        })
}

pub(crate) async fn serve_routes_v4(
    mut rs: fnet_root::RoutesV4RequestStream,
    ctx: crate::bindings::Ctx,
) -> Result<(), fidl::Error> {
    while let Some(req) = rs.try_next().await? {
        match req {
            fnet_root::RoutesV4Request::GlobalRouteSet { route_set, control_handle: _ } => {
                let stream = route_set.into_stream();
                let ctx = ctx.clone();
                fasync::Scope::current().spawn_request_stream_handler(stream, |stream| {
                    async move {
                        serve_route_set::<Ipv4, _, _>(
                            stream,
                            &mut GlobalRouteSet::<Ipv4>::new(&ctx),
                            std::future::pending(), /* never cancelled */
                            &ctx,
                        )
                        .await
                    }
                });
            }
        }
    }

    Ok(())
}

pub(crate) async fn serve_routes_v6(
    mut rs: fnet_root::RoutesV6RequestStream,
    ctx: crate::bindings::Ctx,
) -> Result<(), fidl::Error> {
    while let Some(req) = rs.try_next().await? {
        match req {
            fnet_root::RoutesV6Request::GlobalRouteSet { route_set, control_handle: _ } => {
                let stream = route_set.into_stream();
                let ctx = ctx.clone();
                fasync::Scope::current().spawn_request_stream_handler(stream, |stream| {
                    async move {
                        serve_route_set::<Ipv6, _, _>(
                            stream,
                            &mut GlobalRouteSet::<Ipv6>::new(&ctx),
                            std::future::pending(), /* never cancelled */
                            &ctx,
                        )
                        .await
                    }
                });
            }
        }
    }

    Ok(())
}
