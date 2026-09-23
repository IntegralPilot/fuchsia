// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::{HashMap, VecDeque};

use fidl::endpoints::{ControlHandle as _, RequestStream as _, Responder as _};
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext::IntoExt;
use fidl_fuchsia_net_multicast_ext::FidlMulticastAdminIpExt;
use fidl_fuchsia_net_neighbor::{
    self as fnet_neighbor, ControllerError, ControllerRequest, ControllerRequestStream,
    ViewRequest, ViewRequestStream,
};
use fidl_fuchsia_net_neighbor_ext as fnet_neighbor_ext;

use assert_matches::assert_matches;
use futures::channel::mpsc;
use futures::task::Poll;
use futures::{Future, SinkExt as _, StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use log::{debug, error, info, warn};
use net_types::ethernet::Mac;
use net_types::ip::{Ip, IpAddr, IpAddress, Ipv4, Ipv6};
use net_types::{SpecifiedAddr, UnicastAddr, Witness as _};
use thiserror::Error;

use crate::bindings::devices::{BindingId, DeviceIdAndName};
use crate::bindings::time::StackTime;
use crate::bindings::util::{ErrorLogExt, IntoFidl};
use crate::bindings::{BindingsCtx, Ctx};
use netstack3_core::device::{
    DeviceId, EthernetDeviceId, EthernetLinkDevice, EthernetWeakDeviceId, WeakDeviceId,
};
use netstack3_core::error::NotFoundError;
use netstack3_core::neighbor::{
    NeighborRemovalError, StaticNeighborInsertionError, TriggerNeighborProbeError,
};
use netstack3_core::routes::Entry;
use netstack3_core::{IpExt, neighbor};

#[derive(Debug)]
pub(crate) struct Event {
    pub(crate) id: EthernetWeakDeviceId<BindingsCtx>,
    pub(crate) addr: SpecifiedAddr<IpAddr>,
    pub(crate) kind: neighbor::EventKind<Mac>,
    pub(crate) at: StackTime,
}

struct EventLogger<'a> {
    event: &'a Event,
    ctx: &'a Ctx,
}

// NB: By burying this logic in a Display impl, we ensure it only gets
// evaluated if the stack's log level is sufficient to actually log the message.
impl std::fmt::Display for EventLogger<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { event: Event { id, addr, kind, at }, ctx } = self;
        let dev = WeakDeviceId::Ethernet(id.clone());
        // NB: We need a mutable reference to ctx, but the `Display` trait
        // only gives us a shared reference. Clone the Ctx to work around this.
        // Fortunately Ctx clones are cheap because it's an `Arc`.
        let mut ctx = (*ctx).clone();
        let neighbor_type = match (*addr).into() {
            IpAddr::V4(addr) => get_neighbor_type::<Ipv4>(&addr, &dev, &mut ctx),
            IpAddr::V6(addr) => get_neighbor_type::<Ipv6>(&addr, &dev, &mut ctx),
        };
        let bindings_id = id.bindings_id();
        write!(f, "neighbor event {bindings_id:?} {addr} ({neighbor_type}) {kind:?} at {at}")
    }
}

// Additional debug info from the routing table about a particular neighbor.
//
// Note the order of the variants below is important: each later variant is
// considered an upgrade of an earlier variant.
#[derive(Debug, PartialEq, PartialOrd)]
enum NeighborType {
    // No additional information is known about this neighbor.
    Unknown,
    // The neighbor is directly connected (e.g. it's reachable via an onlink
    // route).
    Onlink,
    // The neighbor is a gateway (e.g. it's listed as the gateway on a route).
    Gateway,
    // The neighbor is a gateway for the Internet (e.g. it's listed as a gateway
    // on a default route).
    InternetGateway,
}

impl std::fmt::Display for NeighborType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(f, "Unknown"),
            Self::Onlink => write!(f, "Onlink"),
            Self::Gateway => write!(f, "Gateway"),
            Self::InternetGateway => write!(f, "Internet Gateway"),
        }
    }
}

impl NeighborType {
    fn maybe_upgrade(&mut self, new: NeighborType) {
        if &new > self {
            *self = new
        }
    }

    // Considers the given route, and modifies our current understanding
    // of this `NeighborType`.
    fn maybe_upgrade_with_route<I: Ip, D1, D2: PartialEq<D1>>(
        &mut self,
        addr: &SpecifiedAddr<I::Addr>,
        dev: &D1,
        route: &Entry<I::Addr, D2>,
    ) {
        let Entry { device, subnet, gateway, metric: _, route_preference: _ } = route;
        // NB: Ignore routes on different devices.
        if device != dev {
            return;
        }
        match gateway {
            None => {
                if subnet.contains(&addr.get()) {
                    self.maybe_upgrade(NeighborType::Onlink)
                }
            }
            Some(gateway) => {
                if gateway == addr {
                    // Is this a default route?
                    if subnet.prefix() == 0 {
                        self.maybe_upgrade(NeighborType::InternetGateway)
                    } else {
                        self.maybe_upgrade(NeighborType::Gateway)
                    }
                }
            }
        }
    }
}

// Identify the `NeighborType` by consulting the routing table.
#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
fn get_neighbor_type<I: IpExt + FidlMulticastAdminIpExt>(
    addr: &SpecifiedAddr<I::Addr>,
    dev: &WeakDeviceId<BindingsCtx>,
    ctx: &mut Ctx,
) -> NeighborType {
    let neighbor_type = ctx.api().routes::<I>().fold_routes(
        NeighborType::Unknown,
        |mut neighbor_type, _table_id, route| {
            neighbor_type.maybe_upgrade_with_route::<I, _, _>(addr, dev, route);
            neighbor_type
        },
    );
    neighbor_type
}

fn new_fidl_entry(
    binding_id: BindingId,
    addr: SpecifiedAddr<IpAddr>,
    state: neighbor::EventState<Mac>,
    at: StackTime,
) -> fnet_neighbor::Entry {
    let (state, mac) = match state {
        neighbor::EventState::Dynamic(dynamic_state) => match dynamic_state {
            neighbor::EventDynamicState::Incomplete => {
                (fnet_neighbor::EntryState::Incomplete, None)
            }
            neighbor::EventDynamicState::Reachable(mac) => {
                (fnet_neighbor::EntryState::Reachable, Some(mac.get()))
            }
            neighbor::EventDynamicState::Stale(mac) => {
                (fnet_neighbor::EntryState::Stale, Some(mac.get()))
            }
            neighbor::EventDynamicState::Delay(mac) => {
                (fnet_neighbor::EntryState::Delay, Some(mac.get()))
            }
            neighbor::EventDynamicState::Probe(mac) => {
                (fnet_neighbor::EntryState::Probe, Some(mac.get()))
            }
            neighbor::EventDynamicState::Unreachable(mac) => {
                (fnet_neighbor::EntryState::Unreachable, Some(mac.get()))
            }
        },
        neighbor::EventState::Static(mac) => (fnet_neighbor::EntryState::Static, Some(mac.get())),
    };
    fnet_neighbor_ext::Entry {
        interface: binding_id,
        neighbor: addr.get().into_ext(),
        state,
        mac: mac.map(IntoExt::into_ext),
        updated_at: at.into_fidl(),
    }
    .into()
}

fn get_link_layer_addr(state: &neighbor::EventState<Mac>) -> Option<UnicastAddr<Mac>> {
    match state {
        neighbor::EventState::Static(addr) => Some(*addr),
        neighbor::EventState::Dynamic(state) => match state {
            neighbor::EventDynamicState::Incomplete => None,
            neighbor::EventDynamicState::Reachable(addr) => Some(*addr),
            neighbor::EventDynamicState::Stale(addr) => Some(*addr),
            neighbor::EventDynamicState::Delay(addr) => Some(*addr),
            neighbor::EventDynamicState::Probe(addr) => Some(*addr),
            neighbor::EventDynamicState::Unreachable(addr) => Some(*addr),
        },
    }
}

/// Returns whether the event is "interesting" (e.g. worthy of any info log).
///
/// Add and remove events are automatically interesting. Changes are interesting
/// if the entry is not known to be reachable, or if the the entry's link layer
/// address changed.
fn is_interesting_event(event: &Event, old_state: Option<&NeighborState>) -> bool {
    match event.kind {
        neighbor::EventKind::Added(_) => true,
        neighbor::EventKind::Removed => true,
        neighbor::EventKind::Changed(new_state) => {
            let Some(old) = old_state else {
                panic!("neighbor changed but not found: {event:?}");
            };

            !old.previously_reachable
                || (get_link_layer_addr(&old.state) != get_link_layer_addr(&new_state))
        }
    }
}

pub(crate) struct Worker {
    event_receiver: mpsc::UnboundedReceiver<Event>,
    watcher_receiver: mpsc::Receiver<NewWatcher>,
}

/// Arbitrarily picked constant to limit memory consumed by queued watcher requests.
const WATCHER_CHANNEL_CAPACITY: usize = 128;

pub(crate) fn new_worker() -> (Worker, mpsc::Sender<NewWatcher>, mpsc::UnboundedSender<Event>) {
    let (event_sink, event_receiver) = futures::channel::mpsc::unbounded();
    let (watcher_sink, watcher_receiver) =
        futures::channel::mpsc::channel(WATCHER_CHANNEL_CAPACITY);
    (Worker { event_receiver, watcher_receiver }, watcher_sink, event_sink)
}

fn handle_new_watcher(
    neighbor_state: &HashMap<BindingId, HashMap<SpecifiedAddr<IpAddr>, NeighborState>>,
    watchers: &mut futures::stream::FuturesUnordered<Watcher>,
    NewWatcher { options, stream }: NewWatcher,
) {
    let options = match options.try_into() {
        Ok(options) => options,
        Err(e) => {
            warn!("failed to initialize neighbor watcher: {:?}", e);
            stream.control_handle().shutdown_with_epitaph(zx::Status::INVALID_ARGS);
            return;
        }
    };
    let event_queue = EventQueue(
        neighbor_state
            .iter()
            .map(|(binding_id, entries)| {
                entries.iter().map(
                    |(addr, NeighborState { state, last_updated, previously_reachable: _ })| {
                        fnet_neighbor::EntryIteratorItem::Existing(new_fidl_entry(
                            *binding_id,
                            *addr,
                            *state,
                            *last_updated,
                        ))
                    },
                )
            })
            .flatten()
            .chain(std::iter::once(fnet_neighbor::EntryIteratorItem::Idle(
                fnet_neighbor::IdleEvent,
            )))
            .collect(),
    );
    watchers.push(Watcher { stream, options, event_queue, responder: None });
}

#[derive(Clone, Copy, Debug)]
struct NeighborState {
    state: neighbor::EventState<Mac>,
    last_updated: StackTime,
    // True if the neighbor was at one point in state `Reachable` and has not
    // since transitioned to state `Unreachable`.
    previously_reachable: bool,
}

impl Worker {
    pub(crate) async fn run(self, ctx: Ctx) {
        let Self { event_receiver: event_stream, watcher_receiver: new_watchers } = self;
        let mut watchers = futures::stream::FuturesUnordered::<Watcher>::new();
        let mut neighbor_state: HashMap<_, HashMap<SpecifiedAddr<IpAddr>, NeighborState>> =
            HashMap::new();

        enum SinkItem {
            NewWatcher(NewWatcher),
            Event(Event),
        }
        // Always consume events before watchers. That allows external
        // observers to assume all side effects of a call are already
        // applied before a watcher observes its initial existing set of
        // properties.
        let mut stream = futures::stream::select_with_strategy(
            event_stream.map(SinkItem::Event),
            new_watchers.map(SinkItem::NewWatcher),
            |_: &mut ()| futures::stream::PollNext::Left,
        );

        enum Item {
            WatcherEnded(Result<(), fidl::Error>),
            SinkItem(Option<SinkItem>),
        }
        loop {
            let item = futures::select! {
                i = stream.next() => Item::SinkItem(i),
                w = watchers.select_next_some() => Item::WatcherEnded(w),
            };
            match item {
                Item::SinkItem(None) => {
                    if !watchers.is_empty() {
                        warn!(
                            "neighbor worker shutting down, dropping {} watchers",
                            watchers.len()
                        );
                    }
                    break;
                }
                Item::WatcherEnded(r) => r.unwrap_or_else(|e| {
                    if !e.is_closed() {
                        error!("error operating neighbor watcher {:?}", e);
                    }
                }),
                Item::SinkItem(Some(SinkItem::NewWatcher(new_watcher))) => {
                    handle_new_watcher(&neighbor_state, &mut watchers, new_watcher);
                }
                Item::SinkItem(Some(SinkItem::Event(
                    ref event @ Event { ref id, kind, addr, at },
                ))) => {
                    let DeviceIdAndName { id: binding_id, name: _ } = *id.bindings_id();

                    let old_entry = neighbor_state.get(&binding_id).and_then(|m| m.get(&addr));

                    if is_interesting_event(event, old_entry) {
                        info!(tag = "NUD"; "{}", EventLogger { event, ctx: &ctx });
                    } else {
                        debug!(tag = "NUD"; "{}", EventLogger { event, ctx: &ctx });
                    }

                    let entry = neighbor_state
                        .entry(binding_id)
                        .or_insert_with(|| HashMap::new())
                        .entry(addr);
                    let fidl_event = match kind {
                        neighbor::EventKind::Added(state) => match entry {
                            std::collections::hash_map::Entry::Occupied(occupied) => {
                                panic!(
                                    "neighbor added but already exists: entry={:?}, event={:?}",
                                    occupied.get(),
                                    event
                                );
                            }
                            std::collections::hash_map::Entry::Vacant(vacant) => {
                                let reachable = matches!(
                                    state,
                                    neighbor::EventState::Dynamic(
                                        neighbor::EventDynamicState::Reachable(_)
                                    )
                                );
                                let _ = vacant.insert(NeighborState {
                                    state,
                                    last_updated: at,
                                    previously_reachable: reachable,
                                });
                                fnet_neighbor::EntryIteratorItem::Added(new_fidl_entry(
                                    binding_id, addr, state, at,
                                ))
                            }
                        },
                        neighbor::EventKind::Removed => match entry {
                            std::collections::hash_map::Entry::Vacant(_) => {
                                panic!("neighbor removed but not found: {event:?}");
                            }
                            std::collections::hash_map::Entry::Occupied(occupied) => {
                                let NeighborState {
                                    state,
                                    last_updated: at,
                                    previously_reachable: _,
                                } = occupied.remove();

                                let entry = assert_matches!(
                                    neighbor_state.entry(binding_id),
                                    std::collections::hash_map::Entry::Occupied(o) => o
                                );
                                if entry.get().is_empty() {
                                    let _ = entry.remove();
                                }

                                fnet_neighbor::EntryIteratorItem::Removed(new_fidl_entry(
                                    binding_id, addr, state, at,
                                ))
                            }
                        },
                        neighbor::EventKind::Changed(state) => match entry {
                            std::collections::hash_map::Entry::Vacant(_) => {
                                panic!("neighbor changed but not found: {event:?}");
                            }
                            std::collections::hash_map::Entry::Occupied(mut occupied) => {
                                let NeighborState {
                                    state: current_state,
                                    last_updated: _,
                                    previously_reachable,
                                } = *occupied.get();
                                // NS3 core guarantees to only emit changed events if state
                                // actually changed.
                                assert_ne!(
                                    current_state, state,
                                    "neighbor changed but nothing changed: {event:?}",
                                );
                                // If the current state confirms reachable or
                                // unreachable, use that. Otherwise, use the
                                // tracked reachability from the old state.
                                let reachable = match &state {
                                    neighbor::EventState::Dynamic(
                                        neighbor::EventDynamicState::Reachable(_),
                                    ) => true,
                                    neighbor::EventState::Dynamic(
                                        neighbor::EventDynamicState::Unreachable(_),
                                    ) => false,
                                    _ => previously_reachable,
                                };
                                let _ = occupied.insert(NeighborState {
                                    state,
                                    last_updated: at,
                                    previously_reachable: reachable,
                                });
                                fnet_neighbor::EntryIteratorItem::Changed(new_fidl_entry(
                                    binding_id, addr, state, at,
                                ))
                            }
                        },
                    };
                    watchers.iter_mut().for_each(|watcher| {
                        watcher.push(fidl_event.clone());
                    });
                }
            }
        }
    }
}

#[derive(Debug)]
/// A bounded queue of [`Events`] to be returned to `fuchsia.net.neighbor/EntryIterator`.
struct EventQueue(VecDeque<fnet_neighbor::EntryIteratorItem>);

const MAX_ITEM_BATCH_SIZE: usize = fnet_neighbor::MAX_ITEM_BATCH_SIZE as usize;
// Arbitrarily-chosen maximum number of events to queue per client (4 times the
// maximum number of entries held in core per IP per interface).
const MAX_EVENTS: usize = 4 * neighbor::MAX_ENTRIES;

impl EventQueue {
    fn is_empty(&self) -> bool {
        let Self(event_queue) = self;
        event_queue.is_empty()
    }

    fn push(
        &mut self,
        event: fnet_neighbor::EntryIteratorItem,
    ) -> Result<(), fnet_neighbor::EntryIteratorItem> {
        let Self(event_queue) = self;
        if event_queue.len() >= MAX_EVENTS {
            return Err(event);
        }
        event_queue.push_back(event);
        Ok(())
    }

    fn pop_max(&mut self) -> impl IntoIterator<Item = fnet_neighbor::EntryIteratorItem> + '_ {
        let Self(event_queue) = self;
        let count = std::cmp::min(MAX_ITEM_BATCH_SIZE, event_queue.len());
        event_queue.drain(0..count)
    }
}

/// The task that serves `fuchsia.net.neighbor/EntryIterator`.
///
/// The future implementation drives `stream` and responds to the requests
/// with events from `event_queue`, and completes when `stream` is exhausted
/// (possibly with an error).
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub(crate) struct Watcher {
    stream: fnet_neighbor::EntryIteratorRequestStream,
    options: fnet_neighbor_ext::EntryIteratorOptions,
    event_queue: EventQueue,
    responder: Option<fnet_neighbor::EntryIteratorGetNextResponder>,
}

fn send_events(
    responder: fnet_neighbor::EntryIteratorGetNextResponder,
    events: &[fnet_neighbor::EntryIteratorItem],
) {
    responder.send(events).unwrap_or_else(|e| {
        if e.is_closed() {
            warn!("neighbor watcher closed when sending event");
        } else {
            error!("error sending event to neighbor watcher: {e:?}");
        }
    })
}

impl Future for Watcher {
    type Output = Result<(), fidl::Error>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        loop {
            let next_request = self.as_mut().stream.poll_next_unpin(cx)?;
            match futures::ready!(next_request) {
                Some(fnet_neighbor::EntryIteratorRequest::GetNext { responder }) => {
                    if !self.event_queue.is_empty() {
                        let events = self.event_queue.pop_max().into_iter().collect::<Vec<_>>();
                        send_events(responder, &events);
                    } else {
                        match &self.responder {
                            Some(existing) => {
                                existing
                                    .control_handle()
                                    .shutdown_with_epitaph(zx::Status::ALREADY_EXISTS);
                                return Poll::Ready(Ok(()));
                            }
                            None => {
                                self.responder = Some(responder);
                            }
                        }
                    }
                }
                None => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl Watcher {
    fn push(&mut self, event: fnet_neighbor::EntryIteratorItem) {
        let Self {
            stream,
            event_queue,
            responder,
            options: fnet_neighbor_ext::EntryIteratorOptions {},
        } = self;

        match responder.take() {
            Some(responder) => {
                debug_assert!(event_queue.is_empty());
                send_events(responder, std::slice::from_ref(&event));
            }
            None => {
                event_queue.push(event).unwrap_or_else(|_: fnet_neighbor::EntryIteratorItem| {
                    warn!("too many pending events enqueued for neighbor watcher, closing channel");
                    stream.control_handle().shutdown();
                });
            }
        }
    }
}

#[derive(Error, Debug)]
#[error("neighbor worker no longer available")]
pub(crate) struct WorkerClosedError;

/// Possible errors when serving `fuchsia.net.neighbor/View`.
#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error("failed to send a Watcher task to parent")]
    Send(#[from] WorkerClosedError),
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
}

impl ErrorLogExt for Error {
    fn log_level(&self) -> log::Level {
        match self {
            Self::Send(WorkerClosedError) => log::Level::Error,
            Self::Fidl(fidl) => fidl.log_level(),
        }
    }
}

pub(crate) struct NewWatcher {
    stream: fnet_neighbor::EntryIteratorRequestStream,
    options: fnet_neighbor::EntryIteratorOptions,
}

pub(super) async fn serve_view(
    stream: ViewRequestStream,
    sink: mpsc::Sender<NewWatcher>,
) -> Result<(), Error> {
    stream
        .err_into()
        .try_fold(sink, |mut sink, request| async move {
            match request {
                ViewRequest::OpenEntryIterator { it, options, control_handle: _ } => sink
                    .send(NewWatcher { stream: it.into_stream(), options })
                    .await
                    .map_err(|_: mpsc::SendError| Error::Send(WorkerClosedError))?,
            }
            Ok(sink)
        })
        .map_ok(|_: mpsc::Sender<NewWatcher>| ())
        .await
}

fn get_ethernet_id(
    ctx: &Ctx,
    interface: u64,
) -> Result<EthernetDeviceId<BindingsCtx>, ControllerError> {
    match BindingId::new(interface)
        .and_then(|id| ctx.bindings_ctx().devices.get_core_id(id))
        .ok_or(ControllerError::InterfaceNotFound)?
    {
        DeviceId::Ethernet(e) => Ok(e),
        // NUD is not supported for Loopback, pure IP, or blackhole devices.
        DeviceId::Loopback(_) | DeviceId::PureIp(_) | DeviceId::Blackhole(_) => {
            Err(ControllerError::InterfaceNotSupported)
        }
    }
}

#[netstack3_core::context_ip_bounds(A::Version, BindingsCtx)]
fn add_static_entry<A: IpAddress>(
    ctx: &mut Ctx,
    interface: u64,
    neighbor: A,
    mac: fnet::MacAddress,
) -> Result<(), ControllerError>
where
    A::Version: IpExt,
{
    let device_id = get_ethernet_id(ctx, interface)?;
    let mac = UnicastAddr::new(mac.into_ext()).ok_or(ControllerError::MacAddressNotUnicast)?;
    ctx.api()
        .neighbor::<A::Version, EthernetLinkDevice>()
        .insert_static_entry(&device_id, neighbor, mac)
        .map_err(|e| match e {
            StaticNeighborInsertionError::IpAddressInvalid => ControllerError::InvalidIpAddress,
            StaticNeighborInsertionError::TableFull => ControllerError::TooManyEntries,
        })
}

#[netstack3_core::context_ip_bounds(A::Version, BindingsCtx)]
fn probe_entry<A: IpAddress>(
    ctx: &mut Ctx,
    interface: u64,
    neighbor: A,
) -> Result<(), ControllerError>
where
    A::Version: IpExt,
{
    let device_id = get_ethernet_id(ctx, interface)?;
    ctx.api()
        .neighbor::<A::Version, EthernetLinkDevice>()
        .probe_entry(&device_id, neighbor)
        .map_err(|e| match e {
            TriggerNeighborProbeError::IpAddressInvalid => ControllerError::InvalidIpAddress,
            TriggerNeighborProbeError::NotFound(_) => ControllerError::NeighborNotFound,
            TriggerNeighborProbeError::LinkAddressUnknown => ControllerError::LinkAddressUnknown,
        })
}

#[netstack3_core::context_ip_bounds(A::Version, BindingsCtx)]
fn remove_entry<A: IpAddress>(
    ctx: &mut Ctx,
    interface: u64,
    neighbor: A,
) -> Result<(), ControllerError>
where
    A::Version: IpExt,
{
    let device_id = get_ethernet_id(ctx, interface)?;
    ctx.api()
        .neighbor::<A::Version, EthernetLinkDevice>()
        .remove_entry(&device_id, neighbor)
        .map_err(|e| match e {
            NeighborRemovalError::IpAddressInvalid => ControllerError::InvalidIpAddress,
            NeighborRemovalError::NotFound(NotFoundError) => ControllerError::NeighborNotFound,
        })
}

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
fn clear_entries<I: IpExt>(ctx: &mut Ctx, interface: u64) -> Result<(), ControllerError> {
    let device_id = get_ethernet_id(ctx, interface)?;
    Ok(ctx.api().neighbor::<I, EthernetLinkDevice>().flush_table(&device_id))
}

pub(super) async fn serve_controller(
    ctx: Ctx,
    stream: ControllerRequestStream,
) -> Result<(), fidl::Error> {
    stream
        .try_for_each(|request| async {
            let mut ctx: Ctx = ctx.clone();
            match request {
                ControllerRequest::AddEntry { interface, neighbor, mac, responder } => {
                    let result = match neighbor.into_ext() {
                        IpAddr::V4(v4) => add_static_entry(&mut ctx, interface, v4, mac),
                        IpAddr::V6(v6) => add_static_entry(&mut ctx, interface, v6, mac),
                    };
                    responder.send(result)
                }
                ControllerRequest::ProbeEntry { interface, neighbor, responder } => {
                    let result = match neighbor.into_ext() {
                        IpAddr::V4(v4) => probe_entry(&mut ctx, interface, v4),
                        IpAddr::V6(v6) => probe_entry(&mut ctx, interface, v6),
                    };
                    responder.send(result)
                }
                ControllerRequest::RemoveEntry { interface, neighbor, responder } => {
                    let result = match neighbor.into_ext() {
                        IpAddr::V4(v4) => remove_entry(&mut ctx, interface, v4),
                        IpAddr::V6(v6) => remove_entry(&mut ctx, interface, v6),
                    };
                    responder.send(result)
                }
                ControllerRequest::ClearEntries { interface, ip_version, responder } => {
                    let result = match ip_version {
                        fnet::IpVersion::V4 => clear_entries::<Ipv4>(&mut ctx, interface),
                        fnet::IpVersion::V6 => clear_entries::<Ipv6>(&mut ctx, interface),
                    };
                    responder.send(result)
                }
            }
        })
        .await
}

#[cfg(test)]
mod tests {
    use net_declare::{net_ip_v4, net_subnet_v4};
    use net_types::SpecifiedAddr;
    use net_types::ip::{Ipv4Addr, Subnet};
    use netstack3_core::routes::{Metric, RawMetric, RoutePreference};
    use test_case::test_case;

    use super::*;

    const RIGHT_ADDR_RAW: Ipv4Addr = net_ip_v4!("192.168.0.1");
    const RIGHT_ADDR: SpecifiedAddr<Ipv4Addr> =
        unsafe { SpecifiedAddr::new_unchecked(RIGHT_ADDR_RAW) };
    const WRONG_ADDR: SpecifiedAddr<Ipv4Addr> =
        unsafe { SpecifiedAddr::new_unchecked(net_ip_v4!("192.168.0.2")) };

    const RIGHT_DEV: u8 = 1;
    const WRONG_DEV: u8 = 2;

    const ONLINK_ENTRY: Entry<Ipv4Addr, u8> = Entry {
        subnet: unsafe { Subnet::new_unchecked(RIGHT_ADDR_RAW, 32) },
        device: RIGHT_DEV,
        gateway: None,
        metric: Metric::ExplicitMetric(RawMetric(0)),
        route_preference: RoutePreference::Medium,
    };
    const GATEWAY_ENTRY: Entry<Ipv4Addr, u8> = Entry {
        subnet: net_subnet_v4!("192.168.0.0/16"),
        device: RIGHT_DEV,
        gateway: Some(RIGHT_ADDR),
        metric: Metric::ExplicitMetric(RawMetric(0)),
        route_preference: RoutePreference::Medium,
    };
    const INTERNET_GATEWAY_ENTRY: Entry<Ipv4Addr, u8> = Entry {
        subnet: net_subnet_v4!("0.0.0.0/0"),
        device: RIGHT_DEV,
        gateway: Some(RIGHT_ADDR),
        metric: Metric::ExplicitMetric(RawMetric(0)),
        route_preference: RoutePreference::Medium,
    };

    #[test_case(RIGHT_ADDR, WRONG_DEV, ONLINK_ENTRY, NeighborType::Unknown
        => NeighborType::Unknown; "wrong_dev")]
    #[test_case(WRONG_ADDR, RIGHT_DEV, ONLINK_ENTRY, NeighborType::Unknown
        => NeighborType::Unknown; "wrong_addr")]
    #[test_case(RIGHT_ADDR, RIGHT_DEV, ONLINK_ENTRY, NeighborType::Unknown
        => NeighborType::Onlink; "unknown_to_onlink")]
    #[test_case(RIGHT_ADDR, RIGHT_DEV, GATEWAY_ENTRY, NeighborType::Unknown
        => NeighborType::Gateway; "unknown_to_gateway")]
    #[test_case(RIGHT_ADDR, RIGHT_DEV, GATEWAY_ENTRY, NeighborType::Onlink
        => NeighborType::Gateway; "onlink_to_gateway")]
    #[test_case(RIGHT_ADDR, RIGHT_DEV, ONLINK_ENTRY, NeighborType::Gateway
        => NeighborType::Gateway; "gateway_downgrade_forbidden")]
    #[test_case(RIGHT_ADDR, RIGHT_DEV, INTERNET_GATEWAY_ENTRY, NeighborType::Unknown
        => NeighborType::InternetGateway; "unknown_to_internet_gateway")]
    #[test_case(RIGHT_ADDR, RIGHT_DEV, INTERNET_GATEWAY_ENTRY, NeighborType::Onlink
        => NeighborType::InternetGateway; "onlink_to_internet_gateway")]
    #[test_case(RIGHT_ADDR, RIGHT_DEV, INTERNET_GATEWAY_ENTRY, NeighborType::Gateway
        => NeighborType::InternetGateway; "gateway_to_internet_gateway")]
    #[test_case(RIGHT_ADDR, RIGHT_DEV, GATEWAY_ENTRY, NeighborType::InternetGateway
        => NeighborType::InternetGateway; "internet_gateway_downgrade_forbidden")]
    fn upgrade_neighbor_type(
        addr: SpecifiedAddr<Ipv4Addr>,
        dev: u8,
        entry: Entry<Ipv4Addr, u8>,
        mut initial: NeighborType,
    ) -> NeighborType {
        initial.maybe_upgrade_with_route::<Ipv4, u8, u8>(&addr, &dev, &entry);
        initial
    }
}
