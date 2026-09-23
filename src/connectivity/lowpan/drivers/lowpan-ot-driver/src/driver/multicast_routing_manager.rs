// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude::*;
use fidl::Error::ClientChannelClosed;
use fidl_fuchsia_net_multicast_admin as fnet_mcast;
use fuchsia_async::net::DatagramSocket;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_sync::Mutex;
use futures::channel::{mpsc as fmpsc, oneshot};
use futures::stream::StreamExt;
use socket2::{Domain, Protocol};

use std::collections::HashMap;
use std::fmt::{Debug, Formatter};

use std::num::NonZeroU64;
use std::pin::Pin;

#[derive(Clone, Debug, PartialEq)]
struct MulticastForwardingCacheEntry {
    src_addr: std::net::Ipv6Addr,
    last_use_time: fuchsia_async::MonotonicInstant,
    interface_in: Option<NonZeroU64>,
    interface_out: Option<NonZeroU64>,
}

impl MulticastForwardingCacheEntry {
    pub fn new(
        src_addr: std::net::Ipv6Addr,
        interface_in: Option<NonZeroU64>,
        interface_out: Option<NonZeroU64>,
    ) -> MulticastForwardingCacheEntry {
        MulticastForwardingCacheEntry {
            src_addr,
            last_use_time: fuchsia_async::MonotonicInstant::now(),
            interface_in,
            interface_out,
        }
    }

    pub fn update_time(&mut self) {
        self.last_use_time = fuchsia_async::MonotonicInstant::now();
    }
}

// min_ttl value to be used for all routes added.
const MIN_TTL: u8 = 1;

// Maximum number of entries in the multicast routing cache.
const MAX_ROUTE_CACHE_CAPACITY: usize = 750;

// The timeout interval for unused route entries to be evicted.
const EXPIRATION_TIMEOUT: fuchsia_async::MonotonicDuration =
    fuchsia_async::MonotonicDuration::from_minutes(5);

// The interval for the periodic task that checks for expired route entries.
const ROUTE_EXPIRATION_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug)]
struct MulticastRoutingMissingRouteEvent {
    addresses: fnet_mcast::Ipv6UnicastSourceAndMulticastDestination,
    input_interface: NonZeroU64,
}

#[derive(Clone, Debug)]
enum MulticastCoordinatorEvent {
    // For listener add/remove, we don't introduce new Routes in netstack, only update them.
    ListenerAdded {
        group_addr: std::net::Ipv6Addr,
    },
    ListenerRemoved {
        group_addr: std::net::Ipv6Addr,
    },
    // For missing route events, we add routes to netstack and update the cache.
    MissingRoute {
        src_addr: std::net::Ipv6Addr,
        group_addr: std::net::Ipv6Addr,
        input_interface: NonZeroU64,
        output_interface: Option<NonZeroU64>,
    },
    // For route expiration task, we remove expired routes from netstack and update the cache.
    CheckExpirations,
}

enum MulticastRoutingManagerState {
    Consumed,
    Created {
        // The receiver that processes `MulticastCoordinatorEvent`.
        coordinator_receiver: fmpsc::UnboundedReceiver<MulticastCoordinatorEvent>,
        // Oneshot channel to pass the receiver to the event loop.
        route_event_sender:
            oneshot::Sender<fmpsc::UnboundedReceiver<MulticastRoutingMissingRouteEvent>>,
    },
    Started {
        // This task is watching the multicast routing events from netstack.
        _netstack_route_event_task: fuchsia_async::Task<()>,
        // The task that processes `MulticastCoordinatorEvent` and manages the routing cache.
        _coordinator_task: fuchsia_async::Task<()>,
    },
}

pub struct MulticastRoutingManager {
    state: Mutex<MulticastRoutingManagerState>,
    // The coordinator_sender sends events to the central coordinator task.
    coordinator_sender: fmpsc::UnboundedSender<MulticastCoordinatorEvent>,
    route_event_receiver: Mutex<
        Option<oneshot::Receiver<fmpsc::UnboundedReceiver<MulticastRoutingMissingRouteEvent>>>,
    >,
    // This proxy connects to the netstack to add/remove routes.
    routing_table_controller_proxy: fnet_mcast::Ipv6RoutingTableControllerProxy,
    thread_interface_id: NonZeroU64,
    backbone_interface_id: NonZeroU64,
}

impl Debug for MulticastRoutingManager {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "thread_interface_id: {}, backbone_interface_id: {}",
            self.thread_interface_id, self.backbone_interface_id,
        )?;
        Ok(())
    }
}

impl MulticastRoutingManager {
    pub fn new(net_if_id: NonZeroU64, backbone_if_id: NonZeroU64) -> Self {
        let proxy = connect_to_protocol::<fnet_mcast::Ipv6RoutingTableControllerMarker>()
            .expect("Failed to connect to Ipv6RoutingTableController service");

        let (coordinator_sender, coordinator_receiver) = fmpsc::unbounded();
        let (route_event_sender, route_event_receiver) = oneshot::channel();

        MulticastRoutingManager {
            state: Mutex::new(MulticastRoutingManagerState::Created {
                coordinator_receiver,
                route_event_sender,
            }),
            coordinator_sender,
            route_event_receiver: Mutex::new(Some(route_event_receiver)),
            routing_table_controller_proxy: proxy,
            thread_interface_id: net_if_id,
            backbone_interface_id: backbone_if_id,
        }
    }

    pub fn set_multicast_listener_callback_fn<OT: ot::BackboneRouter>(&self, ot_instance: &OT) {
        let coordinator_sender_copy = self.coordinator_sender.clone();
        let multicast_socket =
            DatagramSocket::new(Domain::IPV6, Some(Protocol::UDP)).expect("DatagramSocket::new()");
        let backbone_if_id = self.backbone_interface_id;

        ot_instance.set_multicast_listener_callback(Some(
            move |event: ot::BackboneRouterMulticastListenerEvent, address: &ot::Ip6Address| {
                match event {
                    ot::BackboneRouterMulticastListenerEvent::ListenerAdded => {
                        debug!(
                            tag = "mcast_routing";
                            "processing ot::BackboneRouterMulticastListenerEvent::ListenerAdded"
                        );
                        if let Err(err) = multicast_socket
                            .as_ref()
                            // `backbone_if_id` is guaranteed to be valid because it is set in
                            // `new`.
                            .join_multicast_v6(address, backbone_if_id.get().try_into().unwrap())
                        {
                            warn!("Unable to join multicast group `{:?}`: {:?}", address, err);
                        }

                        if let Err(e) = coordinator_sender_copy.unbounded_send(
                            MulticastCoordinatorEvent::ListenerAdded {
                                group_addr: std::net::Ipv6Addr::from(address.octets()),
                            },
                        ) {
                            warn!(tag = "mcast_routing"; "Failed to send ListenerAdded event: {:?}",
                            e);
                        }
                    }

                    ot::BackboneRouterMulticastListenerEvent::ListenerRemoved => {
                        debug!(
                            tag = "mcast_routing";
                            "processing ot::BackboneRouterMulticastListenerEvent::ListenerRemoved"
                        );
                        if let Err(err) = multicast_socket
                            .as_ref()
                            // `backbone_if_id` is guaranteed to be valid because it is set in
                            // `new`.
                            .leave_multicast_v6(address, backbone_if_id.get().try_into().unwrap())
                        {
                            warn!(
                                tag = "mcast_routing";
                                "Unable to leave multicast group `{:?}`: {:?}", address, err
                            );
                        }

                        if let Err(e) = coordinator_sender_copy.unbounded_send(
                            MulticastCoordinatorEvent::ListenerRemoved {
                                group_addr: std::net::Ipv6Addr::from(address.octets()),
                            },
                        ) {
                            warn!(tag = "mcast_routing"; "Failed to send ListenerRemoved event: \
                            {:?}",
                            e);
                        }
                    }
                }
            },
        ));
    }

    pub async fn add_route(
        src_addr: std::net::Ipv6Addr,
        group_addr: std::net::Ipv6Addr,
        interface_in: Option<NonZeroU64>,
        interface_out: Option<NonZeroU64>,
        routing_table_controller_proxy: &fnet_mcast::Ipv6RoutingTableControllerProxy,
    ) {
        let addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: fnet_ext::Ipv6Address(src_addr).into(),
            multicast_destination: fnet_ext::Ipv6Address(group_addr).into(),
        };

        let outgoing_interfaces = if let Some(out_if) = interface_out {
            vec![fnet_mcast::OutgoingInterfaces { id: out_if.into(), min_ttl: MIN_TTL }]
        } else {
            vec![]
        };

        let route = fnet_mcast::Route {
            expected_input_interface: Some(interface_in.unwrap().into()),
            action: Some(fnet_mcast::Action::OutgoingInterfaces(outgoing_interfaces)),
            ..Default::default()
        };

        match routing_table_controller_proxy.add_route(&addresses, &route).await {
            Err(err) => {
                let suffix =
                    if err.is_closed() { " Check event loop logs for OnClose reason." } else { "" };
                warn!(
                    tag = "mcast_routing";
                    "Got FIDL error {:?} when trying to add route {:?} \
                    for (src: {}, group: {}).{}",
                    err,
                    route,
                    src_addr,
                    group_addr,
                    suffix,
                );
            }
            Ok(Err(err)) => {
                warn!(
                    tag = "mcast_routing";
                    "Unexpected error `{:?}` trying to add route {:?} for (src: {}, group: {})",
                    err,
                    route,
                    src_addr,
                    group_addr,
                )
            }
            Ok(Ok(())) => {
                info!(tag = "mcast_routing"; "route added successfully");
            }
        }
    }

    pub async fn delete_route(
        src_addr: std::net::Ipv6Addr,
        group_addr: std::net::Ipv6Addr,
        routing_table_controller_proxy: &fnet_mcast::Ipv6RoutingTableControllerProxy,
    ) {
        let addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: fnet_ext::Ipv6Address(src_addr).into(),
            multicast_destination: fnet_ext::Ipv6Address(group_addr).into(),
        };

        match routing_table_controller_proxy.del_route(&addresses).await {
            Err(err) => {
                let suffix =
                    if err.is_closed() { " Check event loop logs for OnClose reason." } else { "" };
                warn!(
                    tag = "mcast_routing";
                    "Got FIDL error {:?} when trying to remove route for \
                    (src: {}, group: {}).{}",
                    err,
                    src_addr,
                    group_addr,
                    suffix,
                );
            }
            Ok(Err(err)) => {
                warn!(
                    tag = "mcast_routing";
                    "Unexpected error `{:?}` trying to remove route for (src: {}, group: {})",
                    err,
                    src_addr,
                    group_addr,
                )
            }
            Ok(Ok(())) => {
                info!(tag = "mcast_routing"; "route removed successfully");
            }
        }
    }

    fn update_multicast_routing_cache_table(
        cache_table: &mut HashMap<std::net::Ipv6Addr, Vec<MulticastForwardingCacheEntry>>,
        total_routes: &mut usize,
        src_addr: std::net::Ipv6Addr,
        group_addr: std::net::Ipv6Addr,
        in_interface: NonZeroU64,
        out_interface: Option<NonZeroU64>,
    ) -> Option<(std::net::Ipv6Addr, MulticastForwardingCacheEntry)> {
        // Check if the entry already exists. If it does, update and return.
        let found = {
            let entries = cache_table.entry(group_addr).or_insert_with(Vec::new);
            if let Some(entry) = entries.iter_mut().find(|e| e.src_addr == src_addr) {
                entry.interface_in = Some(in_interface);
                entry.interface_out = out_interface;
                entry.update_time();
                true
            } else {
                false
            }
        };

        if found {
            return None;
        }

        // Not found, new entry case. Check capacity and evict if needed.
        let mut evicted = None;
        if *total_routes >= MAX_ROUTE_CACHE_CAPACITY {
            let mut oldest_time = fuchsia_async::MonotonicInstant::from_nanos(i64::MAX);
            let mut oldest_loc = None;
            for (multicast_group_addr, multicast_group_entries) in cache_table.iter() {
                for (idx, entry) in multicast_group_entries.iter().enumerate() {
                    if entry.last_use_time < oldest_time {
                        oldest_time = entry.last_use_time;
                        oldest_loc = Some((*multicast_group_addr, idx));
                    }
                }
            }

            let (oldest_group_addr, oldest_idx) = oldest_loc.unwrap();
            let multicast_group_entries = cache_table.get_mut(&oldest_group_addr).unwrap();
            let oldest_entry = multicast_group_entries.swap_remove(oldest_idx);
            if multicast_group_entries.is_empty() {
                cache_table.remove(&oldest_group_addr);
            }
            *total_routes -= 1;
            evicted = Some((oldest_group_addr, oldest_entry));
        }

        // Insert new entry.
        let new_entry =
            MulticastForwardingCacheEntry::new(src_addr, Some(in_interface), out_interface);
        let entries = cache_table.entry(group_addr).or_insert_with(Vec::new);
        entries.push(new_entry);
        *total_routes += 1;

        evicted
    }

    pub fn start(&self) {
        info!(tag = "mcast_routing"; "starting multicast routing manager");
        let mut state_guard = self.state.lock();

        if matches!(*state_guard, MulticastRoutingManagerState::Started { .. }) {
            warn!(tag = "mcast_routing"; "start called when MulticastRoutingManager is already \
            started");
            return;
        }

        let old_state =
            std::mem::replace(&mut *state_guard, MulticastRoutingManagerState::Consumed);
        let MulticastRoutingManagerState::Created { coordinator_receiver, route_event_sender } =
            old_state
        else {
            panic!("MulticastRoutingManager was found in `Consumed` state while trying to start");
        };
        {
            let (sender, receiver) = fmpsc::unbounded();

            if let Err(_) = route_event_sender.send(receiver) {
                warn!(tag = "mcast_routing"; "Failed to send receiver to future, maybe already \
                closed");
            }

            let netstack_route_event_task =
                fasync::Task::spawn(Self::multicast_routing_netstack_route_event_loop(
                    self.routing_table_controller_proxy.clone(),
                    sender,
                ));

            let coordinator_task = fasync::Task::spawn(Self::coordinator_event_loop(
                self.routing_table_controller_proxy.clone(),
                self.thread_interface_id,
                self.backbone_interface_id,
                coordinator_receiver,
            ));

            *state_guard = MulticastRoutingManagerState::Started {
                _netstack_route_event_task: netstack_route_event_task,
                _coordinator_task: coordinator_task,
            };
        }
    }

    async fn coordinator_event_loop(
        routing_table_controller_proxy: fnet_mcast::Ipv6RoutingTableControllerProxy,
        thread_interface_id: NonZeroU64,
        backbone_interface_id: NonZeroU64,
        receiver: fmpsc::UnboundedReceiver<MulticastCoordinatorEvent>,
    ) {
        info!(tag = "mcast_routing"; "coordinator_event_loop started");
        let mut cache_table =
            HashMap::<std::net::Ipv6Addr, Vec<MulticastForwardingCacheEntry>>::new();
        let mut total_routes = 0;

        let mut receiver = receiver.fuse();
        let mut interval =
            fuchsia_async::Interval::new(fuchsia_async::MonotonicDuration::from_nanos(
                ROUTE_EXPIRATION_CHECK_INTERVAL.as_nanos() as i64,
            ))
            .fuse();

        loop {
            let event = futures::select! {
                event = receiver.next() => {
                    match event {
                        Some(e) => e,
                        None => break, // channel closed
                    }
                }
                _ = interval.next() => {
                    MulticastCoordinatorEvent::CheckExpirations
                }
            };

            match event {
                MulticastCoordinatorEvent::ListenerAdded { group_addr } => {
                    let mut routes_to_update = Vec::new();
                    if let Some(entries) = cache_table.get_mut(&group_addr) {
                        for entry in entries.iter_mut() {
                            if entry.interface_in == Some(backbone_interface_id)
                                && entry.interface_out != Some(thread_interface_id)
                            {
                                let needs_delete = entry.interface_out.is_some();
                                entry.interface_out = Some(thread_interface_id);
                                entry.update_time();
                                routes_to_update.push((entry.src_addr, needs_delete));
                            }
                        }
                    }

                    for (src_addr, needs_delete) in routes_to_update {
                        // TODO(https://fxbug.dev/493250937): Remove when netstack supports
                        // blocking route entries.
                        if needs_delete {
                            MulticastRoutingManager::delete_route(
                                src_addr,
                                group_addr,
                                &routing_table_controller_proxy,
                            )
                            .await;
                        }

                        MulticastRoutingManager::add_route(
                            src_addr,
                            group_addr,
                            Some(backbone_interface_id),
                            Some(thread_interface_id),
                            &routing_table_controller_proxy,
                        )
                        .await;
                    }
                }
                MulticastCoordinatorEvent::ListenerRemoved { group_addr } => {
                    let mut routes_to_delete = Vec::new();

                    if let Some(entries) = cache_table.get_mut(&group_addr) {
                        for entry in entries.iter_mut() {
                            let needs_delete = entry.interface_out.is_some();
                            entry.interface_out = None;

                            if needs_delete {
                                routes_to_delete.push(entry.src_addr);
                            }
                        }
                    }

                    for src_addr in routes_to_delete {
                        // TODO(https://fxbug.dev/493250937): Remove when netstack supports
                        // blocking route entries.
                        MulticastRoutingManager::delete_route(
                            src_addr,
                            group_addr,
                            &routing_table_controller_proxy,
                        )
                        .await;
                    }
                }
                MulticastCoordinatorEvent::MissingRoute {
                    src_addr,
                    group_addr,
                    input_interface,
                    output_interface,
                } => {
                    let in_interface = input_interface;

                    // Check if it already exists to avoid duplicate `AddRoute` FIDL calls.
                    let mut exists_with_same_out = false;
                    if let Some(entries) = cache_table.get_mut(&group_addr) {
                        if let Some(existing) = entries.iter_mut().find(|e| e.src_addr == src_addr)
                        {
                            if existing.interface_out == output_interface {
                                // Already in cache with same output interface, just update time.
                                existing.update_time();
                                exists_with_same_out = true;
                            }
                        }
                    }

                    if exists_with_same_out {
                        continue;
                    }

                    if let Some((evicted_group, evicted_entry)) =
                        Self::update_multicast_routing_cache_table(
                            &mut cache_table,
                            &mut total_routes,
                            src_addr,
                            group_addr,
                            in_interface,
                            output_interface,
                        )
                    {
                        // TODO(https://fxbug.dev/493250937): Remove when netstack supports
                        // blocking route entries.
                        if evicted_entry.interface_out.is_some() {
                            MulticastRoutingManager::delete_route(
                                evicted_entry.src_addr,
                                evicted_group,
                                &routing_table_controller_proxy,
                            )
                            .await;
                        }
                    }

                    if output_interface.is_some() {
                        MulticastRoutingManager::add_route(
                            src_addr,
                            group_addr,
                            Some(in_interface),
                            output_interface,
                            &routing_table_controller_proxy,
                        )
                        .await;
                    }
                }
                MulticastCoordinatorEvent::CheckExpirations => {
                    // Maximum allocated heap memory: 48 bytes * 750 = 36KB.
                    let mut routes_to_check = Vec::new();
                    for (group_addr, entries) in cache_table.iter() {
                        for entry in entries.iter() {
                            routes_to_check.push((
                                *group_addr,
                                entry.src_addr,
                                entry.last_use_time,
                                entry.interface_out.is_some(),
                            ));
                        }
                    }

                    // (group_addr, src_addr, needs_delete)
                    let mut to_remove_routes = Vec::new();
                    // (group_addr, src_addr, new_synced_time_nanos)
                    let mut to_update_times = Vec::new();

                    for (group_addr, src_addr, last_use_time, needs_delete) in routes_to_check {
                        let addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
                            unicast_source: fnet_ext::Ipv6Address(src_addr).into(),
                            multicast_destination: fnet_ext::Ipv6Address(group_addr).into(),
                        };

                        match routing_table_controller_proxy.get_route_stats(&addresses).await {
                            Ok(Ok(route_stats)) => {
                                let (effective_last_used, is_fallback) = match route_stats.last_used
                                {
                                    Some(time) => {
                                        (fuchsia_async::MonotonicInstant::from_nanos(time), false)
                                    }
                                    None => (last_use_time, true),
                                };

                                let now = fuchsia_async::MonotonicInstant::now();
                                let elapsed = now - effective_last_used;

                                if elapsed > EXPIRATION_TIMEOUT {
                                    let source_str = if is_fallback {
                                        "local timer fallback"
                                    } else {
                                        "netstack stats"
                                    };
                                    info!(tag = "mcast_routing"; "route (src: {}, group: {}) \
                                    expired (idle for {} seconds) ({})",
                                    src_addr, group_addr, elapsed.into_seconds(), source_str);
                                    to_remove_routes.push((group_addr, src_addr, needs_delete));
                                } else if !is_fallback {
                                    to_update_times.push((
                                        group_addr,
                                        src_addr,
                                        effective_last_used,
                                    ));
                                }
                            }
                            Ok(Err(e)) => {
                                warn!(tag = "mcast_routing"; "failed to get route stats for \
                                (src: {}, group: {}): {:?}", src_addr, group_addr, e)
                            }
                            Err(e) => {
                                warn!(tag = "mcast_routing"; "FIDL error getting route stats \
                                for (src: {}, group: {}): {:?}", src_addr, group_addr, e)
                            }
                        }
                    }

                    for (group_addr, src_addr, new_time) in to_update_times {
                        if let Some(entries) = cache_table.get_mut(&group_addr) {
                            if let Some(e) = entries.iter_mut().find(|x| x.src_addr == src_addr) {
                                e.last_use_time = new_time;
                            }
                        }
                    }

                    for (group_addr, src_addr, needs_delete) in to_remove_routes {
                        if let Some(entries) = cache_table.get_mut(&group_addr) {
                            if let Some(pos) = entries.iter().position(|x| x.src_addr == src_addr) {
                                entries.swap_remove(pos);
                                total_routes -= 1;
                            }
                            if entries.is_empty() {
                                cache_table.remove(&group_addr);
                            }
                        }
                        if needs_delete {
                            MulticastRoutingManager::delete_route(
                                src_addr,
                                group_addr,
                                &routing_table_controller_proxy,
                            )
                            .await;
                        }
                    }
                }
            }
        }
        info!(tag = "mcast_routing"; "coordinator_event_loop ended");
    }

    async fn multicast_routing_netstack_route_event_loop(
        routing_table_controller_proxy: fnet_mcast::Ipv6RoutingTableControllerProxy,
        sender: fmpsc::UnboundedSender<MulticastRoutingMissingRouteEvent>,
    ) {
        info!(tag = "mcast_routing"; "multicast_routing_netstack_route_event_loop started");
        loop {
            match routing_table_controller_proxy.watch_routing_events().await {
                Ok((dropped_events, addresses, input_interface, event)) => {
                    if dropped_events != 0 {
                        warn!(
                            tag = "mcast_routing";
                            "Dropped {dropped_events:?} events before getting the event"
                        );
                    }

                    info!(
                        tag = "mcast_routing";
                        "Got routing event {:?} for addresses (src: {}, group: {}) on interface: \
                        {:?}",
                        event,
                        std::net::Ipv6Addr::from(addresses.unicast_source.addr),
                        std::net::Ipv6Addr::from(addresses.multicast_destination.addr),
                        input_interface
                    );

                    match event {
                        fnet_mcast::RoutingEvent::MissingRoute(fnet_mcast::Empty {}) => {
                            if let Err(e) =
                                sender.unbounded_send(MulticastRoutingMissingRouteEvent {
                                    addresses,
                                    input_interface: NonZeroU64::new(input_interface)
                                        .expect("InterfaceId must be non-zero"),
                                })
                            {
                                warn!(tag = "mcast_routing"; "Failed to send MissingRoute event \
                                to receiver: {:?}", e);
                            }
                        }
                        fnet_mcast::RoutingEvent::WrongInputInterface(interface) => {
                            warn!(
                                tag = "mcast_routing";
                                "Got routing event WrongInputInterface({:?})", interface
                            );
                        }
                    }
                }
                Err(err) => {
                    warn!(
                        tag = "mcast_routing";
                        "Got error in waiting for routing events: {:?}", err
                    );

                    if let ClientChannelClosed { epitaph, .. } = err
                        && epitaph.is_peer_closed()
                    {
                        match routing_table_controller_proxy.take_event_stream().try_next().await {
                            Ok(Some(fnet_mcast::Ipv6RoutingTableControllerEvent::OnClose {
                                error: reason,
                            })) => {
                                warn!(tag = "mcast_routing"; "Fidl channel closed with error from \
                                OnClose: {:?}", reason);
                            }
                            Ok(None) => {
                                warn!(tag = "mcast_routing"; "Fidl channel closed cleanly without \
                                OnClose event");
                            }
                            Err(e) => {
                                warn!(tag = "mcast_routing"; "Error reading from event stream \
                                while expecting OnClose: {:?}", e);
                            }
                        }
                        // TODO(https://fxbug.dev/493250937): file a crash report here.
                    }

                    break;
                }
            }
        }
    }

    fn has_multicast_listener(&self, instance: &ot::Instance, addr: std::net::Ipv6Addr) -> bool {
        let group_addr: ot::Ip6Address = addr.octets().into();
        instance.iter_multicast_listeners().any(|info| (&info).get_address() == group_addr)
    }

    fn handle_missing_route_event(
        &self,
        instance: &ot::Instance,
        route_event: MulticastRoutingMissingRouteEvent,
    ) {
        let backbone_interface_id = self.backbone_interface_id;
        let thread_interface_id = self.thread_interface_id;

        let mut output_interface = None;
        let has_listener = self.has_multicast_listener(
            instance,
            std::net::Ipv6Addr::from(route_event.addresses.multicast_destination.addr),
        );

        if route_event.input_interface == backbone_interface_id {
            // Forward multicast traffic from Backbone to Thread if the group address is
            // subscribed by any Thread device via MLR.
            if has_listener {
                output_interface = Some(thread_interface_id);
            }
        } else if route_event.input_interface == thread_interface_id {
            let src_addr_prefix = ot::Ip6NetworkPrefix::from(route_event.addresses.unicast_source);
            let dst_addr_prefix =
                ot::Ip6NetworkPrefix::from(route_event.addresses.multicast_destination);

            // verify source prefix is not link local.
            // verify source prefix is not the mesh local prefix.
            // verify destination prefix has a scope larger than `RealmLocalScope`.
            if (!src_addr_prefix.is_link_local())
                && (instance.get_mesh_local_prefix() != &src_addr_prefix)
                && (dst_addr_prefix.get_scope() > ot::Scope::REALM_LOCAL)
            {
                output_interface = Some(backbone_interface_id)
            }
        } else {
            info!(
                tag = "mcast_routing";
                "ignoring missing route event from unexpected interface {}",
                route_event.input_interface
            );
            return;
        }

        info!(
            tag = "mcast_routing";
            "processing missing route event, input_if:{}, output_if:{:?}, backbone_if:{}, \
            net_if:{}",
            route_event.input_interface,
            output_interface,
            backbone_interface_id,
            thread_interface_id
        );

        // Route the missing event to the coordinator.
        if let Err(e) =
            self.coordinator_sender.unbounded_send(MulticastCoordinatorEvent::MissingRoute {
                src_addr: std::net::Ipv6Addr::from(route_event.addresses.unicast_source.addr),
                group_addr: std::net::Ipv6Addr::from(
                    route_event.addresses.multicast_destination.addr,
                ),
                input_interface: route_event.input_interface,
                output_interface,
            })
        {
            warn!(tag = "mcast_routing"; "Failed to send MissingRoute event to coordinator: {:?}",
            e);
        }
    }
}

pub trait MulticastRoutingManagerPollerExt {
    fn multicast_routing_manager_future(&self)
    -> Pin<Box<dyn Future<Output = Result> + Send + '_>>;
}

impl<T: AsRef<ot::Instance> + AsRef<Option<MulticastRoutingManager>> + Send>
    MulticastRoutingManagerPollerExt for fuchsia_sync::Mutex<T>
{
    fn multicast_routing_manager_future(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result> + Send + '_>> {
        let receiver = {
            let guard = self.lock();
            let mrm: &Option<MulticastRoutingManager> = guard.as_ref();
            mrm.as_ref().and_then(|mrm| mrm.route_event_receiver.lock().take())
        };

        Box::pin(async move {
            // Only start listening to netstack events if the multicast routing manager is
            // initialized.
            if let Some(rx) = receiver {
                match rx.await {
                    Ok(mut receiver) => {
                        while let Some(route_event) = receiver.next().await {
                            let guard = self.lock();
                            let ot: &ot::Instance = guard.as_ref();
                            let mrm: &Option<MulticastRoutingManager> = guard.as_ref();
                            match mrm {
                                Some(mrm) => mrm.handle_missing_route_event(ot, route_event),
                                None => {
                                    warn!(tag = "mcast_routing"; "Received missing route event \
                                    but MulticastRoutingManager is None");
                                }
                            }
                        }
                        warn!(tag = "mcast_routing"; "netstack event stream ended unexpectedly");
                    }
                    Err(e) => {
                        warn!(tag = "mcast_routing"; "Failed to receive netstack event stream: {}",
                        e);
                    }
                }
            }
            // Once the Infra interface changes, lowpan driver will restart and this future will
            // be dropped.
            futures::future::pending().await
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use fidl_fuchsia_net_multicast_admin as fnet_mcast;
    use futures::TryStreamExt as _;

    // Backbone interface (i.e. wlan client).
    const BACKBONE_IF_ID: NonZeroU64 = NonZeroU64::new(1).unwrap();
    // Thread interface (i.e. lowpan0).
    const NET_IF_ID: NonZeroU64 = NonZeroU64::new(2).unwrap();

    const GROUP_DEST_ADDR1: std::net::Ipv6Addr = net_declare::std_ip_v6!("ff04::abcd");

    const UNICAST_SOURCE_ADDR: std::net::Ipv6Addr =
        net_declare::std_ip_v6!("fda2:a902:d05b:1::abcd");
    const MULTICAST_DEST_ADDR1: std::net::Ipv6Addr = GROUP_DEST_ADDR1;

    const UNICAST_SOURCE_ADDR_FIDL: fidl_fuchsia_net::Ipv6Address =
        fidl_fuchsia_net::Ipv6Address { addr: UNICAST_SOURCE_ADDR.octets() };
    const MULTICAST_DEST_ADDR1_FIDL: fidl_fuchsia_net::Ipv6Address =
        fidl_fuchsia_net::Ipv6Address { addr: MULTICAST_DEST_ADDR1.octets() };

    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_cache_insertion() {
        let mut cache_table =
            HashMap::<std::net::Ipv6Addr, Vec<MulticastForwardingCacheEntry>>::new();
        let mut total_routes = 0;
        let src_addr = UNICAST_SOURCE_ADDR;
        let group_addr = MULTICAST_DEST_ADDR1;
        let in_if = BACKBONE_IF_ID;
        let out_if = Some(NET_IF_ID);

        let result = MulticastRoutingManager::update_multicast_routing_cache_table(
            &mut cache_table,
            &mut total_routes,
            src_addr,
            group_addr,
            in_if,
            out_if,
        );

        assert_eq!(result, None);
        let actual_entry = &cache_table.get(&group_addr).unwrap()[0];
        let expected_entry = MulticastForwardingCacheEntry {
            src_addr,
            last_use_time: actual_entry.last_use_time,
            interface_in: Some(in_if),
            interface_out: out_if,
        };
        // Construct expected cache table.
        let expected_cache = HashMap::from([(group_addr, vec![expected_entry])]);

        assert_eq!(cache_table, expected_cache);
        assert_eq!(total_routes, 1);
    }

    // Using fake time since we hope to mimic different timestamp.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_cache_update() {
        let mut cache_table =
            HashMap::<std::net::Ipv6Addr, Vec<MulticastForwardingCacheEntry>>::new();
        let mut total_routes = 0;
        let src_addr = UNICAST_SOURCE_ADDR;
        let group_addr = MULTICAST_DEST_ADDR1;
        let in_if = BACKBONE_IF_ID;
        let out_if_1 = Some(NET_IF_ID);
        let out_if_2 = None;

        // First insertion.
        let result = MulticastRoutingManager::update_multicast_routing_cache_table(
            &mut cache_table,
            &mut total_routes,
            src_addr,
            group_addr,
            in_if,
            out_if_1,
        );
        assert_eq!(result, None);

        let initial_time = cache_table
            .values()
            .next()
            .expect("cache table should not be empty")
            .first()
            .expect("route vector should not be empty")
            .last_use_time;
        // Update fake time.
        fuchsia_async::TestExecutor::advance_to(
            fuchsia_async::MonotonicInstant::now()
                + fuchsia_async::MonotonicDuration::from_millis(5),
        )
        .await;

        // Second insertion (update).
        let result = MulticastRoutingManager::update_multicast_routing_cache_table(
            &mut cache_table,
            &mut total_routes,
            src_addr,
            group_addr,
            in_if,
            out_if_2,
        );

        assert_eq!(result, None);
        let actual_entry = &cache_table.get(&group_addr).unwrap()[0];
        let updated_time = actual_entry.last_use_time;
        assert!(updated_time > initial_time);

        let expected_entry = MulticastForwardingCacheEntry {
            src_addr,
            last_use_time: updated_time,
            interface_in: Some(in_if),
            interface_out: out_if_2,
        };
        let expected_cache = HashMap::from([(group_addr, vec![expected_entry])]);

        assert_eq!(cache_table, expected_cache);
        assert_eq!(total_routes, 1);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_coordinator_missing_route_adds_route() {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fnet_mcast::Ipv6RoutingTableControllerMarker,
        >();
        let (sender, receiver) = fmpsc::unbounded();

        let thread_if = NET_IF_ID;
        let backbone_if = BACKBONE_IF_ID;

        let _task = fuchsia_async::Task::spawn(MulticastRoutingManager::coordinator_event_loop(
            proxy,
            thread_if,
            backbone_if,
            receiver,
        ));

        // Trigger `MissingRoute` event.
        sender
            .unbounded_send(MulticastCoordinatorEvent::MissingRoute {
                src_addr: UNICAST_SOURCE_ADDR,
                group_addr: MULTICAST_DEST_ADDR1,
                input_interface: BACKBONE_IF_ID,
                output_interface: Some(thread_if),
            })
            .unwrap();

        // Verify `AddRoute` is called to netstack.
        let (addresses, route, responder) = assert_matches!(
            stream.try_next().await.unwrap(),
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::AddRoute {
                addresses,
                route,
                responder
            }) => (addresses, route, responder)
        );

        let expected_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: UNICAST_SOURCE_ADDR_FIDL,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(addresses, expected_addresses);
        let expected_route = fnet_mcast::Route {
            expected_input_interface: Some(BACKBONE_IF_ID.get()),
            action: Some(fnet_mcast::Action::OutgoingInterfaces(vec![
                fnet_mcast::OutgoingInterfaces { id: NET_IF_ID.get(), min_ttl: 1 },
            ])),
            ..Default::default()
        };
        assert_eq!(route, expected_route);

        responder.send(Ok(())).unwrap();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_coordinator_missing_route_blocking_no_add_route() {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fnet_mcast::Ipv6RoutingTableControllerMarker,
        >();
        let (sender, receiver) = fmpsc::unbounded();

        let thread_if = NET_IF_ID;
        let backbone_if = BACKBONE_IF_ID;

        let task = fuchsia_async::Task::spawn(MulticastRoutingManager::coordinator_event_loop(
            proxy,
            thread_if,
            backbone_if,
            receiver,
        ));

        // Trigger `MissingRoute` event where Thread side has no listener.
        sender
            .unbounded_send(MulticastCoordinatorEvent::MissingRoute {
                src_addr: UNICAST_SOURCE_ADDR,
                group_addr: MULTICAST_DEST_ADDR1,
                input_interface: BACKBONE_IF_ID,
                output_interface: None,
            })
            .unwrap();

        // Close the sender so the coordinator loop ends.
        drop(sender);

        // Wait for the task to finish.
        task.await;

        // Verify no FIDL calls were made since output_interface was None.
        assert_matches!(stream.try_next().await.unwrap(), None);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_coordinator_listener_added_updates_route() {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fnet_mcast::Ipv6RoutingTableControllerMarker,
        >();
        let (sender, receiver) = fmpsc::unbounded();

        let thread_if = NET_IF_ID;
        let backbone_if = BACKBONE_IF_ID;

        let _task = fuchsia_async::Task::spawn(MulticastRoutingManager::coordinator_event_loop(
            proxy,
            thread_if,
            backbone_if,
            receiver,
        ));

        // 1. Add a blocking route (output_interface: None).
        sender
            .unbounded_send(MulticastCoordinatorEvent::MissingRoute {
                src_addr: UNICAST_SOURCE_ADDR,
                group_addr: MULTICAST_DEST_ADDR1,
                input_interface: BACKBONE_IF_ID,
                output_interface: None,
            })
            .unwrap();

        // Verify try_next doesn't have any pending requests.
        use futures::FutureExt as _;
        assert_matches!(stream.try_next().now_or_never(), None);

        // 2. Add listener for the same group address.
        sender
            .unbounded_send(MulticastCoordinatorEvent::ListenerAdded {
                group_addr: MULTICAST_DEST_ADDR1,
            })
            .unwrap();

        // Verify `AddRoute` is called on FIDL server.
        let (addresses, route, responder) = match stream.try_next().await {
            Ok(Some(fnet_mcast::Ipv6RoutingTableControllerRequest::AddRoute {
                addresses,
                route,
                responder,
            })) => (addresses, route, responder),
            other => panic!("Expected AddRoute request, got {:?}", other),
        };

        let expected_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: UNICAST_SOURCE_ADDR_FIDL,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(addresses, expected_addresses);

        let expected_route = fnet_mcast::Route {
            expected_input_interface: Some(BACKBONE_IF_ID.get()),
            action: Some(fnet_mcast::Action::OutgoingInterfaces(vec![
                fnet_mcast::OutgoingInterfaces { id: NET_IF_ID.get(), min_ttl: 1 },
            ])),
            ..Default::default()
        };
        assert_eq!(route, expected_route);

        responder.send(Ok(())).unwrap();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_cache_eviction_at_limit() {
        let mut cache_table =
            HashMap::<std::net::Ipv6Addr, Vec<MulticastForwardingCacheEntry>>::new();
        let mut total_routes = 0;
        let in_if = BACKBONE_IF_ID;
        let out_if = Some(NET_IF_ID);

        // Insert `MAX_ROUTE_CACHE_CAPACITY` entries.
        for i in 0..MAX_ROUTE_CACHE_CAPACITY {
            let base_bits = u128::from(net_declare::std_ip_v6!("2001::"));
            let src_addr = std::net::Ipv6Addr::from(base_bits + i as u128);
            let group_addr = MULTICAST_DEST_ADDR1;

            let result = MulticastRoutingManager::update_multicast_routing_cache_table(
                &mut cache_table,
                &mut total_routes,
                src_addr,
                group_addr,
                in_if,
                out_if,
            );
            assert_eq!(result, None);
        }

        assert_eq!(total_routes, MAX_ROUTE_CACHE_CAPACITY);
        assert_eq!(cache_table.len(), 1);

        // Record the oldest entry (the first one).
        let oldest_entry = cache_table
            .get(&MULTICAST_DEST_ADDR1)
            .unwrap()
            .iter()
            .min_by_key(|e| e.last_use_time)
            .unwrap();
        let oldest_src = oldest_entry.src_addr;
        let oldest_time = oldest_entry.last_use_time;

        // Insert the (`MAX_ROUTE_CACHE_CAPACITY` + 1)th entry.
        let new_src_addr = net_declare::std_ip_v6!("2001::ffff");
        let result = MulticastRoutingManager::update_multicast_routing_cache_table(
            &mut cache_table,
            &mut total_routes,
            new_src_addr,
            MULTICAST_DEST_ADDR1,
            in_if,
            out_if,
        );

        let expected_evicted_entry = MulticastForwardingCacheEntry {
            src_addr: oldest_src,
            last_use_time: oldest_time,
            interface_in: Some(in_if),
            interface_out: out_if,
        };
        assert_eq!(result, Some((MULTICAST_DEST_ADDR1, expected_evicted_entry)));
        assert_eq!(total_routes, MAX_ROUTE_CACHE_CAPACITY);
        let entries = cache_table.get(&MULTICAST_DEST_ADDR1).unwrap();
        assert!(entries.iter().any(|e| e.src_addr == new_src_addr));
        assert!(!entries.iter().any(|e| e.src_addr == oldest_src));
    }

    // Using fake time since we hope to mimic timestamps.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_coordinator_listener_removed_reverts_active_route() {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fnet_mcast::Ipv6RoutingTableControllerMarker,
        >();
        let (sender, receiver) = fmpsc::unbounded();

        let thread_if = NET_IF_ID;
        let backbone_if = BACKBONE_IF_ID;

        let _task = fuchsia_async::Task::spawn(MulticastRoutingManager::coordinator_event_loop(
            proxy,
            thread_if,
            backbone_if,
            receiver,
        ));

        // 1. Add a route which Thread side has listeners.
        sender
            .unbounded_send(MulticastCoordinatorEvent::MissingRoute {
                src_addr: UNICAST_SOURCE_ADDR,
                group_addr: MULTICAST_DEST_ADDR1,
                input_interface: BACKBONE_IF_ID,
                output_interface: Some(thread_if),
            })
            .unwrap();

        // Verify `AddRoute` is called to NS.
        let (addresses, route, responder) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::AddRoute {
                addresses,
                route,
                responder,
            }) => (addresses, route, responder),
            other => panic!("Expected AddRoute request, got {:?}", other),
        };

        let expected_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: UNICAST_SOURCE_ADDR_FIDL,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(addresses, expected_addresses);

        let expected_route = fnet_mcast::Route {
            expected_input_interface: Some(BACKBONE_IF_ID.get()),
            action: Some(fnet_mcast::Action::OutgoingInterfaces(vec![
                fnet_mcast::OutgoingInterfaces { id: NET_IF_ID.get(), min_ttl: 1 },
            ])),
            ..Default::default()
        };
        assert_eq!(route, expected_route);
        responder.send(Ok(())).unwrap();

        // 2. Remove listener on Thread side.
        sender
            .unbounded_send(MulticastCoordinatorEvent::ListenerRemoved {
                group_addr: MULTICAST_DEST_ADDR1,
            })
            .unwrap();

        // Verify `DelRoute` is called to tear down the active route.
        let (del_addresses, del_responder) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::DelRoute {
                addresses,
                responder,
            }) => (addresses, responder),
            other => panic!("Expected DelRoute request, got {:?}", other),
        };
        let expected_del_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: UNICAST_SOURCE_ADDR_FIDL,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(del_addresses, expected_del_addresses);
        del_responder.send(Ok(())).unwrap();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_coordinator_missing_route_refreshes_cache() {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fnet_mcast::Ipv6RoutingTableControllerMarker,
        >();
        let (sender, receiver) = fmpsc::unbounded();

        let thread_if = NET_IF_ID;
        let backbone_if = BACKBONE_IF_ID;

        let task = fuchsia_async::Task::spawn(MulticastRoutingManager::coordinator_event_loop(
            proxy,
            thread_if,
            backbone_if,
            receiver,
        ));

        let event = MulticastCoordinatorEvent::MissingRoute {
            src_addr: UNICAST_SOURCE_ADDR,
            group_addr: MULTICAST_DEST_ADDR1,
            input_interface: BACKBONE_IF_ID,
            output_interface: Some(thread_if),
        };

        // 1. Send first MissingRoute event.
        sender.unbounded_send(event.clone()).unwrap();

        // Verify `AddRoute` is called to NS.
        let (addresses, route, responder) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::AddRoute {
                addresses,
                route,
                responder,
            }) => (addresses, route, responder),
            other => panic!("Expected AddRoute request, got {:?}", other),
        };

        let expected_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: UNICAST_SOURCE_ADDR_FIDL,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(addresses, expected_addresses);

        let expected_route = fnet_mcast::Route {
            expected_input_interface: Some(BACKBONE_IF_ID.get()),
            action: Some(fnet_mcast::Action::OutgoingInterfaces(vec![
                fnet_mcast::OutgoingInterfaces { id: NET_IF_ID.get(), min_ttl: 1 },
            ])),
            ..Default::default()
        };
        assert_eq!(route, expected_route);
        responder.send(Ok(())).unwrap();

        // 2. Send the exact same MissingRoute event again.
        sender.unbounded_send(event).unwrap();

        // Verify no more FIDL calls are made on the stream.
        drop(sender);
        task.await;
        assert_matches!(stream.try_next().await.unwrap(), None);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_coordinator_check_expirations_evicts_stale_route() {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fnet_mcast::Ipv6RoutingTableControllerMarker,
        >();
        let (sender, receiver) = fmpsc::unbounded();

        let thread_if = NET_IF_ID;
        let backbone_if = BACKBONE_IF_ID;

        let _task = fuchsia_async::Task::spawn(MulticastRoutingManager::coordinator_event_loop(
            proxy,
            thread_if,
            backbone_if,
            receiver,
        ));

        // 1. Add a route.
        sender
            .unbounded_send(MulticastCoordinatorEvent::MissingRoute {
                src_addr: UNICAST_SOURCE_ADDR,
                group_addr: MULTICAST_DEST_ADDR1,
                input_interface: BACKBONE_IF_ID,
                output_interface: Some(thread_if),
            })
            .unwrap();

        // Verify `AddRoute` is called to NS.
        let (addresses, route, responder) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::AddRoute {
                addresses,
                route,
                responder,
            }) => (addresses, route, responder),
            other => panic!("Expected AddRoute request, got {:?}", other),
        };

        let expected_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: UNICAST_SOURCE_ADDR_FIDL,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(addresses, expected_addresses);

        let expected_route = fnet_mcast::Route {
            expected_input_interface: Some(BACKBONE_IF_ID.get()),
            action: Some(fnet_mcast::Action::OutgoingInterfaces(vec![
                fnet_mcast::OutgoingInterfaces { id: NET_IF_ID.get(), min_ttl: 1 },
            ])),
            ..Default::default()
        };
        assert_eq!(route, expected_route);
        responder.send(Ok(())).unwrap();

        let old_time = fuchsia_async::MonotonicInstant::now();
        // Advance time to 301 seconds to trigger expiration.
        fuchsia_async::TestExecutor::advance_to(
            fuchsia_async::MonotonicInstant::now()
                + fuchsia_async::MonotonicDuration::from_seconds(301),
        )
        .await;

        // Handle `GetRouteStats`.
        let (addresses, responder) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::GetRouteStats {
                addresses,
                responder,
            }) => (addresses, responder),
            other => panic!("Expected GetRouteStats request, got {:?}", other),
        };
        assert_eq!(addresses.unicast_source, UNICAST_SOURCE_ADDR_FIDL);

        // Return a very old timestamp relative to `now`.
        responder
            .send(Ok(&fnet_mcast::RouteStats {
                last_used: Some(old_time.into_nanos()),
                ..Default::default()
            }))
            .unwrap();

        // Verify `DelRoute` is called.
        let (del_addresses, del_responder) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::DelRoute {
                addresses,
                responder,
            }) => (addresses, responder),
            other => panic!("Expected DelRoute request, got {:?}", other),
        };
        let expected_del_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: UNICAST_SOURCE_ADDR_FIDL,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(del_addresses, expected_del_addresses);
        del_responder.send(Ok(())).unwrap();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_cache_empty_vector_cleanup_on_expiration() {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fnet_mcast::Ipv6RoutingTableControllerMarker,
        >();
        let (sender, receiver) = fmpsc::unbounded();

        let thread_if = NET_IF_ID;
        let backbone_if = BACKBONE_IF_ID;

        let _task = fuchsia_async::Task::spawn(MulticastRoutingManager::coordinator_event_loop(
            proxy,
            thread_if,
            backbone_if,
            receiver,
        ));

        let src_addr_2 = net_declare::std_ip_v6!("fda2:a902:d05b:1::abce");

        // 1. Add two routes for same group.
        sender
            .unbounded_send(MulticastCoordinatorEvent::MissingRoute {
                src_addr: UNICAST_SOURCE_ADDR,
                group_addr: MULTICAST_DEST_ADDR1,
                input_interface: BACKBONE_IF_ID,
                output_interface: Some(thread_if),
            })
            .unwrap();

        let (addresses, route, responder) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::AddRoute {
                addresses,
                route,
                responder,
            }) => (addresses, route, responder),
            other => panic!("Expected AddRoute request, got {:?}", other),
        };
        let expected_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: UNICAST_SOURCE_ADDR_FIDL,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(addresses, expected_addresses);

        let expected_route = fnet_mcast::Route {
            expected_input_interface: Some(BACKBONE_IF_ID.get()),
            action: Some(fnet_mcast::Action::OutgoingInterfaces(vec![
                fnet_mcast::OutgoingInterfaces { id: NET_IF_ID.get(), min_ttl: 1 },
            ])),
            ..Default::default()
        };
        assert_eq!(route, expected_route);
        responder.send(Ok(())).unwrap();

        sender
            .unbounded_send(MulticastCoordinatorEvent::MissingRoute {
                src_addr: src_addr_2,
                group_addr: MULTICAST_DEST_ADDR1,
                input_interface: BACKBONE_IF_ID,
                output_interface: Some(thread_if),
            })
            .unwrap();

        let (addresses, route, responder) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::AddRoute {
                addresses,
                route,
                responder,
            }) => (addresses, route, responder),
            other => panic!("Expected AddRoute request, got {:?}", other),
        };
        let src_addr_2_fidl = fidl_fuchsia_net::Ipv6Address { addr: src_addr_2.octets() };
        let expected_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: src_addr_2_fidl,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(addresses, expected_addresses);

        let expected_route = fnet_mcast::Route {
            expected_input_interface: Some(BACKBONE_IF_ID.get()),
            action: Some(fnet_mcast::Action::OutgoingInterfaces(vec![
                fnet_mcast::OutgoingInterfaces { id: NET_IF_ID.get(), min_ttl: 1 },
            ])),
            ..Default::default()
        };
        assert_eq!(route, expected_route);
        responder.send(Ok(())).unwrap();

        let old_time = fuchsia_async::MonotonicInstant::now();
        // Advance time to 301 seconds to trigger expiration.
        fuchsia_async::TestExecutor::advance_to(
            fuchsia_async::MonotonicInstant::now()
                + fuchsia_async::MonotonicDuration::from_seconds(301),
        )
        .await;

        for _ in 0..2 {
            let responder = match stream.try_next().await.unwrap() {
                Some(fnet_mcast::Ipv6RoutingTableControllerRequest::GetRouteStats {
                    responder,
                    ..
                }) => responder,
                other => panic!("Expected GetRouteStats request, got {:?}", other),
            };
            responder
                .send(Ok(&fnet_mcast::RouteStats {
                    last_used: Some(old_time.into_nanos()),
                    ..Default::default()
                }))
                .unwrap();
        }

        // Both are removed.
        for _ in 0..2 {
            let (del_addresses, del_responder) = match stream.try_next().await.unwrap() {
                Some(fnet_mcast::Ipv6RoutingTableControllerRequest::DelRoute {
                    addresses,
                    responder,
                }) => (addresses, responder),
                other => panic!("Expected DelRoute request, got {:?}", other),
            };
            let expected_addr_1 = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
                unicast_source: UNICAST_SOURCE_ADDR_FIDL,
                multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
            };
            let src_addr_2_fidl = fidl_fuchsia_net::Ipv6Address { addr: src_addr_2.octets() };
            let expected_addr_2 = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
                unicast_source: src_addr_2_fidl,
                multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
            };
            let is_match = del_addresses == expected_addr_1 || del_addresses == expected_addr_2;
            assert!(is_match, "Unexpected del_addresses: {:?}", del_addresses);
            del_responder.send(Ok(())).unwrap();
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_mlr_cache_last_use_time_syncs_with_netstack() {
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fnet_mcast::Ipv6RoutingTableControllerMarker,
        >();
        let (sender, receiver) = fmpsc::unbounded();

        let thread_if = NET_IF_ID;
        let backbone_if = BACKBONE_IF_ID;

        let task = fuchsia_async::Task::spawn(MulticastRoutingManager::coordinator_event_loop(
            proxy,
            thread_if,
            backbone_if,
            receiver,
        ));

        // Add a route.
        sender
            .unbounded_send(MulticastCoordinatorEvent::MissingRoute {
                src_addr: UNICAST_SOURCE_ADDR,
                group_addr: MULTICAST_DEST_ADDR1,
                input_interface: BACKBONE_IF_ID,
                output_interface: Some(thread_if),
            })
            .unwrap();

        let (addresses, route, responder1) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::AddRoute {
                addresses,
                route,
                responder,
            }) => (addresses, route, responder),
            other => panic!("Expected AddRoute request, got {:?}", other),
        };
        let expected_addresses = fnet_mcast::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: UNICAST_SOURCE_ADDR_FIDL,
            multicast_destination: MULTICAST_DEST_ADDR1_FIDL,
        };
        assert_eq!(addresses, expected_addresses);

        let expected_route = fnet_mcast::Route {
            expected_input_interface: Some(BACKBONE_IF_ID.get()),
            action: Some(fnet_mcast::Action::OutgoingInterfaces(vec![
                fnet_mcast::OutgoingInterfaces { id: NET_IF_ID.get(), min_ttl: 1 },
            ])),
            ..Default::default()
        };
        assert_eq!(route, expected_route);
        responder1.send(Ok(())).unwrap();

        // Trigger expiration check.
        sender.unbounded_send(MulticastCoordinatorEvent::CheckExpirations).unwrap();

        // Handle `GetRouteStats`.
        let (addresses, responder) = match stream.try_next().await.unwrap() {
            Some(fnet_mcast::Ipv6RoutingTableControllerRequest::GetRouteStats {
                addresses,
                responder,
            }) => (addresses, responder),
            other => panic!("Expected GetRouteStats request, got {:?}", other),
        };
        assert_eq!(addresses.unicast_source, UNICAST_SOURCE_ADDR_FIDL);

        // Return a recent timestamp to prove it syncs and doesn't evict.
        let recent_time = fuchsia_async::MonotonicInstant::now().into_nanos();
        responder
            .send(Ok(&fnet_mcast::RouteStats {
                last_used: Some(recent_time),
                ..Default::default()
            }))
            .unwrap();

        // Verify no `DelRoute` is called because it's not expired.
        drop(sender);
        task.await;
        assert_matches!(stream.try_next().await.unwrap(), None);
    }
}
