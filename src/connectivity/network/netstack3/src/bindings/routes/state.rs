// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FIDL Worker for the `fuchsia.net.routes` suite of protocols.

use std::fmt::Debug;

use either::Either;
use fidl::endpoints::ProtocolMarker;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fuchsia_async as fasync;
use futures::channel::oneshot;
use futures::{TryStream, TryStreamExt as _};
use log::{error, info, warn};
use net_types::ethernet::Mac;
use net_types::ip::{GenericOverIp, Ip, IpAddr, IpAddress, Ipv4, Ipv6};
use net_types::{SpecifiedAddr, UnicastAddr, Witness as _};
use netstack3_core::device::{DeviceId, EthernetDeviceId, EthernetLinkDevice};
use netstack3_core::error::AddressResolutionFailed;
use netstack3_core::ip::WrapBroadcastMarker;
use netstack3_core::neighbor::{LinkResolutionContext, LinkResolutionResult};
use netstack3_core::routes::{NextHop, ResolvedRoute};

use crate::bindings::routes::rules_state::RuleInterest;
use crate::bindings::routes::watcher::{
    FidlWatcherEvent, ServeWatcherError, Update, UpdateDispatcher, WatcherEvent, WatcherInterest,
    serve_watcher,
};
use crate::bindings::util::{
    ConversionContext as _, IntoCore as _, IntoFidl as _, ResultExt as _, ScopeExt as _,
};
use crate::bindings::{BindingsCtx, Ctx, IpExt, routes};

impl LinkResolutionContext<EthernetLinkDevice> for BindingsCtx {
    type Notifier = LinkResolutionNotifier;
}

#[derive(Debug)]
pub(crate) struct LinkResolutionNotifier(
    oneshot::Sender<Result<UnicastAddr<Mac>, AddressResolutionFailed>>,
);

impl netstack3_core::neighbor::LinkResolutionNotifier<EthernetLinkDevice>
    for LinkResolutionNotifier
{
    type Observer = oneshot::Receiver<Result<UnicastAddr<Mac>, AddressResolutionFailed>>;

    fn new() -> (Self, Self::Observer) {
        let (tx, rx) = oneshot::channel();
        (Self(tx), rx)
    }

    fn notify(self, result: Result<UnicastAddr<Mac>, AddressResolutionFailed>) {
        let Self(tx) = self;
        tx.send(result).unwrap_or_else(|_| {
            error!("link address observer was dropped before resolution completed")
        });
    }
}

/// Serve the `fuchsia.net.routes/State` protocol.
pub(crate) async fn serve_state(
    rs: fnet_routes::StateRequestStream,
    ctx: Ctx,
) -> Result<(), fidl::Error> {
    rs.try_for_each_concurrent(None, |req| async {
        match req {
            fnet_routes::StateRequest::Resolve { destination, responder } => {
                let result =
                    resolve(ctx.clone(), destination, fnet_routes_ext::ResolveOptions::default())
                        .await;
                responder
                    .send(
                        result
                            .map(|r| match r {
                                fnet_routes::ResolveResult::Direct(d) => {
                                    fnet_routes::Resolved::Direct(d)
                                }
                                fnet_routes::ResolveResult::Gateway(g) => {
                                    fnet_routes::Resolved::Gateway(g)
                                }
                                r @ fnet_routes::ResolveResult::__SourceBreaking { .. } => {
                                    unreachable!("unknown resolve result: {:?}", r)
                                }
                            })
                            .as_ref()
                            .map_err(|err| match err {
                                fnet_routes::ResolveError::AddressUnreachable => {
                                    zx::Status::ADDRESS_UNREACHABLE.into_raw()
                                }
                                err @ fnet_routes::ResolveError::__SourceBreaking { .. } => {
                                    unreachable!("impossible error for resolve {:?}", err)
                                }
                            }),
                    )
                    .unwrap_or_log("failed to respond");
                Ok(())
            }
            fnet_routes::StateRequest::Resolve2 { destination, options, responder } => {
                let result = resolve(ctx.clone(), destination, options.into()).await;
                responder
                    .send(result.as_ref().map_err(Clone::clone))
                    .unwrap_or_log("failed to respond");
                Ok(())
            }
            fnet_routes::StateRequest::GetRouteTableName { table_id, responder } => {
                match routes::TableIdEither::new(table_id) {
                    routes::TableIdEither::V4(id) => {
                        ctx.bindings_ctx().get_route_table_name(id, responder)
                    }
                    routes::TableIdEither::V6(id) => {
                        ctx.bindings_ctx().get_route_table_name(id, responder)
                    }
                }
                Ok(())
            }
        }
    })
    .await
}

/// Resolves the route to the given destination address.
///
/// Returns `Err` if the destination can't be resolved.
async fn resolve(
    ctx: Ctx,
    destination: fnet::IpAddress,
    options: fnet_routes_ext::ResolveOptions,
) -> Result<fnet_routes::ResolveResult, fnet_routes::ResolveError> {
    let addr: IpAddr = destination.into_core();
    match addr {
        IpAddr::V4(addr) => resolve_inner(ctx, addr, options).await,
        IpAddr::V6(addr) => resolve_inner(ctx, addr, options).await,
    }
}

/// The inner implementation of [`resolve`] that's generic over `Ip`.
#[netstack3_core::context_ip_bounds(A::Version, BindingsCtx)]
async fn resolve_inner<A: IpAddress>(
    mut ctx: Ctx,
    destination: A,
    options: fnet_routes_ext::ResolveOptions,
) -> Result<fnet_routes::ResolveResult, fnet_routes::ResolveError>
where
    A::Version: IpExt,
{
    let sanitized_dst = SpecifiedAddr::new(destination)
        .map(|dst| {
            netstack3_core::routes::RoutableIpAddr::try_from(dst).map_err(
                |netstack3_core::socket::AddrIsMappedError {}| {
                    fnet_routes::ResolveError::AddressUnreachable
                },
            )
        })
        .transpose()?;
    let core_options = options.into_core();
    let ResolvedRoute {
        device,
        src_addr,
        local_delivery_device: _,
        next_hop,
        internal_forwarding: _,
    } = match ctx.api().routes::<A::Version>().resolve_route(sanitized_dst, &core_options) {
        Err(e) => {
            info!("Resolve failed for {} ({:?}), {:?}", destination, core_options, e);
            return Err(fnet_routes::ResolveError::AddressUnreachable);
        }
        Ok(resolved_route) => resolved_route,
    };
    let (next_hop_addr, next_hop_type) = match next_hop {
        NextHop::RemoteAsNeighbor => {
            (SpecifiedAddr::new(destination), Either::Left(fnet_routes::ResolveResult::Direct))
        }
        NextHop::Broadcast(marker) => {
            <A::Version as Ip>::map_ip::<_, ()>(
                WrapBroadcastMarker(marker),
                |WrapBroadcastMarker(())| (),
                |WrapBroadcastMarker(never)| match never {},
            );
            (SpecifiedAddr::new(destination), Either::Left(fnet_routes::ResolveResult::Direct))
        }
        NextHop::Gateway(gateway) => {
            (Some(gateway), Either::Right(fnet_routes::ResolveResult::Gateway))
        }
    };
    let remote_mac = match &device {
        DeviceId::Loopback(_device) => None,
        DeviceId::Blackhole(_device) => None,
        DeviceId::Ethernet(device) => {
            if let Some(addr) = next_hop_addr {
                Some(resolve_ethernet_link_addr(&mut ctx, device, &addr).await?)
            } else {
                warn!("Cannot attempt Ethernet link resolution for the unspecified address.");
                return Err(fnet_routes::ResolveError::AddressUnreachable);
            }
        }
        DeviceId::PureIp(_device) => None,
    };

    let destination = {
        let address =
            next_hop_addr.map_or(A::Version::UNSPECIFIED_ADDRESS, |a| *a).to_ip_addr().into_fidl();
        let source_address = src_addr.addr().to_ip_addr().into_fidl();
        let mac = remote_mac.map(|mac| mac.get().into_fidl());
        let interface_id = ctx.bindings_ctx().get_binding_id(device);
        fnet_routes::Destination {
            address: Some(address),
            mac,
            interface_id: Some(interface_id.get()),
            source_address: Some(source_address),
            ..Default::default()
        }
    };

    Ok(either::for_both!(next_hop_type, f => f(destination)))
}

/// Performs link-layer resolution of the remote IP Address on the given device.
#[netstack3_core::context_ip_bounds(A::Version, BindingsCtx)]
async fn resolve_ethernet_link_addr<A: IpAddress>(
    ctx: &mut Ctx,
    device: &EthernetDeviceId<BindingsCtx>,
    remote: &SpecifiedAddr<A>,
) -> Result<UnicastAddr<Mac>, fnet_routes::ResolveError>
where
    A::Version: IpExt,
{
    match ctx.api().neighbor::<A::Version, EthernetLinkDevice>().resolve_link_addr(device, remote) {
        LinkResolutionResult::Resolved(mac) => Ok(mac),
        LinkResolutionResult::Pending(observer) => observer
            .await
            .expect("core must send link resolution result before dropping notifier")
            .map_err(|AddressResolutionFailed| fnet_routes::ResolveError::AddressUnreachable),
    }
}

/// Serve the `fuchsia.net.routes/StateV4` protocol.
pub(crate) async fn serve_state_v4(
    mut rs: fnet_routes::StateV4RequestStream,
    dispatchers: routes::Dispatchers<Ipv4>,
) -> Result<(), fidl::Error> {
    let routes::Dispatchers { route_update_dispatcher, rule_update_dispatcher } = dispatchers;
    while let Some(req) = rs.try_next().await? {
        match req {
            fnet_routes::StateV4Request::GetWatcherV4 { options, watcher, control_handle: _ } => {
                fasync::Scope::current().spawn_server_end(watcher, |watcher| {
                    serve_route_watcher::<Ipv4>(
                        watcher,
                        options.into(),
                        route_update_dispatcher.clone(),
                    )
                })
            }
            fnet_routes::StateV4Request::GetRuleWatcherV4 {
                options:
                    fnet_routes::RuleWatcherOptionsV4 {
                        __source_breaking: fidl::marker::SourceBreaking,
                    },
                watcher,
                control_handle: _,
            } => fasync::Scope::current().spawn_server_end(watcher, |watcher| {
                serve_watcher::<fnet_routes_ext::rules::RuleEvent<Ipv4>, _>(
                    watcher,
                    RuleInterest,
                    rule_update_dispatcher.clone(),
                )
            }),
        }
    }
    Ok(())
}

/// Serve the `fuchsia.net.routes/StateV6` protocol.
pub(crate) async fn serve_state_v6(
    mut rs: fnet_routes::StateV6RequestStream,
    dispatchers: routes::Dispatchers<Ipv6>,
) -> Result<(), fidl::Error> {
    let routes::Dispatchers { route_update_dispatcher, rule_update_dispatcher } = dispatchers;
    while let Some(req) = rs.try_next().await? {
        match req {
            fnet_routes::StateV6Request::GetWatcherV6 { options, watcher, control_handle: _ } => {
                fasync::Scope::current().spawn_server_end(watcher, |watcher| {
                    serve_route_watcher::<Ipv6>(
                        watcher,
                        options.into(),
                        route_update_dispatcher.clone(),
                    )
                })
            }
            fnet_routes::StateV6Request::GetRuleWatcherV6 {
                options:
                    fnet_routes::RuleWatcherOptionsV6 {
                        __source_breaking: fidl::marker::SourceBreaking,
                    },
                watcher,
                control_handle: _,
            } => fasync::Scope::current().spawn_server_end(watcher, |watcher| {
                serve_watcher::<fnet_routes_ext::rules::RuleEvent<Ipv6>, _>(
                    watcher,
                    RuleInterest,
                    rule_update_dispatcher.clone(),
                )
            }),
        }
    }
    Ok(())
}

// Serve a single client of the `WatcherV4` or `WatcherV6` protocol.
async fn serve_route_watcher<I: fnet_routes_ext::FidlRouteIpExt>(
    server_end: fidl::endpoints::ServerEnd<I::WatcherMarker>,
    fnet_routes_ext::WatcherOptions { table_interest }: fnet_routes_ext::WatcherOptions,
    dispatcher: RouteUpdateDispatcher<I>,
) -> Result<(), ServeWatcherError> {
    let client_interest = match table_interest {
        Some(fnet_routes::TableInterest::Main(fnet_routes::Main)) => {
            RouteTableInterest::Only { table_id: routes::main_table_id::<I>().into() }
        }
        Some(fnet_routes::TableInterest::Only(table_id)) => {
            RouteTableInterest::Only { table_id: fnet_routes_ext::TableId::new(table_id) }
        }
        Some(fnet_routes::TableInterest::All(fnet_routes::All)) | None => RouteTableInterest::All,
        Some(fnet_routes::TableInterest::__SourceBreaking { unknown_ordinal }) => {
            return Err(ServeWatcherError::ErrorInStream(fidl::Error::UnknownOrdinal {
                ordinal: unknown_ordinal,
                protocol_name: <I::WatcherMarker as ProtocolMarker>::DEBUG_NAME,
            }));
        }
    };
    serve_watcher(server_end, client_interest, dispatcher).await
}

/// The tables the client is interested in.
#[derive(Debug)]
pub(crate) enum RouteTableInterest {
    /// The client is interested in updates across all tables.
    All,
    /// The client only want updates on the specific table.
    ///
    /// The table ID is a scalar instead of [`TableId`] because we don't perform
    /// validation but only filtering.
    Only { table_id: fnet_routes_ext::TableId },
}

impl RouteTableInterest {
    fn has_interest_in<I: Ip>(&self, event: &fnet_routes_ext::Event<I>) -> bool {
        match event {
            fnet_routes_ext::Event::Unknown | fnet_routes_ext::Event::Idle => true,
            fnet_routes_ext::Event::Existing(installed_route)
            | fnet_routes_ext::Event::Added(installed_route)
            | fnet_routes_ext::Event::Removed(installed_route) => match self {
                RouteTableInterest::All => true,
                RouteTableInterest::Only { table_id } => installed_route.table_id == *table_id,
            },
        }
    }
}

impl<I: Ip> WatcherInterest<fnet_routes_ext::Event<I>> for RouteTableInterest {
    fn has_interest_in(&self, event: &fnet_routes_ext::Event<I>) -> bool {
        self.has_interest_in(event)
    }
}

impl<I: Ip> WatcherEvent for fnet_routes_ext::Event<I> {
    type Resource = fnet_routes_ext::InstalledRoute<I>;

    const MAX_EVENTS: usize = fnet_routes::MAX_EVENTS as usize;

    const IDLE: Self = fnet_routes_ext::Event::Idle;

    fn existing(installed: Self::Resource) -> Self {
        Self::Existing(installed)
    }
}

impl<I: Ip> From<Update<fnet_routes_ext::Event<I>>> for fnet_routes_ext::Event<I> {
    fn from(update: Update<fnet_routes_ext::Event<I>>) -> Self {
        match update {
            Update::Added(added) => fnet_routes_ext::Event::Added(added),
            Update::Removed(removed) => fnet_routes_ext::Event::Removed(removed),
        }
    }
}

impl<I: fnet_routes_ext::FidlRouteIpExt> FidlWatcherEvent for fnet_routes_ext::Event<I> {
    type WatcherMarker = I::WatcherMarker;

    // Responds to a single `Watch` request with the given batch of events.
    fn respond_to_watch_request(
        req: fidl::endpoints::Request<Self::WatcherMarker>,
        events: Vec<Self>,
    ) -> Result<(), fidl::Error> {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Inputs<I: fnet_routes_ext::FidlRouteIpExt> {
            req: <<I::WatcherMarker as ProtocolMarker>::RequestStream as TryStream>::Ok,
            events: Vec<fnet_routes_ext::Event<I>>,
        }
        let result = I::map_ip_in::<Inputs<I>, _>(
            Inputs { req, events },
            |Inputs { req, events }| match req {
                fnet_routes::WatcherV4Request::Watch { responder } => {
                    let events = events
                        .into_iter()
                        .map(|event| {
                            event.try_into().unwrap_or_else(|e| match e {
                                fnet_routes_ext::NetTypeConversionError::UnknownUnionVariant(
                                    msg,
                                ) => {
                                    panic!(
                                        "tried to send an event with Unknown enum variant: {}",
                                        msg
                                    )
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    responder.send(&events)
                }
            },
            |Inputs { req, events }| match req {
                fnet_routes::WatcherV6Request::Watch { responder } => {
                    let events = events
                        .into_iter()
                        .map(|event| {
                            event.try_into().unwrap_or_else(|e| match e {
                                fnet_routes_ext::NetTypeConversionError::UnknownUnionVariant(
                                    msg,
                                ) => {
                                    panic!(
                                        "tried to send an event with Unknown enum variant: {}",
                                        msg
                                    )
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    responder.send(&events)
                }
            },
        );
        result
    }
}

pub(crate) type RouteUpdateDispatcher<I> =
    UpdateDispatcher<fnet_routes_ext::Event<I>, RouteTableInterest>;
