// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The special-purpose event loop used by the reachability monitor.
//!
//! This event loop receives events from netstack. Thsose events are used by the reachability
//! monitor to infer the connectivity state.

use reachability_handler::ReachabilityState;

use anyhow::{Context as _, anyhow};
use fidl_fuchsia_net_interfaces_ext::{self as fnet_interfaces_ext, Update as _};
use fuchsia_async::{self as fasync};
use fuchsia_inspect::Inspector;
use fuchsia_inspect::health::Reporter;
use futures::channel::mpsc;
use futures::prelude::*;
use futures::select;
use log::{debug, error, info, warn};
use named_timer::NamedTimeoutExt;
use reachability_core::dig::Dig;
use reachability_core::fetch::Fetch;
use reachability_core::ping::Ping;
use reachability_core::route_table::RouteTable;
use reachability_core::telemetry::{self, TelemetryEvent, TelemetrySender};
use reachability_core::{
    FIDL_TIMEOUT_ID, InterfaceView, Monitor, NeighborCache, NetworkCheckAction, NetworkCheckCookie,
    NetworkCheckResult, NetworkChecker, NetworkCheckerOutcome, watchdog,
};
use reachability_handler::ReachabilityHandler;
use std::collections::{HashMap, HashSet};
use std::pin::pin;
use {
    fidl_fuchsia_hardware_network as fhardware_network, fidl_fuchsia_net_debug as fnet_debug,
    fidl_fuchsia_net_interfaces as fnet_interfaces, fidl_fuchsia_net_neighbor as fnet_neighbor,
    fidl_fuchsia_net_routes as fnet_routes, fidl_fuchsia_net_routes_ext as fnet_routes_ext,
};

const REPORT_PERIOD: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(60);
const PROBE_PERIOD: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(60);
const FIDL_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(90);

struct SystemDispatcher;

#[async_trait::async_trait]
impl watchdog::SystemDispatcher for SystemDispatcher {
    type DeviceDiagnostics = DeviceDiagnosticsProvider;

    async fn log_debug_info(&self) -> Result<(), watchdog::Error> {
        let diagnostics =
            fuchsia_component::client::connect_to_protocol::<fnet_debug::DiagnosticsMarker>()
                .map_err(|e| {
                    error!(e:? = e; "failed to connect to protocol");
                    watchdog::Error::NotSupported
                })?;
        diagnostics
            .log_debug_info_to_syslog()
            .map_err(watchdog::Error::Fidl)
            .on_timeout_named(&FIDL_TIMEOUT_ID, FIDL_TIMEOUT, || Err(watchdog::Error::Timeout))
            .await
    }

    fn get_device_diagnostics(
        &self,
        interface: u64,
    ) -> Result<Self::DeviceDiagnostics, watchdog::Error> {
        let interfaces_debug =
            fuchsia_component::client::connect_to_protocol::<fnet_debug::InterfacesMarker>()
                .map_err(|e| {
                    error!(e:? = e; "failed to connect to protocol");
                    watchdog::Error::NotSupported
                })?;
        let (port, server_end) = fidl::endpoints::create_proxy();
        interfaces_debug.get_port(interface, server_end)?;

        let (diagnostics, server_end) = fidl::endpoints::create_proxy();
        port.get_diagnostics(server_end)?;

        Ok(DeviceDiagnosticsProvider { interface, diagnostics, port })
    }
}

struct DeviceDiagnosticsProvider {
    interface: u64,
    diagnostics: fhardware_network::DiagnosticsProxy,
    port: fhardware_network::PortProxy,
}

#[async_trait::async_trait]
impl watchdog::DeviceDiagnosticsProvider for DeviceDiagnosticsProvider {
    async fn get_counters(&self) -> Result<watchdog::DeviceCounters, watchdog::Error> {
        self.port
            .get_counters()
            .map_err(watchdog::Error::Fidl)
            .and_then(
                |fhardware_network::PortGetCountersResponse { rx_frames, tx_frames, .. }| match (
                    rx_frames, tx_frames,
                ) {
                    (Some(rx_frames), Some(tx_frames)) => {
                        futures::future::ok(watchdog::DeviceCounters { rx_frames, tx_frames })
                    }
                    (None, Some(_)) | (Some(_), None) | (None, None) => {
                        error!(iface = self.interface; "missing information from port counters");
                        futures::future::err(watchdog::Error::NotSupported)
                    }
                },
            )
            .on_timeout_named(&FIDL_TIMEOUT_ID, FIDL_TIMEOUT, || Err(watchdog::Error::Timeout))
            .await
    }

    async fn log_debug_info(&self) -> Result<(), watchdog::Error> {
        self.diagnostics
            .log_debug_info_to_syslog()
            .map_err(watchdog::Error::Fidl)
            .on_timeout_named(&FIDL_TIMEOUT_ID, FIDL_TIMEOUT, || Err(watchdog::Error::Timeout))
            .await
    }
}

type Watchdog = watchdog::Watchdog<SystemDispatcher>;

async fn handle_network_check_message<'a>(
    netcheck_futures: &mut futures::stream::FuturesUnordered<
        futures::future::BoxFuture<'a, (NetworkCheckCookie, NetworkCheckResult)>,
    >,
    msg: Option<(NetworkCheckAction, NetworkCheckCookie)>,
) {
    let (action, cookie) = msg
        .context("network check receiver unexpectedly closed")
        .unwrap_or_else(|err| exit_with_anyhow_error(err));
    match action {
        NetworkCheckAction::Ping(parameters) => {
            netcheck_futures.push(Box::pin(async move {
                let result = reachability_core::ping::Pinger
                    .ping(&parameters.interface_name, parameters.addr)
                    .await;
                (cookie, NetworkCheckResult::Ping { parameters, result })
            }));
        }
        NetworkCheckAction::ResolveDns(parameters) => {
            netcheck_futures.push(Box::pin(async move {
                let ips = reachability_core::dig::Digger::new()
                    .dig(&parameters.interface_name, &parameters.domain)
                    .await;
                (cookie, NetworkCheckResult::ResolveDns { parameters, ips })
            }));
        }
        NetworkCheckAction::Fetch(parameters) => {
            netcheck_futures.push(Box::pin(async move {
                let result = reachability_core::fetch::Fetcher
                    .fetch(
                        &parameters.interface_name,
                        &parameters.domain,
                        &parameters.path,
                        &parameters.ip,
                    )
                    .await;
                (cookie, NetworkCheckResult::Fetch { parameters, result })
            }));
        }
    }
}

/// The event loop.
pub struct EventLoop {
    monitor: Monitor,
    handler: ReachabilityHandler,
    watchdog: Watchdog,
    interface_properties: HashMap<
        u64,
        fnet_interfaces_ext::PropertiesAndState<(), fnet_interfaces_ext::DefaultInterest>,
    >,
    neighbor_cache: NeighborCache,
    routes: RouteTable,
    telemetry_sender: Option<TelemetrySender>,
    network_check_receiver: mpsc::UnboundedReceiver<(NetworkCheckAction, NetworkCheckCookie)>,
    inspector: &'static Inspector,
}

impl EventLoop {
    /// `new` returns an `EventLoop` instance.
    pub fn new(
        monitor: Monitor,
        handler: ReachabilityHandler,
        network_check_receiver: mpsc::UnboundedReceiver<(NetworkCheckAction, NetworkCheckCookie)>,
        inspector: &'static Inspector,
    ) -> Self {
        fuchsia_inspect::component::health().set_starting_up();
        EventLoop {
            monitor,
            handler,
            watchdog: Watchdog::new(),
            interface_properties: Default::default(),
            neighbor_cache: Default::default(),
            routes: Default::default(),
            telemetry_sender: None,
            network_check_receiver,
            inspector,
        }
    }

    /// `run` starts the event loop.
    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        use fuchsia_component::client::connect_to_protocol;

        let cobalt_svc = fuchsia_component::client::connect_to_protocol::<
            fidl_fuchsia_metrics::MetricEventLoggerFactoryMarker,
        >()
        .context("connect to metrics service")
        .unwrap_or_else(|err| exit_with_anyhow_error(err));

        let cobalt_proxy = match telemetry::create_metrics_logger(cobalt_svc).await {
            Ok(proxy) => proxy,
            Err(e) => {
                warn!("Metrics logging is unavailable: {}", e);

                // If it is not possible to acquire a metrics logging proxy, create a disconnected
                // proxy and attempt to serve the policy API with metrics disabled.
                let (proxy, _) = fidl::endpoints::create_proxy::<
                    fidl_fuchsia_metrics::MetricEventLoggerMarker,
                >();
                proxy
            }
        };

        let telemetry_inspect_node = self.inspector.root().create_child("telemetry");
        let (telemetry_sender, telemetry_fut) =
            telemetry::serve_telemetry(cobalt_proxy, telemetry_inspect_node);
        let telemetry_fut = telemetry_fut.fuse();
        self.telemetry_sender = Some(telemetry_sender.clone());
        self.monitor.set_telemetry_sender(telemetry_sender);

        let if_watcher_stream = {
            let interface_state = connect_to_protocol::<fnet_interfaces::StateMarker>()
                .context("network_manager failed to connect to interface state")
                .unwrap_or_else(|err| exit_with_anyhow_error(err));
            fnet_interfaces_ext::event_stream_from_state(&interface_state, Default::default())
                .context("get interface event stream")
                .unwrap_or_else(|err| exit_with_anyhow_error(err))
                .fuse()
        };

        let neigh_watcher_stream = {
            let view = connect_to_protocol::<fnet_neighbor::ViewMarker>()
                .context("failed to connect to neighbor view")
                .unwrap_or_else(|err| exit_with_anyhow_error(err));
            let (proxy, server_end) =
                fidl::endpoints::create_proxy::<fnet_neighbor::EntryIteratorMarker>();
            let () = view
                .open_entry_iterator(server_end, &fnet_neighbor::EntryIteratorOptions::default())
                .context("failed to open EntryIterator")
                .unwrap_or_else(|err| exit_with_anyhow_error(err));
            futures::stream::try_unfold(proxy, |proxy| {
                proxy.get_next().map_ok(|e| {
                    Some((
                        futures::stream::iter(e.into_iter().map(Result::<_, fidl::Error>::Ok)),
                        proxy,
                    ))
                })
            })
            .try_flatten()
            .fuse()
        };

        let ipv4_route_event_stream = {
            let state_v4 = connect_to_protocol::<fnet_routes::StateV4Marker>()
                .context("failed to connect to fuchsia.net.routes/StateV4")
                .unwrap_or_else(|err| exit_with_anyhow_error(err));
            fnet_routes_ext::event_stream_from_state(&state_v4)
                .context("failed to initialize a `WatcherV4` client")
                .unwrap_or_else(|err| exit_with_anyhow_error(err))
                .fuse()
        };
        let ipv6_route_event_stream = {
            let state_v6 = connect_to_protocol::<fnet_routes::StateV6Marker>()
                .context("failed to connect to fuchsia.net.routes/StateV6")
                .unwrap_or_else(|err| exit_with_anyhow_error(err));
            fnet_routes_ext::event_stream_from_state(&state_v6)
                .context("failed to initialize a `WatcherV6` client")
                .unwrap_or_else(|err| exit_with_anyhow_error(err))
                .fuse()
        };

        let mut probe_futures = futures::stream::FuturesUnordered::new();
        let mut netcheck_futures = futures::stream::FuturesUnordered::new();
        let report_stream = fasync::Interval::new(REPORT_PERIOD).fuse();

        let mut if_watcher_stream = pin!(if_watcher_stream);
        let mut report_stream = pin!(report_stream);
        let mut neigh_watcher_stream = pin!(neigh_watcher_stream);
        let mut ipv4_route_event_stream = pin!(ipv4_route_event_stream);
        let mut ipv6_route_event_stream = pin!(ipv6_route_event_stream);
        let mut telemetry_fut = pin!(telemetry_fut);

        // Establish the current routing table state.
        let (v4_routes, v6_routes) = futures::join!(
            fnet_routes_ext::collect_routes_until_idle::<_, HashSet<_>>(
                ipv4_route_event_stream.by_ref(),
            ),
            fnet_routes_ext::collect_routes_until_idle::<_, HashSet<_>>(
                ipv6_route_event_stream.by_ref(),
            )
        );
        self.routes = RouteTable::new_with_existing_routes(
            v4_routes
                .context("get existing IPv4 routes")
                .unwrap_or_else(|err| exit_with_anyhow_error(err)),
            v6_routes
                .context("get existing IPv6 routes")
                .unwrap_or_else(|err| exit_with_anyhow_error(err)),
        );

        debug!("starting event loop");

        fuchsia_inspect::component::health().set_ok();

        loop {
            select! {
                if_watcher_res = if_watcher_stream.try_next() => {
                    if let Some(id) = self.handle_interface_watcher_result(if_watcher_res).await {
                        probe_futures.push(fasync::Interval::new(PROBE_PERIOD).map(move |()| id).into_future());
                    }
                },
                neigh_res = neigh_watcher_stream.try_next() => {
                    let event = neigh_res
                        .unwrap_or_else(|err| exit_with_anyhow_error(
                            anyhow!("neighbor watcher event stream fidl error: {err}")
                        ))
                        .unwrap_or_else(|| exit_with_anyhow_error(
                            anyhow!("neighbor event stream ended")
                        ));
                    self.neighbor_cache.process_neighbor_event(event);
                }
                route_v4_res = ipv4_route_event_stream.try_next() => {
                    let event = route_v4_res
                        .unwrap_or_else(|err| exit_with_anyhow_error(
                            anyhow!("ipv4 route watch error: {err}")
                        ))
                        .unwrap_or_else(|| exit_with_anyhow_error(
                            anyhow!("ipv4 route event stream ended")
                        ));
                    self.handle_route_watcher_event(event);
                }
                route_v6_res = ipv6_route_event_stream.try_next() => {
                    let event = route_v6_res
                        .unwrap_or_else(|err| exit_with_anyhow_error(
                            anyhow!("ipv6 route watch error: {err}")
                        ))
                        .unwrap_or_else(|| exit_with_anyhow_error(
                            anyhow!("ipv6 route event stream ended")
                        ));
                    self.handle_route_watcher_event(event);
                }
                report = report_stream.next() => {
                    let () = report
                        .context("periodic timer for reporting unexpectedly ended")
                        .unwrap_or_else(|err| exit_with_anyhow_error(err));
                    let () = self.monitor.report_state();
                },
                probe = probe_futures.select_next_some() => {
                    match probe {
                        (Some(id), stream) => {
                            if let Some(fnet_interfaces_ext::PropertiesAndState { properties, state: _ }) = self.interface_properties.get(&id) {
                                let () = Self::begin_network_check(
                                    &mut self.monitor,
                                    &mut self.watchdog,
                                    &mut self.handler,
                                    properties,
                                    &self.routes,
                                    &self.neighbor_cache
                                ).await;

                                let () = probe_futures.push(stream.into_future());
                            }
                        }
                        (None, _) => {
                            exit_with_anyhow_error(anyhow!(
                                "probe timer for probing reachability unexpectedly ended"
                            ));
                        }
                    }
                },
                msg = self.network_check_receiver.next() => {
                    handle_network_check_message(&mut netcheck_futures, msg).await;
                },
                netcheck_res = netcheck_futures.select_next_some() => {
                    self.handle_netcheck_response(netcheck_res).await;
                },
                telemetry_res = telemetry_fut => exit_with_anyhow_error(
                    anyhow!("unexpectedly stopped serving telemetry: {telemetry_res:?}")
                ),
            }
        }
    }

    async fn handle_interface_watcher_result(
        &mut self,
        if_watcher_res: Result<
            Option<fnet_interfaces_ext::EventWithInterest<fnet_interfaces_ext::DefaultInterest>>,
            fidl::Error,
        >,
    ) -> Option<u64> {
        let event = if_watcher_res
            .unwrap_or_else(|err| {
                exit_with_anyhow_error(anyhow!("interface watcher event stream fidl error: {err}"))
            })
            .unwrap_or_else(|| {
                exit_with_anyhow_error(anyhow!("interface watcher stream unexpectedly ended"))
            });

        // Only proceed with tracking interfaces in `interface_properties` if
        // the interface should be monitored.
        let should_monitor_interface = match event.inner() {
            fnet_interfaces::Event::Existing(properties)
            | fnet_interfaces::Event::Added(properties) => Self::should_monitor_interface(
                properties
                    .port_class
                    .clone()
                    .expect("non-Changed Events should have port_class present"),
            ),
            // Changed and Removed events are preceded by a previous Existing
            // or Added event that add the interface to `interface_properties`.
            // When the interface does not exist in the map, don't process the
            // Changed or Removed event.
            fnet_interfaces::Event::Changed(fnet_interfaces::Properties { id, .. }) => self
                .interface_properties
                .contains_key(&id.expect("Changed events must include the interface id")),
            fnet_interfaces::Event::Removed(id) => self.interface_properties.contains_key(id),
            fnet_interfaces::Event::Idle(_) => true,
        };
        if !should_monitor_interface {
            return None;
        }

        let discovered_id = self
            .handle_interface_watcher_event(event)
            .await
            .unwrap_or_else(|err| exit_with_anyhow_error(err));
        if let Some(id) = discovered_id {
            if let Some(telemetry_sender) = &self.telemetry_sender {
                let has_default_ipv4_route =
                    self.interface_properties.values().any(|p| p.properties.has_default_ipv4_route);
                let has_default_ipv6_route =
                    self.interface_properties.values().any(|p| p.properties.has_default_ipv6_route);
                telemetry_sender.send(TelemetryEvent::NetworkConfig {
                    has_default_ipv4_route,
                    has_default_ipv6_route,
                });
            }
            return Some(id);
        }
        None
    }

    async fn handle_interface_watcher_event(
        &mut self,
        event: fnet_interfaces_ext::EventWithInterest<fnet_interfaces_ext::DefaultInterest>,
    ) -> Result<Option<u64>, anyhow::Error> {
        match self
            .interface_properties
            .update(event)
            .context("failed to update interface properties map with watcher event")?
        {
            fnet_interfaces_ext::UpdateResult::Added { properties, state: _ }
            | fnet_interfaces_ext::UpdateResult::Existing { properties, state: _ } => {
                let id = properties.id;
                debug!("setting timer for interface {}", id);

                let () = Self::begin_network_check(
                    &mut self.monitor,
                    &mut self.watchdog,
                    &mut self.handler,
                    properties,
                    &self.routes,
                    &self.neighbor_cache,
                )
                .await;

                return Ok(Some(id.get()));
            }
            fnet_interfaces_ext::UpdateResult::Changed {
                previous:
                    fnet_interfaces::Properties {
                        online,
                        addresses,
                        has_default_ipv4_route,
                        has_default_ipv6_route,
                        ..
                    },
                current:
                    properties @ fnet_interfaces_ext::Properties {
                        addresses: current_addresses,
                        id: _,
                        name: _,
                        port_class: _,
                        online: _,
                        has_default_ipv4_route: _,
                        has_default_ipv6_route: _,
                        port_identity_koid: _,
                    },
                state: _,
            } => {
                // NOTE: Filter changed events to only changes that might affect
                // reachability. This acts as a guard against too many
                // reachability checks in case of fuchsia.net.interfaces adding
                // more fields.
                if online.is_some()
                    || has_default_ipv4_route.is_some()
                    || has_default_ipv6_route.is_some()
                    || addresses.is_some_and(|addresses| {
                        let previous = addresses
                            .iter()
                            .filter_map(|fnet_interfaces::Address { addr, .. }| addr.as_ref());
                        let current = current_addresses.iter().map(
                            |fnet_interfaces_ext::Address {
                                 addr,
                                 valid_until: _,
                                 preferred_lifetime_info: _,
                                 assignment_state,
                             }| {
                                assert_eq!(
                                    *assignment_state,
                                    fnet_interfaces::AddressAssignmentState::Assigned
                                );
                                addr
                            },
                        );
                        previous.ne(current)
                    })
                {
                    let () = Self::begin_network_check(
                        &mut self.monitor,
                        &mut self.watchdog,
                        &mut self.handler,
                        properties,
                        &self.routes,
                        &self.neighbor_cache,
                    )
                    .await;
                }
            }
            fnet_interfaces_ext::UpdateResult::Removed(
                fnet_interfaces_ext::PropertiesAndState { properties, state: () },
            ) => {
                self.watchdog.handle_interface_removed(properties.id.get());
                self.monitor.handle_interface_removed(properties);
            }
            fnet_interfaces_ext::UpdateResult::NoChange => {}
        }
        Ok(None)
    }

    /// Determine whether an interface should be monitored or not.
    fn should_monitor_interface(port_class: fnet_interfaces::PortClass) -> bool {
        match port_class {
            fnet_interfaces::PortClass::Loopback(_)
            | fnet_interfaces::PortClass::Blackhole(_)
            | fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Lowpan) => false,
            fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Virtual)
            | fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Ethernet)
            | fnet_interfaces::PortClass::Device(fhardware_network::PortClass::WlanClient)
            | fnet_interfaces::PortClass::Device(fhardware_network::PortClass::WlanAp)
            | fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Ppp)
            | fnet_interfaces::PortClass::Device(fhardware_network::PortClass::Bridge) => true,
            fnet_interfaces::PortClass::Device(
                fhardware_network::PortClass::__SourceBreaking { unknown_ordinal },
            ) => {
                log::warn!("unknown fhardware_network::PortClass ordinal {unknown_ordinal:?}");
                false
            }
            fnet_interfaces::PortClass::__SourceBreaking { unknown_ordinal } => {
                log::warn!("unknown fnet_interfaces::PortClass ordinal {unknown_ordinal:?}");
                false
            }
        }
    }

    /// Handles events observed by the route watchers by adding/removing routes
    /// from the underlying `RouteTable`.
    ///
    /// # Panics
    ///
    /// Panics if the given event is unexpected (e.g. not an add or remove).
    pub fn handle_route_watcher_event<I: net_types::ip::Ip>(
        &mut self,
        event: fnet_routes_ext::Event<I>,
    ) {
        match event {
            fnet_routes_ext::Event::Added(route) => {
                if !self.routes.add_route(route) {
                    error!("Received add event for already existing route: {:?}", route)
                }
            }
            fnet_routes_ext::Event::Removed(route) => {
                if !self.routes.remove_route(&route) {
                    error!("Received removed event for non-existing route: {:?}", route)
                }
            }
            // Note that we don't expect to observe any existing events, because
            // the route watchers were drained of existing events prior to
            // starting the event loop.
            fnet_routes_ext::Event::Existing(_)
            | fnet_routes_ext::Event::Idle
            | fnet_routes_ext::Event::Unknown => {
                panic!("route watcher observed unexpected event: {:?}", event)
            }
        }
    }

    async fn begin_network_check(
        monitor: &mut Monitor,
        watchdog: &mut Watchdog,
        handler: &mut ReachabilityHandler,
        properties: &fnet_interfaces_ext::Properties<fnet_interfaces_ext::DefaultInterest>,
        routes: &RouteTable,
        neighbor_cache: &NeighborCache,
    ) {
        let view = InterfaceView {
            properties,
            routes,
            neighbors: neighbor_cache.get_interface_neighbors(properties.id.get()),
        };

        // TODO(https://fxbug.dev/42074495): Move watchdog into its own future in the eventloop to prevent
        // network check reliance on the watchdog completing.
        let () = watchdog
            .check_interface_state(zx::MonotonicInstant::get(), &SystemDispatcher {}, view)
            .await;

        let (system_internet, system_gateway, system_dns, system_http) = {
            match monitor.begin(view) {
                Ok(NetworkCheckerOutcome::MustResume) => return,
                Ok(NetworkCheckerOutcome::Complete) => {
                    let monitor_state = monitor.state();
                    (
                        monitor_state.system_has_internet(),
                        monitor_state.system_has_gateway(),
                        monitor_state.system_has_dns(),
                        monitor_state.system_has_http(),
                    )
                }
                Err(e) => {
                    info!("begin network check error: {:?}", e);
                    return;
                }
            }
        };

        handler
            .replace_state(ReachabilityState {
                internet_available: system_internet,
                gateway_reachable: system_gateway,
                dns_active: system_dns,
                http_active: system_http,
            })
            .await;
    }

    // TODO(https://fxbug.dev/42076412): handle_netcheck_response and handle_network_check_message are missing
    // tests because they reply on NetworkCheckCookie, which cannot be created in the event loop.
    async fn handle_netcheck_response(
        &mut self,
        (cookie, result): (NetworkCheckCookie, NetworkCheckResult),
    ) {
        match self.monitor.resume(cookie, result) {
            Ok(NetworkCheckerOutcome::MustResume) => {}
            Ok(NetworkCheckerOutcome::Complete) => {
                let (system_internet, system_gateway, system_dns, system_http) = {
                    let monitor_state = self.monitor.state();
                    (
                        monitor_state.system_has_internet(),
                        monitor_state.system_has_gateway(),
                        monitor_state.system_has_dns(),
                        monitor_state.system_has_http(),
                    )
                };

                self.handler
                    .replace_state(ReachabilityState {
                        internet_available: system_internet,
                        gateway_reachable: system_gateway,
                        dns_active: system_dns,
                        http_active: system_http,
                    })
                    .await;
            }
            Err(e) => error!("resume network check error: {:?}", e),
        }
    }
}

/// If we encounter an unrecoverable state, log an error and exit.
//
// TODO(https://fxbug.dev/42070352): add a test that works as intended.
fn exit_with_anyhow_error(cause: anyhow::Error) -> ! {
    error!(cause:%; "exiting due to error");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused)]
    use assert_matches::assert_matches as _;
    use fidl_fuchsia_net::{IpAddress, Ipv4Address, Ipv6Address, Subnet};
    use fidl_fuchsia_net_interfaces::{Address, AddressAssignmentState, Event, Properties};
    use net_declare::{std_ip_v4, std_ip_v6};

    fn create_eventloop() -> EventLoop {
        let handler = ReachabilityHandler::new();
        let inspector = fuchsia_inspect::component::inspector();
        let (sender, receiver) =
            futures::channel::mpsc::unbounded::<(NetworkCheckAction, NetworkCheckCookie)>();
        let mut monitor = Monitor::new(sender).expect("failed to create reachability monitor");
        let () = monitor.set_inspector(inspector);

        return EventLoop::new(monitor, handler, receiver, inspector);
    }

    #[fuchsia::test]
    async fn test_handle_interface_watcher_result_ipv4() {
        let mut event_loop = create_eventloop();

        let v4_subnet = Subnet {
            addr: IpAddress::Ipv4(Ipv4Address { addr: std_ip_v4!("192.0.2.1").octets() }),
            prefix_len: 16,
        };

        let addr = Address {
            addr: Some(v4_subnet),
            assignment_state: Some(AddressAssignmentState::Assigned),
            ..Default::default()
        };

        let mut props = Properties {
            id: Some(12345),
            addresses: Some(vec![addr]),
            online: Some(true),
            port_class: Some(fnet_interfaces::PortClass::Device(
                fidl_fuchsia_hardware_network::PortClass::Ethernet,
            )),
            has_default_ipv4_route: Some(true),
            has_default_ipv6_route: Some(true),
            name: Some("IPv4 Reachability Test Interface".to_string()),
            ..Default::default()
        };

        let event_res = Ok(Some(Event::Existing(props.clone()).into()));
        assert_eq!(event_loop.handle_interface_watcher_result(event_res).await, Some(12345));

        props.id = Some(54321);
        let event_res = Ok(Some(Event::Added(props.clone()).into()));
        assert_eq!(event_loop.handle_interface_watcher_result(event_res).await, Some(54321));
    }

    #[fuchsia::test]
    async fn test_handle_interface_watcher_result_ipv6() {
        let mut event_loop = create_eventloop();

        let v6_subnet = Subnet {
            addr: IpAddress::Ipv6(Ipv6Address { addr: std_ip_v6!("2001:db8::1").octets() }),
            prefix_len: 16,
        };

        let addr = Address {
            addr: Some(v6_subnet),
            assignment_state: Some(AddressAssignmentState::Assigned),
            ..Default::default()
        };

        let mut props = Properties {
            id: Some(12345),
            addresses: Some(vec![addr]),
            online: Some(true),
            port_class: Some(fnet_interfaces::PortClass::Device(
                fidl_fuchsia_hardware_network::PortClass::Ethernet,
            )),
            has_default_ipv4_route: Some(true),
            has_default_ipv6_route: Some(true),
            name: Some("IPv6 Reachability Test Interface".to_string()),
            ..Default::default()
        };

        let event_res = Ok(Some(Event::Existing(props.clone()).into()));
        assert_eq!(event_loop.handle_interface_watcher_result(event_res).await, Some(12345));

        props.id = Some(54321);
        let event_res = Ok(Some(Event::Added(props.clone()).into()));
        assert_eq!(event_loop.handle_interface_watcher_result(event_res).await, Some(54321));
    }
}
