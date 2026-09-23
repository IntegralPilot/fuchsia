// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(clippy::unused_async)]

pub mod dig;
pub mod fetch;
mod inspect;
mod neighbor_cache;
pub mod ping;
pub mod route_table;
pub mod telemetry;
pub mod watchdog;

#[cfg(test)]
mod testutil;

use crate::route_table::{Route, RouteTable};
use crate::telemetry::processors::link_properties_state::LinkProperties;
use crate::telemetry::{TelemetryEvent, TelemetrySender};
use anyhow::anyhow;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext::{self as fnet_ext, IpExt};
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fuchsia_async as fasync;
use fuchsia_inspect::{Inspector, Node as InspectNode};
use futures::channel::mpsc;
use inspect::InspectInfo;
use log::{debug, error, info};
use named_timer::DeadlineId;
use net_declare::{fidl_subnet, std_ip};
use net_types::ScopeableAddress as _;
use num_derive::FromPrimitive;
use std::collections::HashSet;
use std::collections::hash_map::{Entry, HashMap};

use std::net::IpAddr;

pub use neighbor_cache::{InterfaceNeighborCache, NeighborCache};

const IPV4_INTERNET_CONNECTIVITY_CHECK_ADDRESS: std::net::IpAddr = std_ip!("8.8.8.8");
const IPV6_INTERNET_CONNECTIVITY_CHECK_ADDRESS: std::net::IpAddr = std_ip!("2001:4860:4860::8888");
const UNSPECIFIED_V4: fidl_fuchsia_net::Subnet = fidl_subnet!("0.0.0.0/0");
const UNSPECIFIED_V6: fidl_fuchsia_net::Subnet = fidl_subnet!("::0/0");
const GSTATIC: &'static str = "www.gstatic.com";
const GENERATE_204: &'static str = "/generate_204";
// Gstatic has a TTL of 300 seconds, therefore, we will perform a lookup every
// 300 seconds since we won't get any better indication of DNS function.
// TODO(https://fxbug.dev/42072067): Dynamically query TTL based on the domain's DNS record
const DNS_PROBE_PERIOD: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(300);

// Timeout ID for the fake clock component that restrains the integration tests from reaching the
// FIDL timeout and subsequently failing. Shared by the eventloop and integration library.
pub const FIDL_TIMEOUT_ID: DeadlineId<'static> =
    DeadlineId::new("reachability", "fidl-request-timeout");

/// `Stats` keeps the monitoring service statistic counters.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    /// `events` is the number of events received.
    pub events: u64,
    /// `state_updates` is the number of times reachability state has changed.
    pub state_updates: HashMap<Id, u64>,
}

// TODO(dpradilla): consider splitting the state in l2 state and l3 state, as there can be multiple
/// `LinkState` represents the layer 2 and layer 3 state
#[derive(Default, Debug, Ord, PartialOrd, Eq, PartialEq, Clone, Copy, FromPrimitive)]
#[repr(u8)]
pub enum LinkState {
    /// State not yet determined.
    #[default]
    None = 1,
    /// Interface no longer present.
    Removed = 5,
    /// Interface is down.
    Down = 10,
    /// Interface is up, no packets seen yet.
    Up = 15,
    /// L3 Interface is up, local neighbors seen.
    Local = 20,
    /// L3 Interface is up, local gateway configured and reachable.
    Gateway = 25,
    /// Expected response seen from reachability test URL.
    Internet = 30,
}

impl LinkState {
    fn log_state_vals_inspect(node: &InspectNode, name: &str) {
        let child = node.create_child(name);
        for i in LinkState::None as u32..=LinkState::Internet as u32 {
            match <LinkState as num_traits::FromPrimitive>::from_u32(i) {
                Some(state) => child.record_string(i.to_string(), format!("{:?}", state)),
                None => (),
            }
        }
        node.record(child);
    }
}

/// `ApplicationState` represents the layer 7 state
#[derive(Default, Debug, Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
pub struct ApplicationState {
    pub dns_resolved: bool,
    pub http_fetch_succeeded: bool,
}

/// `State` represents the reachability state.
#[derive(Default, Debug, Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
pub struct State {
    pub link: LinkState,
    pub application: ApplicationState,
}

impl From<LinkState> for State {
    fn from(link: LinkState) -> Self {
        State { link, ..Default::default() }
    }
}

impl LinkState {
    fn has_interface_up(&self) -> bool {
        match self {
            LinkState::None | LinkState::Removed | LinkState::Down => false,
            LinkState::Up | LinkState::Local | LinkState::Gateway | LinkState::Internet => true,
        }
    }

    fn has_internet(&self) -> bool {
        match self {
            LinkState::None
            | LinkState::Removed
            | LinkState::Down
            | LinkState::Up
            | LinkState::Local
            | LinkState::Gateway => false,
            LinkState::Internet => true,
        }
    }

    fn has_gateway(&self) -> bool {
        match self {
            LinkState::None
            | LinkState::Removed
            | LinkState::Down
            | LinkState::Up
            | LinkState::Local => false,
            LinkState::Gateway | LinkState::Internet => true,
        }
    }
}

impl State {
    fn set_link_state(&mut self, link: LinkState) {
        *self = State { link, ..Default::default() };
    }

    fn has_interface_up(&self) -> bool {
        self.link.has_interface_up()
    }

    fn has_internet(&self) -> bool {
        self.link.has_internet()
    }

    fn has_gateway(&self) -> bool {
        self.link.has_gateway()
    }

    fn has_dns(&self) -> bool {
        self.application.dns_resolved
    }

    fn has_http(&self) -> bool {
        self.application.http_fetch_succeeded
    }
}

impl std::fmt::Display for LinkState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LinkState::None => "None",
            LinkState::Removed => "Removed",
            LinkState::Down => "Down",
            LinkState::Up => "Up",
            LinkState::Local => "Local",
            LinkState::Gateway => "Gateway",
            LinkState::Internet => "Internet",
        })
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (dns: {}, http: {})",
            self.link, self.application.dns_resolved, self.application.http_fetch_succeeded
        )
    }
}

impl std::str::FromStr for LinkState {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "None" => Ok(Self::None),
            "Removed" => Ok(Self::Removed),
            "Down" => Ok(Self::Down),
            "Up" => Ok(Self::Up),
            "Local" => Ok(Self::Local),
            "Gateway" => Ok(Self::Gateway),
            "Internet" => Ok(Self::Internet),
            _ => Err(()),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum Proto {
    IPv4,
    IPv6,
}
impl std::fmt::Display for Proto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Proto::IPv4 => "IPv4",
            Proto::IPv6 => "IPv6",
        })
    }
}

/// A trait for types containing reachability state that should be compared without the timestamp.
trait StateEq {
    /// Returns true iff `self` and `other` have equivalent reachability state.
    fn compare_state(&self, other: &Self) -> bool;
}

/// `StateEvent` records a state and the time it was reached.
// NB PartialEq is derived only for tests to avoid unintentionally making a comparison that
// includes the timestamp.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(PartialEq))]
struct StateEvent {
    /// `state` is the current reachability state.
    state: State,
    /// The time of this event.
    time: fasync::MonotonicInstant,
}

impl StateEvent {
    /// Overwrite `self` with `other` if the state is different, returning the previous and current
    /// values (which may be equal).
    fn update(&mut self, other: Self) -> Delta<Self> {
        let previous = Some(*self);
        if self.state != other.state {
            *self = other;
        }
        Delta { previous, current: *self }
    }
}

impl StateEq for StateEvent {
    fn compare_state(&self, &Self { state, time: _ }: &Self) -> bool {
        self.state == state
    }
}

impl std::fmt::Display for StateEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.state, f)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Delta<T> {
    current: T,
    previous: Option<T>,
}

impl<T: StateEq> Delta<T> {
    fn change_observed(&self) -> bool {
        match &self.previous {
            Some(previous) => !previous.compare_state(&self.current),
            None => true,
        }
    }
}

// NB PartialEq is derived only for tests to avoid unintentionally making a comparison that
// includes the timestamp in `StateEvent`.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
struct StateDelta {
    port: IpVersions<Delta<StateEvent>>,
    system: IpVersions<Delta<SystemState>>,
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct IpVersions<T> {
    ipv4: T,
    ipv6: T,
}

impl<T> IpVersions<T> {
    fn with_version<F: FnMut(Proto, &T)>(&self, mut f: F) {
        let () = f(Proto::IPv4, &self.ipv4);
        let () = f(Proto::IPv6, &self.ipv6);
    }
}

impl IpVersions<Option<SystemState>> {
    fn state(&self) -> IpVersions<Option<State>> {
        IpVersions {
            ipv4: self.ipv4.map(|s| s.state.state),
            ipv6: self.ipv6.map(|s| s.state.state),
        }
    }
}

impl IpVersions<Option<State>> {
    fn has_interface_up(&self) -> bool {
        self.satisfies(State::has_interface_up)
    }

    fn has_internet(&self) -> bool {
        self.satisfies(State::has_internet)
    }

    fn has_dns(&self) -> bool {
        self.satisfies(State::has_dns)
    }

    fn has_http(&self) -> bool {
        self.satisfies(State::has_http)
    }

    fn has_gateway(&self) -> bool {
        self.satisfies(State::has_gateway)
    }

    fn satisfies<F>(&self, f: F) -> bool
    where
        F: Fn(&State) -> bool,
    {
        return [self.ipv4, self.ipv6].iter().filter_map(|state| state.as_ref()).any(f);
    }
}

type Id = u64;

// NB PartialEq is derived only for tests to avoid unintentionally making a comparison that
// includes the timestamp in `StateEvent`.
#[derive(Copy, Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
struct SystemState {
    id: Id,
    state: StateEvent,
}

impl SystemState {
    fn max(self, other: Self) -> Self {
        if other.state.state > self.state.state { other } else { self }
    }
}

impl StateEq for SystemState {
    fn compare_state(&self, &Self { id, state: StateEvent { state, time: _ } }: &Self) -> bool {
        self.id == id && self.state.state == state
    }
}

impl std::fmt::Display for SystemState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} via Interface [{}]", self.state, self.id)
    }
}

/// `StateInfo` keeps the reachability state.
// NB PartialEq is derived only for tests to avoid unintentionally making a comparison that
// includes the timestamp in `StateEvent`.
#[derive(Debug, Default, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub struct StateInfo {
    /// Mapping from interface ID to reachability information.
    per_interface: HashMap<Id, IpVersions<StateEvent>>,
    /// Interface IDs with the best reachability state per IP version.
    system: IpVersions<Option<Id>>,
}

impl StateInfo {
    /// Get the reachability info associated with an interface.
    fn get(&self, id: Id) -> Option<&IpVersions<StateEvent>> {
        self.per_interface.get(&id)
    }

    /// Get the system-wide IPv4 reachability info.
    fn get_system_ipv4(&self) -> Option<SystemState> {
        self.system.ipv4.map(|id| SystemState {
            id,
            state: self
                .get(id)
                .unwrap_or_else(|| {
                    panic!("inconsistent system IPv4 state: no interface with ID {:?}", id)
                })
                .ipv4,
        })
    }

    /// Get the system-wide IPv6 reachability info.
    fn get_system_ipv6(&self) -> Option<SystemState> {
        self.system.ipv6.map(|id| SystemState {
            id,
            state: self
                .get(id)
                .unwrap_or_else(|| {
                    panic!("inconsistent system IPv6 state: no interface with ID {:?}", id)
                })
                .ipv6,
        })
    }

    fn get_system(&self) -> IpVersions<Option<SystemState>> {
        IpVersions { ipv4: self.get_system_ipv4(), ipv6: self.get_system_ipv6() }
    }

    pub fn system_has_internet(&self) -> bool {
        self.get_system().state().has_internet()
    }

    pub fn system_has_gateway(&self) -> bool {
        self.get_system().state().has_gateway()
    }

    pub fn system_has_dns(&self) -> bool {
        self.get_system().state().has_dns()
    }

    pub fn system_has_http(&self) -> bool {
        self.get_system().state().has_http()
    }

    /// Report the duration of the current state for each interface and each protocol.
    fn report(&self) {
        let time = fasync::MonotonicInstant::now();
        debug!("system reachability state IPv4 {:?}", self.get_system_ipv4());
        debug!("system reachability state IPv6 {:?}", self.get_system_ipv6());
        for (id, IpVersions { ipv4, ipv6 }) in self.per_interface.iter() {
            debug!(
                "reachability state {:?} IPv4 {:?} with duration {:?}",
                id,
                ipv4,
                time - ipv4.time
            );
            debug!(
                "reachability state {:?} IPv6 {:?} with duration {:?}",
                id,
                ipv6,
                time - ipv6.time
            );
        }
    }

    /// Update interface `id` with its new reachability info.
    ///
    /// Returns the protocols and their new reachability states iff a change was observed.
    fn update(&mut self, id: Id, new_reachability: IpVersions<StateEvent>) -> StateDelta {
        let previous_system_ipv4 = self.get_system_ipv4();
        let previous_system_ipv6 = self.get_system_ipv6();
        let port = match self.per_interface.entry(id) {
            Entry::Occupied(mut occupied) => {
                let IpVersions { ipv4, ipv6 } = occupied.get_mut();
                let IpVersions { ipv4: new_ipv4, ipv6: new_ipv6 } = new_reachability;

                IpVersions { ipv4: ipv4.update(new_ipv4), ipv6: ipv6.update(new_ipv6) }
            }
            Entry::Vacant(vacant) => {
                let IpVersions { ipv4, ipv6 } = vacant.insert(new_reachability);
                IpVersions {
                    ipv4: Delta { previous: None, current: *ipv4 },
                    ipv6: Delta { previous: None, current: *ipv6 },
                }
            }
        };

        let IpVersions { ipv4: system_ipv4, ipv6: system_ipv6 } = self.per_interface.iter().fold(
            {
                let IpVersions {
                    ipv4: Delta { previous: _, current: curr_ipv4 },
                    ipv6: Delta { previous: _, current: curr_ipv6 },
                } = port;
                // Prioritize the `previous` system state as the initial `SystemState` when it is
                // present and holds state for a different interface than the one we're updating.
                // This prevents the `SystemState` from flipping between interfaces when multiple
                // interfaces have the same state.
                let ipv4 = previous_system_ipv4
                    .map(|prev| {
                        if prev.id != id {
                            SystemState { id: prev.id, state: prev.state }
                        } else {
                            SystemState { id, state: curr_ipv4 }
                        }
                    })
                    .unwrap_or(SystemState { id, state: curr_ipv4 });
                let ipv6 = previous_system_ipv6
                    .map(|prev| {
                        if prev.id != id {
                            SystemState { id: prev.id, state: prev.state }
                        } else {
                            SystemState { id, state: curr_ipv6 }
                        }
                    })
                    .unwrap_or(SystemState { id, state: curr_ipv6 });
                IpVersions { ipv4, ipv6 }
            },
            |IpVersions { ipv4: system_ipv4, ipv6: system_ipv6 },
             (&id, &IpVersions { ipv4, ipv6 })| {
                IpVersions {
                    ipv4: system_ipv4.max(SystemState { id, state: ipv4 }),
                    ipv6: system_ipv6.max(SystemState { id, state: ipv6 }),
                }
            },
        );

        self.system = IpVersions { ipv4: Some(system_ipv4.id), ipv6: Some(system_ipv6.id) };

        StateDelta {
            port,
            system: IpVersions {
                ipv4: Delta { previous: previous_system_ipv4, current: system_ipv4 },
                ipv6: Delta { previous: previous_system_ipv6, current: system_ipv6 },
            },
        }
    }
}

/// Provides a view into state for a specific system interface.
#[derive(Copy, Clone, Debug)]
pub struct InterfaceView<'a> {
    pub properties: &'a fnet_interfaces_ext::Properties<fnet_interfaces_ext::DefaultInterest>,
    pub routes: &'a RouteTable,
    pub neighbors: Option<&'a InterfaceNeighborCache>,
}

/// `NetworkCheckerOutcome` contains values indicating whether a network check completed or needs
/// resumption.
#[derive(Debug)]
pub enum NetworkCheckerOutcome {
    /// The network check must be resumed via a call to `resume` to complete.
    MustResume,
    /// The network check is finished and the reachability state for the specified interface has
    /// been updated. A new network check can begin on the same interface via `begin`.
    Complete,
}

/// A Network Checker is a re-entrant, asynchronous state machine that monitors availability of
/// networks over a given network interface.
pub trait NetworkChecker {
    /// `begin` starts a re-entrant, asynchronous network check on the supplied interface. It
    /// returns whether the network check was completed, must be resumed, or if the supplied
    /// interface already had an ongoing network check.
    fn begin(&mut self, view: InterfaceView<'_>) -> Result<NetworkCheckerOutcome, anyhow::Error>;

    /// `resume` continues a network check that was not yet completed.
    fn resume(
        &mut self,
        cookie: NetworkCheckCookie,
        result: NetworkCheckResult,
    ) -> Result<NetworkCheckerOutcome, anyhow::Error>;
}

// States involved in `Monitor`'s implementation of NetworkChecker.
#[derive(Debug, Default)]
enum NetworkCheckState {
    // `Begin` starts a new network check. This state analyzes link properties. It can transition
    // to `PingGateway` when a default gateway is configured on the interface, to `PingInternet`
    // when off-link routes are configured but no default gateway, and `Idle` if analyzing link
    // properties allows determining that connectivity past the local network is not possible.
    #[default]
    Begin,
    // `PingGateway` sends a ping to each of the available gateways with a default route. It can
    // transition to `PingInternet` when a healthy gateway is detected through neighbor discovery,
    // or when at least one gateway ping successfully returns, and `Idle` if no healthy gateway is
    // detected and no gateway pings successfully return.
    PingGateway,
    // `PingInternet` sends a ping to an IPv4 and IPv6 external address. It can only transition to
    // `ResolveDns` after it has completed internet pings.
    PingInternet,
    // `ResolveDns` makes a DNS request for the provided domain and then transitions to `FetchHttp`
    // after it has completed. If DNS_PROBE_PERIOD has not passed, the results will still be
    // cached, and this will transition immediately to `FetchHttp`.
    ResolveDns,
    // `FetchHttp` fetches a URL over http. It can only transition to `Idle` after it has
    // completed all of the http requests.
    FetchHttp,
    // `Idle` terminates a network check. The system is ready to begin processing another network
    // check for interface associated with this check.
    Idle,
}
impl std::fmt::Display for NetworkCheckState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkCheckState::Begin => write!(f, "Begin"),
            NetworkCheckState::PingGateway => write!(f, "Ping Gateway"),
            NetworkCheckState::PingInternet => write!(f, "Ping Internet"),
            NetworkCheckState::ResolveDns => write!(f, "Resolve DNS"),
            NetworkCheckState::FetchHttp => write!(f, "Fetch URL"),
            NetworkCheckState::Idle => write!(f, "Idle"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedIps {
    v4: Vec<std::net::Ipv4Addr>,
    v6: Vec<std::net::Ipv6Addr>,
}

struct PersistentNetworkCheckContext {
    // Map of resolved IP addresses indexed by domain name.
    resolved_addrs: HashMap<String, ResolvedIps>,
    // Dns Resolve Time
    resolved_time: zx::MonotonicInstant,
    // Context about the interface, that enables telemetry.
    telemetry: TelemetryContext,
}

impl Default for PersistentNetworkCheckContext {
    fn default() -> Self {
        Self {
            resolved_addrs: Default::default(),
            resolved_time: zx::MonotonicInstant::INFINITE_PAST,
            telemetry: Default::default(),
        }
    }
}

impl From<TelemetryContext> for PersistentNetworkCheckContext {
    fn from(value: TelemetryContext) -> Self {
        Self {
            resolved_addrs: Default::default(),
            resolved_time: zx::MonotonicInstant::INFINITE_PAST,
            telemetry: value,
        }
    }
}

// Information about the interface that is important for telemetry,
// and is not tied to a specific instance of a network check.
#[derive(Clone, Default)]
struct TelemetryContext {
    // The interface identifiers derived from the interface's PortClass. Used
    // to determine which TimeSeries are applicable to the current interface.
    interface_identifiers: Vec<telemetry::processors::InterfaceIdentifier>,
    has_v4_address: bool,
    has_default_ipv4_route: bool,
    has_v6_address: bool,
    has_default_ipv6_route: bool,
}

impl TelemetryContext {
    fn new(
        port_class: fnet_interfaces_ext::PortClass,
        addresses: &Vec<fnet_interfaces_ext::Address<fnet_interfaces_ext::DefaultInterest>>,
        has_default_ipv4_route: bool,
        has_default_ipv6_route: bool,
    ) -> Self {
        let interface_identifiers = telemetry::processors::identifiers_from_port_class(port_class);
        // Whether the interface has a globally routable v4 / v6 address.
        // v6 address must not be link local.
        let (has_v4_address, has_v6_address) = {
            addresses.iter().fold((false, false), |(mut has_v4, mut has_v6), addr| {
                match addr.addr.addr {
                    fnet::IpAddress::Ipv4(_) => {
                        has_v4 = true;
                    }
                    fnet::IpAddress::Ipv6(v6) => {
                        has_v6 = has_v6 || !v6.is_unicast_link_local();
                    }
                };
                (has_v4, has_v6)
            })
        };
        Self {
            interface_identifiers,
            has_v4_address,
            has_default_ipv4_route,
            has_v6_address,
            has_default_ipv6_route,
        }
    }
}

// Contains all information related to a network check on an interface.
struct NetworkCheckContext {
    // The current status of the state machine.
    checker_state: NetworkCheckState,
    // The list of addresses to ping (either gateway or internet).
    ping_addrs: Vec<std::net::SocketAddr>,
    // The quantity of pings sent.
    pings_expected: usize,
    // The quantity of pings that have been received.
    pings_completed: usize,
    // The quantity of fetches that have been completed.
    fetches_expected: usize,
    // The quantity of fetches that have been completed.
    fetches_completed: usize,
    // The current calculated state.
    discovered_state: IpVersions<State>,
    // Whether the network check should ping internet regardless of if the gateway pings fail.
    always_ping_internet: bool,
    // Whether an online router was discoverable via neighbor discovery.
    router_discoverable: IpVersions<bool>,
    // Whether the gateway successfully responded to pings.
    gateway_pingable: IpVersions<bool>,
    // Context that persists between check cycles
    persistent_context: PersistentNetworkCheckContext,
    // TODO(https://fxbug.dev/42074525): Add tombstone marker to inform NetworkCheck that the interface has
    // been removed and we no longer need to run checks on this interface. This can occur when
    // receiving an interface removed event, but a network check for that interface is still in
    // progress.
}

impl NetworkCheckContext {
    fn set_global_link_state(&mut self, link: LinkState) {
        self.discovered_state.ipv4.set_link_state(link);
        self.discovered_state.ipv6.set_link_state(link);
    }

    fn initiate_ping(
        &mut self,
        id: Id,
        interface_name: &str,
        network_check_sender: &mpsc::UnboundedSender<(NetworkCheckAction, NetworkCheckCookie)>,
        new_state: NetworkCheckState,
        addrs: Vec<std::net::SocketAddr>,
    ) {
        self.checker_state = new_state;
        self.ping_addrs = addrs;
        self.pings_expected = self.ping_addrs.len();
        self.pings_completed = 0;
        self.ping_addrs
            .iter()
            .map(|addr| {
                let action = NetworkCheckAction::Ping(PingParameters {
                    interface_name: interface_name.to_string(),
                    addr: addr.clone(),
                });
                (action, NetworkCheckCookie { id })
            })
            .for_each(|message| match network_check_sender.unbounded_send(message) {
                Ok(()) => {}
                Err(e) => {
                    debug!("unable to send network check internet msg: {:?}", e)
                }
            });
    }
}

impl Default for NetworkCheckContext {
    // Create a context for an interface's network check.
    fn default() -> Self {
        NetworkCheckContext {
            checker_state: Default::default(),
            ping_addrs: Vec::new(),
            pings_expected: 0usize,
            pings_completed: 0usize,
            fetches_expected: 0usize,
            fetches_completed: 0usize,
            discovered_state: IpVersions {
                ipv4: State { link: LinkState::None, ..Default::default() },
                ipv6: State { link: LinkState::None, ..Default::default() },
            },
            always_ping_internet: true,
            router_discoverable: Default::default(),
            gateway_pingable: Default::default(),
            persistent_context: Default::default(),
        }
    }
}

impl From<TelemetryContext> for NetworkCheckContext {
    fn from(value: TelemetryContext) -> Self {
        NetworkCheckContext {
            persistent_context: PersistentNetworkCheckContext::from(value),
            ..Default::default()
        }
    }
}

/// NetworkCheckCookie is an opaque type used to continue an asynchronous network check.
#[derive(Clone)]
pub struct NetworkCheckCookie {
    /// The interface id.
    id: Id,
}

#[derive(Debug)]
pub enum NetworkCheckResult {
    Ping { parameters: PingParameters, result: Result<(), ping::PingError> },
    ResolveDns { parameters: ResolveDnsParameters, ips: Option<ResolvedIps> },
    Fetch { parameters: FetchParameters, result: Result<u16, fetch::FetchError> },
}

#[derive(Debug, Clone)]
pub struct PingParameters {
    /// The name of the interface sending the ping.
    pub interface_name: std::string::String,
    /// The address to ping.
    pub addr: std::net::SocketAddr,
}

#[derive(Debug, Clone)]
pub struct ResolveDnsParameters {
    /// The name of the interface sending the ping.
    pub interface_name: std::string::String,
    /// The domain to resolve.
    pub domain: String,
}

#[derive(Debug, Clone)]
pub struct FetchParameters {
    /// The name of the interface sending the ping.
    pub interface_name: std::string::String,
    /// The http domain, sent with the Host header to the server.
    pub domain: std::string::String,
    /// The DNS Resolved IP address for the fetch server.
    pub ip: std::net::IpAddr,
    /// Path to send request to.
    pub path: String,
    /// The expected HTTP status codes.
    pub expected_statuses: Vec<u16>,
}

impl NetworkCheckResult {
    fn interface_name(&self) -> &str {
        match self {
            NetworkCheckResult::Ping {
                parameters: PingParameters { interface_name, .. }, ..
            } => interface_name,
            NetworkCheckResult::ResolveDns {
                parameters: ResolveDnsParameters { interface_name, .. },
                ..
            } => interface_name,
            NetworkCheckResult::Fetch {
                parameters: FetchParameters { interface_name, .. },
                ..
            } => interface_name,
        }
    }

    fn ping_result(self) -> Option<(PingParameters, Result<(), ping::PingError>)> {
        match self {
            NetworkCheckResult::Ping { parameters, result } => Some((parameters, result)),
            _ => None,
        }
    }

    fn resolve_dns_result(self) -> Option<(ResolveDnsParameters, Option<ResolvedIps>)> {
        match self {
            NetworkCheckResult::ResolveDns { parameters, ips } => Some((parameters, ips)),
            _ => None,
        }
    }

    fn fetch_result(self) -> Option<(FetchParameters, Result<u16, fetch::FetchError>)> {
        match self {
            NetworkCheckResult::Fetch { parameters, result } => Some((parameters, result)),
            _ => None,
        }
    }
}

/// `NetworkCheckAction` describes the action to be completed before resuming the network check.
#[derive(Debug, Clone)]
pub enum NetworkCheckAction {
    Ping(PingParameters),
    ResolveDns(ResolveDnsParameters),
    Fetch(FetchParameters),
}

pub trait TimeProvider {
    fn now(&mut self) -> zx::MonotonicInstant;
}

#[derive(Debug, Default)]
pub struct MonotonicInstant;
impl TimeProvider for MonotonicInstant {
    fn now(&mut self) -> zx::MonotonicInstant {
        zx::MonotonicInstant::get()
    }
}

/// `Monitor` monitors the reachability state.
pub struct Monitor<Time = MonotonicInstant> {
    state: StateInfo,
    stats: Stats,
    inspector: Option<&'static Inspector>,
    system_node: Option<InspectInfo>,
    nodes: HashMap<Id, InspectInfo>,
    telemetry_sender: Option<TelemetrySender>,
    /// In `Monitor`'s implementation of NetworkChecker, the sender is used to dispatch network
    /// checks to the eventloop to be run concurrently. The network check then will be resumed with
    /// the result of the `NetworkCheckAction`.
    network_check_sender: mpsc::UnboundedSender<(NetworkCheckAction, NetworkCheckCookie)>,
    interface_context: HashMap<Id, NetworkCheckContext>,
    time_provider: Time,
}

impl<Time: TimeProvider + Default> Monitor<Time> {
    /// Create the monitoring service.
    pub fn new(
        network_check_sender: mpsc::UnboundedSender<(NetworkCheckAction, NetworkCheckCookie)>,
    ) -> anyhow::Result<Self> {
        Ok(Monitor {
            state: Default::default(),
            stats: Default::default(),
            inspector: None,
            system_node: None,
            nodes: HashMap::new(),
            telemetry_sender: None,
            network_check_sender,
            interface_context: HashMap::new(),
            time_provider: Default::default(),
        })
    }
}

impl<Time> Monitor<Time> {
    /// Create the monitoring service.
    pub fn new_with_time_provider(
        network_check_sender: mpsc::UnboundedSender<(NetworkCheckAction, NetworkCheckCookie)>,
        time_provider: Time,
    ) -> anyhow::Result<Self> {
        Ok(Monitor {
            state: Default::default(),
            stats: Default::default(),
            inspector: None,
            system_node: None,
            nodes: HashMap::new(),
            telemetry_sender: None,
            network_check_sender,
            interface_context: HashMap::new(),
            time_provider,
        })
    }
}

impl<Time: TimeProvider> Monitor<Time> {
    pub fn state(&self) -> &StateInfo {
        &self.state
    }

    /// Reports all information.
    pub fn report_state(&self) {
        self.state.report();
        debug!("reachability stats {:?}", self.stats);
    }

    /// Sets the inspector.
    pub fn set_inspector(&mut self, inspector: &'static Inspector) {
        self.inspector = Some(inspector);

        let system_node = InspectInfo::new(inspector.root(), "system", "");
        self.system_node = Some(system_node);

        LinkState::log_state_vals_inspect(inspector.root(), "state_vals");
    }

    pub fn set_telemetry_sender(&mut self, telemetry_sender: TelemetrySender) {
        self.telemetry_sender = Some(telemetry_sender);
    }

    fn interface_node(&mut self, id: Id, name: &str) -> Option<&mut InspectInfo> {
        self.inspector.map(move |inspector| {
            self.nodes.entry(id).or_insert_with_key(|id| {
                InspectInfo::new(inspector.root(), &format!("{:?}", id), name)
            })
        })
    }

    fn update_state_from_context(
        &mut self,
        id: Id,
        name: &str,
    ) -> Result<NetworkCheckerOutcome, anyhow::Error> {
        let ctx = self.interface_context.get_mut(&id).ok_or_else(|| {
            anyhow!(
                "attempting to update state with context but context for id {} does not exist",
                id
            )
        })?;

        ctx.checker_state = NetworkCheckState::Idle;

        if let Some(IpVersions { ipv4, ipv6 }) = self.state.get(id) {
            if ipv4.state.link == LinkState::Removed && ipv6.state.link == LinkState::Removed {
                debug!("interface {} was removed, skipping state update", id);
                return Ok(NetworkCheckerOutcome::Complete);
            }
        }

        let info = IpVersions {
            ipv4: StateEvent {
                state: ctx.discovered_state.ipv4,
                time: fasync::MonotonicInstant::now(),
            },
            ipv6: StateEvent {
                state: ctx.discovered_state.ipv6,
                time: fasync::MonotonicInstant::now(),
            },
        };

        let gateway_event_v4 = TelemetryEvent::GatewayProbe {
            gateway_discoverable: ctx.router_discoverable.ipv4,
            gateway_pingable: ctx.gateway_pingable.ipv4,
            internet_available: ctx.discovered_state.ipv4.has_internet(),
        };
        let gateway_event_v6 = TelemetryEvent::GatewayProbe {
            gateway_discoverable: ctx.router_discoverable.ipv6,
            gateway_pingable: ctx.gateway_pingable.ipv6,
            internet_available: ctx.discovered_state.ipv6.has_internet(),
        };

        if let Some(telemetry_sender) = &mut self.telemetry_sender {
            telemetry_sender.send(gateway_event_v4);
            telemetry_sender.send(gateway_event_v6);
            telemetry_sender.send(TelemetryEvent::SystemStateUpdate {
                update: telemetry::SystemStateUpdate {
                    system_state: self.state.get_system().state(),
                },
            });
            let telemetry_context = &ctx.persistent_context.telemetry;
            let interface_identifiers = &telemetry_context.interface_identifiers;
            telemetry_sender.send(TelemetryEvent::LinkPropertiesUpdate {
                interface_identifiers: interface_identifiers.clone(),
                link_properties: IpVersions {
                    ipv4: LinkProperties {
                        has_address: telemetry_context.has_v4_address,
                        has_default_route: telemetry_context.has_default_ipv4_route,
                        has_dns: ctx.discovered_state.ipv4.has_dns(),
                        has_http_reachability: ctx.discovered_state.ipv4.has_http(),
                    },
                    ipv6: LinkProperties {
                        has_address: telemetry_context.has_v6_address,
                        has_default_route: telemetry_context.has_default_ipv6_route,
                        has_dns: ctx.discovered_state.ipv6.has_dns(),
                        has_http_reachability: ctx.discovered_state.ipv6.has_http(),
                    },
                },
            });
            telemetry_sender.send(TelemetryEvent::LinkStateUpdate {
                interface_identifiers: interface_identifiers.clone(),
                link_state: IpVersions {
                    ipv4: ctx.discovered_state.ipv4.link,
                    ipv6: ctx.discovered_state.ipv6.link,
                },
            });
        }

        let () = self.update_state(id, &name, info);
        Ok(NetworkCheckerOutcome::Complete)
    }

    /// Update state based on the new reachability info.
    fn update_state(&mut self, id: Id, name: &str, reachability: IpVersions<StateEvent>) {
        let StateDelta { port, system } = self.state.update(id, reachability);

        let () = port.with_version(|proto, delta| {
            if delta.change_observed() {
                let &Delta { previous, current } = delta;
                if let Some(previous) = previous {
                    info!(
                        "Interface [{}] updated --> {} current: {}, previous: {}",
                        id, proto, current, previous
                    );
                } else {
                    info!("New interface [{}] --> {}: {}", id, proto, current);
                }
                let () = log_state(self.interface_node(id, name), proto, current.state);
                *self.stats.state_updates.entry(id).or_insert(0) += 1;
            }
        });

        let () = system.with_version(|proto, delta| {
            if delta.change_observed() {
                let &Delta { previous, current } = delta;
                if let Some(previous) = previous {
                    info!(
                        "System reachability updated --> {} current: {}, previous: {}",
                        proto, current, previous
                    );
                } else {
                    info!("Initial system reachability --> {}: {}", proto, current);
                }
                let () = log_state(self.system_node.as_mut(), proto, current.state.state);
            }
        });
    }

    /// Handle an interface removed event.
    pub fn handle_interface_removed(
        &mut self,
        fnet_interfaces_ext::Properties { id, name, .. }: fnet_interfaces_ext::Properties<
            fnet_interfaces_ext::DefaultInterest,
        >,
    ) {
        let time = fasync::MonotonicInstant::now();
        if let Some(mut reachability) = self.state.get(id.into()).cloned() {
            reachability.ipv4 = StateEvent {
                state: State { link: LinkState::Removed, ..Default::default() },
                time,
            };
            reachability.ipv6 = StateEvent {
                state: State { link: LinkState::Removed, ..Default::default() },
                time,
            };
            let () = self.update_state(id.into(), &name, reachability);
        }
    }

    fn handle_fetch_success(ctx: &mut NetworkCheckContext, ip: std::net::IpAddr) {
        match ctx.checker_state {
            NetworkCheckState::FetchHttp => match ip {
                IpAddr::V4(_) => {
                    ctx.discovered_state.ipv4.application.http_fetch_succeeded = true;
                }
                IpAddr::V6(_) => {
                    ctx.discovered_state.ipv6.application.http_fetch_succeeded = true;
                }
            },
            NetworkCheckState::PingGateway
            | NetworkCheckState::PingInternet
            | NetworkCheckState::Begin
            | NetworkCheckState::Idle
            | NetworkCheckState::ResolveDns => {
                panic!("continue check had an invalid state")
            }
        }
    }

    fn handle_ping_success(ctx: &mut NetworkCheckContext, addr: &std::net::SocketAddr) {
        match ctx.checker_state {
            NetworkCheckState::PingGateway => match addr {
                std::net::SocketAddr::V4 { .. } => {
                    ctx.gateway_pingable.ipv4 = true;
                    ctx.discovered_state.ipv4.set_link_state(LinkState::Gateway);
                }
                std::net::SocketAddr::V6 { .. } => {
                    ctx.gateway_pingable.ipv6 = true;
                    ctx.discovered_state.ipv6.set_link_state(LinkState::Gateway);
                }
            },
            NetworkCheckState::PingInternet => match addr {
                std::net::SocketAddr::V4 { .. } => {
                    ctx.discovered_state.ipv4.set_link_state(LinkState::Internet)
                }
                std::net::SocketAddr::V6 { .. } => {
                    ctx.discovered_state.ipv6.set_link_state(LinkState::Internet)
                }
            },
            NetworkCheckState::FetchHttp
            | NetworkCheckState::Begin
            | NetworkCheckState::Idle
            | NetworkCheckState::ResolveDns => {
                panic!("continue check had an invalid state")
            }
        }
    }
}

impl<Time: TimeProvider> NetworkChecker for Monitor<Time> {
    fn begin(
        &mut self,
        InterfaceView {
            properties:
                &fnet_interfaces_ext::Properties {
                    id,
                    ref name,
                    port_class,
                    online,
                    ref addresses,
                    has_default_ipv4_route,
                    has_default_ipv6_route,
                    port_identity_koid: _,
                },
            routes,
            neighbors,
        }: InterfaceView<'_>,
    ) -> Result<NetworkCheckerOutcome, anyhow::Error> {
        let id = Id::from(id);
        // Check to see if the current interface view is already in the map. If its state is not
        // Idle then another network check for the interface is already processing. In this case,
        // drop the `begin` request and log it.
        // It is expected for this to occur when an interface is experiencing many events in a
        // short period of time, for example changing between online and offline multiple times
        // over the span of a few seconds. It is safe that this happens, as the system is
        // eventually consistent.
        let telemetry_context = TelemetryContext::new(
            port_class,
            &addresses,
            has_default_ipv4_route,
            has_default_ipv6_route,
        );
        let ctx = self
            .interface_context
            .entry(id)
            .or_insert_with(|| NetworkCheckContext::from(telemetry_context.clone()));

        match ctx.checker_state {
            NetworkCheckState::Begin => {}
            NetworkCheckState::Idle => {
                let mut new_ctx = NetworkCheckContext::default();
                // Copy persistent context context between passes
                std::mem::swap(&mut new_ctx.persistent_context, &mut ctx.persistent_context);
                // The telemetry should be updated based on the Properties passed into `begin`.
                new_ctx.persistent_context.telemetry = telemetry_context;
                *ctx = new_ctx;
            }
            NetworkCheckState::PingGateway
            | NetworkCheckState::PingInternet
            | NetworkCheckState::FetchHttp
            | NetworkCheckState::ResolveDns => {
                // Update the Properties for the TelemetryContext so that the LinkProperties can
                // be reported properly.
                ctx.persistent_context.telemetry = telemetry_context;
                return Err(anyhow!("skipped, non-idle state found on Interface [{id}]"));
            }
        }

        if !online {
            ctx.set_global_link_state(LinkState::Down);
            return self.update_state_from_context(id, name);
        }

        ctx.set_global_link_state(LinkState::Up);

        // TODO(https://fxbug.dev/42154208) Check if packet count has increased, and if so upgrade
        // the state to LinkLayerUp.
        let device_routes: Vec<_> = routes.device_routes(id).collect();

        let neighbor_scan_health = scan_neighbor_health(neighbors, &device_routes);

        let has_route = IpVersions {
            ipv4: device_routes
                .iter()
                .any(|route| matches!(route.destination.addr, fnet::IpAddress::Ipv4(_))),
            ipv6: device_routes
                .iter()
                .any(|route| matches!(route.destination.addr, fnet::IpAddress::Ipv6(_))),
        };

        if neighbor_scan_health.ipv4 == NeighborHealthScanResult::NoneHealthy
            && neighbor_scan_health.ipv6 == NeighborHealthScanResult::NoneHealthy
        {
            if !has_route.ipv4 && !has_route.ipv6 {
                // Both protocols are `Up`, no need to perform any further calculations.
                return self.update_state_from_context(id, name);
            }

            // When a router is not discoverable via ND, the internet should only be pinged
            // if the gateway ping succeeds.
            ctx.always_ping_internet = false;
        }
        if has_route.ipv4 || neighbor_scan_health.ipv4.is_healthy() {
            ctx.discovered_state.ipv4.set_link_state(LinkState::Local);
        }
        if has_route.ipv6 || neighbor_scan_health.ipv6.is_healthy() {
            ctx.discovered_state.ipv6.set_link_state(LinkState::Local);
        }

        let gateway_ping_addrs = device_routes
            .iter()
            .filter_map(move |Route { destination, outbound_interface, next_hop }| {
                if *destination != UNSPECIFIED_V4 && *destination != UNSPECIFIED_V6 {
                    return None;
                }
                next_hop.and_then(|next_hop| {
                    let fnet_ext::IpAddress(next_hop) = next_hop.into();
                    match next_hop.into() {
                        std::net::IpAddr::V4(v4) => {
                            Some(std::net::SocketAddr::V4(std::net::SocketAddrV4::new(v4, 0)))
                        }
                        std::net::IpAddr::V6(v6) => match (*outbound_interface).try_into() {
                            Err(std::num::TryFromIntError { .. }) => {
                                error!("device id {} doesn't fit in u32", outbound_interface);
                                None
                            }
                            Ok(device_id) => {
                                if device_id == 0
                                    && net_types::ip::Ipv6Addr::from_bytes(v6.octets()).scope()
                                        != net_types::ip::Ipv6Scope::Global
                                {
                                    None
                                } else {
                                    Some(std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                                        v6, 0, 0, device_id,
                                    )))
                                }
                            }
                        },
                    }
                })
            })
            .map(|next_hop| next_hop)
            .collect::<Vec<_>>();

        // A router is determined to be discoverable if it is online (marked as healthy by ND).
        ctx.router_discoverable = IpVersions {
            ipv4: neighbor_scan_health.ipv4 == NeighborHealthScanResult::HealthyRouter,
            ipv6: neighbor_scan_health.ipv6 == NeighborHealthScanResult::HealthyRouter,
        };
        if gateway_ping_addrs.is_empty() {
            // When there are no gateway addresses to ping, the gateway is not pingable. The list
            // of Gateway addresses is obtained by filtering the default IPv4 and IPv6 routes.

            // We use the discovery of an online router as a separate opportunity to calculate
            // internet reachability because of the potential for various network configurations.
            // One potential case involves having an AP operating in bridge mode, and having a
            // separate device host DHCP. In this situation, it's possible to have routes that can
            // be used to send pings to the internet that are not default routes. In another case,
            // a router may have a very specific target prefix that is routable. The device could
            // access a remote set of addresses through this local router and not view it as being
            // accessed through a default route.
            if neighbor_scan_health.ipv4 == NeighborHealthScanResult::HealthyRouter
                || neighbor_scan_health.ipv6 == NeighborHealthScanResult::HealthyRouter
            {
                // Setup to ping internet addresses, skipping over gateway pings.
                // Internet can be pinged when either an online router is discovered or the gateway
                // is pingable. In this case, the discovery of a router enables the internet ping.
                // TODO(https://fxbug.dev/42074958): Create an occurrence metric for this case
                ctx.initiate_ping(
                    id,
                    name,
                    &self.network_check_sender,
                    NetworkCheckState::PingInternet,
                    [
                        IPV4_INTERNET_CONNECTIVITY_CHECK_ADDRESS,
                        IPV6_INTERNET_CONNECTIVITY_CHECK_ADDRESS,
                    ]
                    .into_iter()
                    .map(|ip| std::net::SocketAddr::new(ip, 0))
                    .collect(),
                );
            } else {
                // The router is not online and the gateway cannot be pinged; therefore, the
                // internet pings can be skipped and the final reachability state can be
                // determined.
                return self.update_state_from_context(id, name);
            }
        } else {
            // Setup to ping gateway addresses.
            if neighbor_scan_health.ipv4.is_healthy_router() {
                ctx.discovered_state.ipv4.set_link_state(LinkState::Gateway);
            }
            if neighbor_scan_health.ipv6.is_healthy_router() {
                ctx.discovered_state.ipv6.set_link_state(LinkState::Gateway);
            }
            ctx.initiate_ping(
                id,
                name,
                &self.network_check_sender,
                NetworkCheckState::PingGateway,
                gateway_ping_addrs,
            );
        }
        Ok(NetworkCheckerOutcome::MustResume)
    }

    fn resume(
        &mut self,
        cookie: NetworkCheckCookie,
        result: NetworkCheckResult,
    ) -> Result<NetworkCheckerOutcome, anyhow::Error> {
        let ctx = self.interface_context.get_mut(&cookie.id).ok_or_else(|| {
            anyhow!("resume: interface id {} should already exist in map", cookie.id)
        })?;
        let interface_name = result.interface_name().to_string();
        match ctx.checker_state {
            NetworkCheckState::Begin | NetworkCheckState::Idle => {
                return Err(anyhow!(
                    "skipped, idle state found in resume for interface {}",
                    cookie.id
                ));
            }
            NetworkCheckState::PingGateway | NetworkCheckState::PingInternet => {
                let (parameters, result) = result.ping_result().ok_or_else(|| {
                    anyhow!("resume: mismatched state and result {interface_name} ({})", cookie.id)
                })?;
                ctx.pings_completed = ctx.pings_completed + 1;

                // Grab `ping_is_ok` before `result` is moved into `telemetry_sender.send`.
                let ping_is_ok = result.is_ok();

                if let Some(telemetry_sender) = &mut self.telemetry_sender {
                    let interface_identifiers =
                        ctx.persistent_context.telemetry.interface_identifiers.clone();
                    if let NetworkCheckState::PingInternet = ctx.checker_state {
                        telemetry_sender.send(TelemetryEvent::InternetPingResult {
                            interface_identifiers,
                            ping_parameters: parameters.clone(),
                            internet_ping_result: result,
                        });
                    } else {
                        telemetry_sender.send(TelemetryEvent::GatewayPingResult {
                            interface_identifiers,
                            ping_parameters: parameters.clone(),
                            gateway_ping_result: result,
                        });
                    }
                }

                let PingParameters { interface_name, addr, .. } = parameters;
                if ping_is_ok {
                    let () = Self::handle_ping_success(ctx, &addr);
                }

                if ctx.pings_completed == ctx.pings_expected {
                    if let NetworkCheckState::PingGateway = ctx.checker_state {
                        ctx.initiate_ping(
                            cookie.id,
                            &interface_name,
                            &self.network_check_sender,
                            NetworkCheckState::PingInternet,
                            [
                                IPV4_INTERNET_CONNECTIVITY_CHECK_ADDRESS,
                                IPV6_INTERNET_CONNECTIVITY_CHECK_ADDRESS,
                            ]
                            .into_iter()
                            .map(|ip| std::net::SocketAddr::new(ip, 0))
                            .collect(),
                        );
                    } else {
                        let parameters = ResolveDnsParameters {
                            interface_name: interface_name.to_string(),
                            domain: GSTATIC.into(),
                        };
                        ctx.checker_state = NetworkCheckState::ResolveDns;

                        if self.time_provider.now() - ctx.persistent_context.resolved_time
                            < DNS_PROBE_PERIOD
                        {
                            debug!(
                                "Skipping ResolveDns since it has not yet been {} seconds",
                                DNS_PROBE_PERIOD.clone().into_seconds()
                            );
                            if let Some(ips) = ctx.persistent_context.resolved_addrs.get(GSTATIC) {
                                if !ips.v4.is_empty() {
                                    ctx.discovered_state.ipv4.application.dns_resolved = true;
                                }
                                if !ips.v6.is_empty() {
                                    ctx.discovered_state.ipv6.application.dns_resolved = true;
                                }
                            }
                            return self.resume(
                                cookie,
                                NetworkCheckResult::ResolveDns { parameters, ips: None },
                            );
                        }

                        let action = NetworkCheckAction::ResolveDns(parameters);
                        match self
                            .network_check_sender
                            .unbounded_send((action, NetworkCheckCookie { id: cookie.id }))
                        {
                            Ok(()) => {}
                            Err(e) => {
                                debug!("unable to send network check internet msg: {e:?}")
                            }
                        }
                    }
                }
            }
            NetworkCheckState::ResolveDns => {
                let (ResolveDnsParameters { interface_name, domain }, ips) =
                    result.resolve_dns_result().ok_or_else(|| {
                        anyhow!(
                            "resume: mismatched state and result {interface_name} ({})",
                            cookie.id
                        )
                    })?;

                if let Some(ips) = ips {
                    if !ips.v4.is_empty() {
                        ctx.discovered_state.ipv4.application.dns_resolved = true;
                    }
                    if !ips.v6.is_empty() {
                        ctx.discovered_state.ipv6.application.dns_resolved = true;
                    }
                    ctx.persistent_context.resolved_time = self.time_provider.now();
                    let _: Option<ResolvedIps> =
                        ctx.persistent_context.resolved_addrs.insert(domain.clone(), ips);
                }

                ctx.checker_state = NetworkCheckState::FetchHttp;
                ctx.fetches_expected = 0;

                let mut add_fetch = |ip: IpAddr| {
                    ctx.fetches_expected += 1;
                    let action = NetworkCheckAction::Fetch(FetchParameters {
                        interface_name: interface_name.clone(),
                        domain: domain.clone(),
                        ip,
                        path: GENERATE_204.into(),
                        expected_statuses: vec![204],
                    });
                    match self
                        .network_check_sender
                        .unbounded_send((action, NetworkCheckCookie { id: cookie.id }))
                    {
                        Ok(()) => {}
                        Err(e) => debug!("unable to send network check internet message: {e:?}"),
                    }
                };

                if let Some(v4) =
                    ctx.persistent_context.resolved_addrs.get(&domain).and_then(|ips| ips.v4.get(0))
                {
                    add_fetch(IpAddr::V4(*v4));
                }
                if let Some(v6) =
                    ctx.persistent_context.resolved_addrs.get(&domain).and_then(|ips| ips.v6.get(0))
                {
                    add_fetch(IpAddr::V6(*v6));
                }

                if ctx.fetches_expected == 0 {
                    return self.update_state_from_context(cookie.id, &interface_name);
                }
            }
            NetworkCheckState::FetchHttp => {
                let (parameters, result) = result.fetch_result().ok_or_else(|| {
                    anyhow!("resume: mismatched state and result {interface_name} ({})", cookie.id)
                })?;
                ctx.fetches_completed += 1;

                // Grab `fetch_ok_status` before `result` is moved into `telemetry_sender.send`.
                let fetch_ok_status = result.as_ref().copied().ok();

                if let Some(telemetry_sender) = &mut self.telemetry_sender {
                    telemetry_sender.send(TelemetryEvent::FetchResult {
                        interface_identifiers: ctx
                            .persistent_context
                            .telemetry
                            .interface_identifiers
                            .clone(),
                        fetch_parameters: parameters.clone(),
                        fetch_result: result,
                    });
                }

                let FetchParameters { interface_name, ip, expected_statuses, .. } = parameters;
                if let Some(status) = fetch_ok_status {
                    if expected_statuses.contains(&status) {
                        let () = Self::handle_fetch_success(ctx, ip);
                    }
                }

                if ctx.fetches_completed == ctx.fetches_expected {
                    return self.update_state_from_context(cookie.id, &interface_name);
                }
            }
        }
        Ok(NetworkCheckerOutcome::MustResume)
    }
}

fn log_state(info: Option<&mut InspectInfo>, proto: Proto, state: State) {
    info.into_iter().for_each(|info| info.log_link_state(proto, state.link))
}

#[derive(Default, PartialEq)]
enum NeighborHealthScanResult {
    // No healthy neighbors were discovered.
    #[default]
    NoneHealthy,
    // A healthy neighbor was discovered.
    HealthyNeighbor,
    // A healthy router was discovered. Takes precedence over
    // `HealthyNeighbor` since an healthy router implies
    // a healthy neighbor.
    HealthyRouter,
}

impl NeighborHealthScanResult {
    // A neighbor was discovered. Update the state based on whether the neighbor
    // is a router.
    fn update_scan_result(&mut self, is_router: bool) {
        *self = match (&self, is_router) {
            // HealthyRouter should never degrade to HealthyNeighbor.
            (_, true) | (Self::HealthyRouter, _) => Self::HealthyRouter,
            _ => Self::HealthyNeighbor,
        }
    }

    fn is_healthy(&self) -> bool {
        match self {
            Self::NoneHealthy => false,
            Self::HealthyNeighbor | Self::HealthyRouter => true,
        }
    }

    fn is_healthy_router(&self) -> bool {
        match self {
            Self::NoneHealthy | Self::HealthyNeighbor => false,
            Self::HealthyRouter => true,
        }
    }
}

// Determines whether any online neighbors or online gateways are discoverable via neighbor
// discovery. The definition of a Healthy neighbor correlates to a neighbor being online.
fn scan_neighbor_health(
    neighbors: Option<&InterfaceNeighborCache>,
    device_routes: &Vec<route_table::Route>,
) -> IpVersions<NeighborHealthScanResult> {
    match neighbors {
        None => Default::default(),
        Some(neighbors) => {
            let router_next_hops: HashSet<_> =
                device_routes.iter().filter_map(|Route { next_hop, .. }| *next_hop).collect();
            neighbors.iter_health().fold(
                Default::default(),
                |mut neighbor_health_scan, (neighbor, health)| {
                    match health {
                        // When we find an unhealthy or unknown neighbor, continue,
                        // keeping whether we've previously found a healthy neighbor.
                        neighbor_cache::NeighborHealth::Unhealthy { .. }
                        | neighbor_cache::NeighborHealth::Unknown => neighbor_health_scan,
                        // If there's a healthy router, then we're done. If the neighbor
                        // is not a router, then we know we have a healthy neighbor, but
                        // not a healthy router.
                        neighbor_cache::NeighborHealth::Healthy { .. } => {
                            let scan = match neighbor {
                                fnet::IpAddress::Ipv4(..) => &mut neighbor_health_scan.ipv4,
                                fnet::IpAddress::Ipv6(..) => &mut neighbor_health_scan.ipv6,
                            };

                            scan.update_scan_result(router_next_hops.contains(neighbor));
                            neighbor_health_scan
                        }
                    }
                },
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fetch::FetchAddr;

    use super::*;
    use crate::dig::Dig;
    use crate::fetch::Fetch;
    use crate::neighbor_cache::{NeighborHealth, NeighborState};
    use crate::ping::Ping;
    use async_trait::async_trait;
    use diagnostics_assertions::assert_data_tree;
    use fidl_fuchsia_net as fnet;
    use fidl_fuchsia_net_interfaces as fnet_interfaces;
    use fuchsia_async as fasync;
    use futures::StreamExt as _;
    use net_declare::{fidl_ip, fidl_subnet, std_ip, std_socket_addr};
    use net_types::ip;
    use std::pin::pin;
    use std::task::Poll;
    use test_case::test_case;

    const ETHERNET_INTERFACE_NAME: &str = "eth1";
    const ID1: u64 = 1;
    const ID2: u64 = 2;
    // RFC5737§3 specifies the reserved IPv4 address prefix for tests and documentation.
    const IPV4_ADDR: fnet::IpAddress = fidl_ip!("192.168.0.1");
    // RFC-3849§4 specifies the global IPv6 unicast address prefix for tests and documentation.
    const IPV6_ADDR: fnet::IpAddress = fidl_ip!("2001:db8::");

    // A trait for writing helper constructors.
    //
    // Note that this trait differs from `std::convert::From` only in name, but will almost always
    // contain shortcuts that would be too surprising for an actual `From` implementation.
    trait Construct<T> {
        fn construct(_: T) -> Self;
    }

    impl<S: Into<State>> Construct<S> for StateEvent {
        fn construct(link: S) -> Self {
            Self { state: link.into(), time: fasync::MonotonicInstant::INFINITE }
        }
    }

    impl Construct<(LinkState, bool, bool)> for StateEvent {
        fn construct((link, dns_resolved, http_fetch_succeeded): (LinkState, bool, bool)) -> Self {
            Self {
                state: State {
                    link,
                    application: ApplicationState { dns_resolved, http_fetch_succeeded },
                },
                time: fasync::MonotonicInstant::INFINITE,
            }
        }
    }

    impl Construct<StateEvent> for IpVersions<StateEvent> {
        fn construct(state: StateEvent) -> Self {
            Self { ipv4: state, ipv6: state }
        }
    }

    struct FakeTime {
        increment: zx::MonotonicDuration,
        time: zx::MonotonicInstant,
    }

    impl TimeProvider for FakeTime {
        fn now(&mut self) -> zx::MonotonicInstant {
            let result = self.time;
            self.time += self.increment;
            result
        }
    }

    #[fuchsia::test]
    async fn test_log_state_vals_inspect() {
        let inspector = Inspector::default();
        LinkState::log_state_vals_inspect(inspector.root(), "state_vals");
        assert_data_tree!(inspector, root: {
            state_vals: {
                "1": "None",
                "5": "Removed",
                "10": "Down",
                "15": "Up",
                "20": "Local",
                "25": "Gateway",
                "30": "Internet",
            }
        })
    }

    #[fuchsia::test]
    fn test_display() {
        assert_eq!(Proto::IPv4.to_string(), "IPv4");
        assert_eq!(Proto::IPv6.to_string(), "IPv6");

        let system_state = SystemState {
            id: 2,
            state: StateEvent {
                state: State {
                    link: LinkState::Local,
                    application: ApplicationState {
                        dns_resolved: false,
                        http_fetch_succeeded: false,
                    },
                },
                time: fasync::MonotonicInstant::from_nanos(1_000_000_000),
            },
        };
        assert_eq!(system_state.to_string(), "Local (dns: false, http: false) via Interface [2]");

        let system_state_zero = SystemState {
            id: 0,
            state: StateEvent {
                state: State {
                    link: LinkState::Down,
                    application: ApplicationState {
                        dns_resolved: false,
                        http_fetch_succeeded: false,
                    },
                },
                time: fasync::MonotonicInstant::from_nanos(1_000_000_000),
            },
        };
        assert_eq!(
            system_state_zero.to_string(),
            "Down (dns: false, http: false) via Interface [0]"
        );

        let system_state_internet = SystemState {
            id: 42,
            state: StateEvent {
                state: State {
                    link: LinkState::Internet,
                    application: ApplicationState {
                        dns_resolved: true,
                        http_fetch_succeeded: true,
                    },
                },
                time: fasync::MonotonicInstant::from_nanos(1_000_000_000),
            },
        };
        assert_eq!(
            system_state_internet.to_string(),
            "Internet (dns: true, http: true) via Interface [42]"
        );

        let system_state_max_id = SystemState {
            id: u64::MAX,
            state: StateEvent {
                state: State {
                    link: LinkState::Gateway,
                    application: ApplicationState {
                        dns_resolved: true,
                        http_fetch_succeeded: false,
                    },
                },
                time: fasync::MonotonicInstant::from_nanos(1_000_000_000),
            },
        };
        assert_eq!(
            system_state_max_id.to_string(),
            "Gateway (dns: true, http: false) via Interface [18446744073709551615]"
        );

        // Verify that no internal types or compiler artifacts leak into formatted strings.
        for formatted in [
            system_state.to_string(),
            system_state_zero.to_string(),
            system_state_internet.to_string(),
            system_state_max_id.to_string(),
        ] {
            assert!(!formatted.contains("PhantomData"));
            assert!(!formatted.contains("Instant"));
            assert!(!formatted.contains("MonotonicTimeline"));
        }
    }

    #[test_case(NetworkCheckState::PingGateway, &[std_socket_addr!("1.2.3.0:8080")];
        "gateway ping on ipv4")]
    #[test_case(NetworkCheckState::PingGateway, &[std_socket_addr!("[123::]:0")];
        "gateway ping on ipv6")]
    #[test_case(NetworkCheckState::PingGateway, &[std_socket_addr!("1.2.3.0:8080"),
        std_socket_addr!("[123::]:0")]; "gateway ping on ipv4/ipv6")]
    #[test_case(NetworkCheckState::PingInternet, &[std_socket_addr!("8.8.8.8:0")];
        "internet ping on ipv4")]
    #[test_case(NetworkCheckState::PingInternet, &[std_socket_addr!("[2001:4860:4860::8888]:0")];
        "internet ping on ipv6")]
    #[test_case(NetworkCheckState::PingInternet, &[std_socket_addr!("8.8.8.8:0"),
        std_socket_addr!("[2001:4860:4860::8888]:0")]; "internet ping on ipv4/ipv6")]
    fn test_handle_ping_success(checker_state: NetworkCheckState, addrs: &[std::net::SocketAddr]) {
        let mut expected_state_v4: State = Default::default();
        let mut expected_state_v6: State = Default::default();

        let mut ctx = NetworkCheckContext { checker_state, ..Default::default() };
        // Initial state.
        assert_eq!(ctx.discovered_state.ipv4, expected_state_v4);
        assert_eq!(ctx.discovered_state.ipv6, expected_state_v6);

        let expected_state = match ctx.checker_state {
            NetworkCheckState::PingGateway => LinkState::Gateway.into(),
            NetworkCheckState::PingInternet => LinkState::Internet.into(),
            NetworkCheckState::ResolveDns => LinkState::Internet.into(),
            NetworkCheckState::FetchHttp => State {
                link: LinkState::Internet,
                application: ApplicationState { dns_resolved: true, http_fetch_succeeded: true },
            },
            NetworkCheckState::Begin | NetworkCheckState::Idle => Default::default(),
        };

        addrs.iter().for_each(|addr| {
            // Run the function under test for each address.
            let () = Monitor::<FakeTime>::handle_ping_success(&mut ctx, addr);
            // Update the expected values accordingly.
            match addr {
                std::net::SocketAddr::V4 { .. } => {
                    expected_state_v4 = expected_state;
                }
                std::net::SocketAddr::V6 { .. } => {
                    expected_state_v6 = expected_state;
                }
            }
        });
        // Final state.
        assert_eq!(ctx.discovered_state.ipv4, expected_state_v4);
        assert_eq!(ctx.discovered_state.ipv6, expected_state_v6);
    }

    #[derive(Default, Clone)]
    struct FakePing {
        gateway_addrs: std::collections::HashSet<std::net::IpAddr>,
        gateway_response: bool,
        internet_response: bool,
    }

    #[async_trait]
    impl Ping for FakePing {
        async fn ping(
            &self,
            _interface_name: &str,
            addr: std::net::SocketAddr,
        ) -> Result<(), crate::ping::PingError> {
            let Self { gateway_addrs, gateway_response, internet_response } = self;
            let ip = addr.ip();
            let success = if [
                IPV4_INTERNET_CONNECTIVITY_CHECK_ADDRESS,
                IPV6_INTERNET_CONNECTIVITY_CHECK_ADDRESS,
            ]
            .contains(&ip)
            {
                *internet_response
            } else if gateway_addrs.contains(&ip) {
                *gateway_response
            } else {
                false
            };
            if success { Ok(()) } else { Err(crate::ping::PingError::NoReply) }
        }
    }

    #[derive(Default)]
    struct FakeDig {
        response: Option<ResolvedIps>,
    }

    impl FakeDig {
        fn new(ips: Vec<std::net::IpAddr>) -> Self {
            let mut ips_out = ResolvedIps::default();
            for ip in ips {
                match ip {
                    IpAddr::V4(v4) => ips_out.v4.push(v4),
                    IpAddr::V6(v6) => ips_out.v6.push(v6),
                }
            }
            FakeDig { response: Some(ips_out) }
        }
    }

    #[async_trait]
    impl Dig for FakeDig {
        async fn dig(&self, _interface_name: &str, _domain: &str) -> Option<ResolvedIps> {
            self.response.clone()
        }
    }

    #[derive(Default)]
    struct FakeFetch {
        expected_url: Option<&'static str>,
        response: Option<Box<dyn Fn() -> Result<u16, fetch::FetchError> + Send + Sync>>,
    }

    #[async_trait]
    impl Fetch for FakeFetch {
        async fn fetch<FA: FetchAddr + std::marker::Sync>(
            &self,
            _interface_name: &str,
            domain: &str,
            path: &str,
            _addr: &FA,
        ) -> Result<u16, fetch::FetchError> {
            if let Some(expected) = self.expected_url {
                assert_eq!(
                    format!("http://{domain}{path}"),
                    expected,
                    "Did not receive expected URL"
                );
            }
            if let Some(response) = &self.response {
                response()
            } else {
                Err(fetch::FetchError::ReadTcpStreamTimeout)
            }
        }
    }

    struct NetworkCheckTestResponder {
        receiver: mpsc::UnboundedReceiver<(NetworkCheckAction, NetworkCheckCookie)>,
    }

    impl NetworkCheckTestResponder {
        fn new(
            receiver: mpsc::UnboundedReceiver<(NetworkCheckAction, NetworkCheckCookie)>,
        ) -> Self {
            Self { receiver }
        }

        async fn respond_to_messages<P: Ping, D: Dig, F: Fetch, Time: TimeProvider>(
            &mut self,
            monitor: &mut Monitor<Time>,
            p: P,
            d: D,
            f: F,
        ) {
            loop {
                if let Some((action, cookie)) = self.receiver.next().await {
                    match action {
                        NetworkCheckAction::Ping(parameters) => {
                            let result = p.ping(&parameters.interface_name, parameters.addr).await;
                            match monitor
                                .resume(cookie, NetworkCheckResult::Ping { parameters, result })
                            {
                                // Has reached final state.
                                Ok(NetworkCheckerOutcome::Complete) => return,
                                _ => {}
                            }
                        }
                        NetworkCheckAction::ResolveDns(parameters) => {
                            let ips = d.dig(&parameters.interface_name, &parameters.domain).await;
                            match monitor
                                .resume(cookie, NetworkCheckResult::ResolveDns { parameters, ips })
                            {
                                // Has reached final state.
                                Ok(NetworkCheckerOutcome::Complete) => return,
                                _ => {}
                            }
                        }
                        NetworkCheckAction::Fetch(parameters) => {
                            let result = f
                                .fetch(
                                    &parameters.interface_name,
                                    &parameters.domain,
                                    &parameters.path,
                                    &parameters.ip,
                                )
                                .await;
                            match monitor
                                .resume(cookie, NetworkCheckResult::Fetch { parameters, result })
                            {
                                // Has reached final state.
                                Ok(NetworkCheckerOutcome::Complete) => return,
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    fn run_network_check_partial_properties_repeated<P: Ping, D: Dig, F: Fetch>(
        exec: &mut fasync::TestExecutor,
        name: &str,
        interface_id: u64,
        routes: &RouteTable,
        mocks: Vec<(P, D, F)>,
        neighbors: Option<&InterfaceNeighborCache>,
        internet_ping_address: std::net::IpAddr,
        sleep_between: Option<zx::MonotonicDuration>,
    ) -> Vec<State> {
        let properties = &fnet_interfaces_ext::Properties {
            id: interface_id.try_into().expect("should be nonzero"),
            name: name.to_string(),
            port_class: fnet_interfaces_ext::PortClass::Ethernet,
            online: true,
            addresses: Default::default(),
            has_default_ipv4_route: Default::default(),
            has_default_ipv6_route: Default::default(),
            port_identity_koid: Default::default(),
        };

        let mock_count = mocks.len();
        match run_network_check_repeated(exec, properties, routes, neighbors, mocks, sleep_between)
        {
            Ok(Some(events)) => {
                // Implementation checks v4 and v6 connectivity concurrently, although these tests
                // only check for a single protocol at a time. The address being pinged determines
                // which protocol to use.
                events
                    .into_iter()
                    .map(|event| match internet_ping_address {
                        std::net::IpAddr::V4 { .. } => event.ipv4.state,
                        std::net::IpAddr::V6 { .. } => event.ipv6.state,
                    })
                    .collect()
            }
            Ok(None) => {
                error!("id for interface unexpectedly did not exist after network check");
                std::iter::repeat(LinkState::None.into()).take(mock_count).collect()
            }
            Err(e) => {
                error!("network check had an issue calculating state: {:?}", e);
                std::iter::repeat(LinkState::None.into()).take(mock_count).collect()
            }
        }
    }

    fn run_network_check_partial_properties<P: Ping, D: Dig, F: Fetch>(
        exec: &mut fasync::TestExecutor,
        name: &str,
        interface_id: u64,
        routes: &RouteTable,
        pinger: P,
        digger: D,
        fetcher: F,
        neighbors: Option<&InterfaceNeighborCache>,
        internet_ping_address: std::net::IpAddr,
    ) -> State {
        run_network_check_partial_properties_repeated(
            exec,
            name,
            interface_id,
            routes,
            vec![(pinger, digger, fetcher)],
            neighbors,
            internet_ping_address,
            None,
        )
        .pop()
        .unwrap_or_else(|| {
            error!("network check returned no states");
            LinkState::None.into()
        })
    }

    fn run_network_check_repeated<P: Ping, D: Dig, F: Fetch>(
        exec: &mut fasync::TestExecutor,
        properties: &fnet_interfaces_ext::Properties<fnet_interfaces_ext::DefaultInterest>,
        routes: &RouteTable,
        neighbors: Option<&InterfaceNeighborCache>,
        mocks: Vec<(P, D, F)>,
        sleep_between: Option<zx::MonotonicDuration>,
    ) -> Result<Option<Vec<IpVersions<StateEvent>>>, anyhow::Error> {
        let (sender, receiver) = mpsc::unbounded::<(NetworkCheckAction, NetworkCheckCookie)>();
        let mut monitor = Monitor::new_with_time_provider(
            sender,
            FakeTime {
                increment: sleep_between.unwrap_or(zx::MonotonicDuration::from_nanos(10)),
                time: zx::MonotonicInstant::get(),
            },
        )
        .unwrap();
        let mut network_check_responder = NetworkCheckTestResponder::new(receiver);

        let view = InterfaceView { properties, routes, neighbors };
        let network_check_fut = async {
            let mut states = Vec::new();
            for (pinger, digger, fetcher) in mocks {
                match monitor.begin(view) {
                    Ok(NetworkCheckerOutcome::Complete) => {}
                    Ok(NetworkCheckerOutcome::MustResume) => {
                        let () = network_check_responder
                            .respond_to_messages(&mut monitor, pinger, digger, fetcher)
                            .await;
                    }
                    Err(e) => {
                        error!("begin had an issue calculating state: {:?}", e)
                    }
                }
                states.push(monitor.state().get(properties.id.get()).map(Clone::clone));
            }
            states
        };

        let mut network_check_fut = pin!(network_check_fut);
        match exec.run_until_stalled(&mut network_check_fut) {
            Poll::Ready(got) => Ok(got.into_iter().collect()),
            Poll::Pending => Err(anyhow::anyhow!("network_check blocked unexpectedly")),
        }
    }

    fn run_network_check<P: Ping, D: Dig, F: Fetch>(
        exec: &mut fasync::TestExecutor,
        properties: &fnet_interfaces_ext::Properties<fnet_interfaces_ext::DefaultInterest>,
        routes: &RouteTable,
        neighbors: Option<&InterfaceNeighborCache>,
        pinger: P,
        digger: D,
        fetcher: F,
    ) -> Result<Option<IpVersions<StateEvent>>, anyhow::Error> {
        run_network_check_repeated(
            exec,
            properties,
            routes,
            neighbors,
            vec![(pinger, digger, fetcher)],
            None,
        )
        .map(|res| res.and_then(|mut v| v.pop()))
    }

    #[test]
    fn test_network_check_ipv6_local_only() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let time = fasync::MonotonicInstant::from_nanos(1_000_000_000);
        let () = exec.set_fake_time(time.into());

        // The next_hop of the default route must be the same as a known neighbor. This is used
        // to determine this neighbor as a valid gateway.
        let routes = testutil::build_route_table_from_flattened_routes([Route {
            destination: UNSPECIFIED_V6,
            outbound_interface: ID1,
            next_hop: Some(IPV6_ADDR),
        }]);
        let properties = &fnet_interfaces_ext::Properties {
            id: ID1.try_into().expect("should be nonzero"),
            name: ETHERNET_INTERFACE_NAME.to_string(),
            port_class: fnet_interfaces_ext::PortClass::Ethernet,
            online: true,
            addresses: vec![],
            has_default_ipv4_route: false,
            has_default_ipv6_route: true,
            port_identity_koid: Default::default(),
        };
        let neighbors = InterfaceNeighborCache::default();

        let got = run_network_check(
            &mut exec,
            properties,
            &routes,
            Some(&neighbors),
            FakePing::default(),
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect("run_network_check failed")
        .expect("interface state not found");

        let want_ipv4 =
            StateEvent { state: State { link: LinkState::Up, ..Default::default() }, time };
        let want_ipv6 =
            StateEvent { state: State { link: LinkState::Local, ..Default::default() }, time };
        assert_eq!(got.ipv4, want_ipv4);
        assert_eq!(got.ipv6, want_ipv6);
    }

    #[test]
    fn test_network_check_ipv6_local_only_not_default_route() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let time = fasync::MonotonicInstant::from_nanos(1_000_000_000);
        let () = exec.set_fake_time(time.into());

        // The next_hop of the default route must be the same as a known neighbor. This is used
        // to determine this neighbor as a valid gateway.
        let routes = testutil::build_route_table_from_flattened_routes([Route {
            destination: fidl_subnet!("::/1"),
            outbound_interface: ID1,
            next_hop: Some(IPV6_ADDR),
        }]);
        let properties = &fnet_interfaces_ext::Properties {
            id: ID1.try_into().expect("should be nonzero"),
            name: ETHERNET_INTERFACE_NAME.to_string(),
            port_class: fnet_interfaces_ext::PortClass::Ethernet,
            online: true,
            addresses: vec![],
            has_default_ipv4_route: false,
            has_default_ipv6_route: true,
            port_identity_koid: Default::default(),
        };
        let neighbors = InterfaceNeighborCache::default();

        let got = run_network_check(
            &mut exec,
            properties,
            &routes,
            Some(&neighbors),
            FakePing::default(),
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect("run_network_check failed")
        .expect("interface state not found");

        let want_ipv4 =
            StateEvent { state: State { link: LinkState::Up, ..Default::default() }, time };
        let want_ipv6 =
            StateEvent { state: State { link: LinkState::Local, ..Default::default() }, time };
        assert_eq!(got.ipv4, want_ipv4);
        assert_eq!(got.ipv6, want_ipv6);
    }

    #[test]
    fn test_network_check_ipv6_gateway_only() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let time = fasync::MonotonicInstant::from_nanos(1_000_000_000);
        let () = exec.set_fake_time(time.into());

        // The next_hop of the default route must be the same as a known neighbor. This is used
        // to determine this neighbor as a valid gateway.
        let routes = testutil::build_route_table_from_flattened_routes([Route {
            destination: UNSPECIFIED_V6,
            outbound_interface: ID1,
            next_hop: Some(IPV6_ADDR),
        }]);
        let properties = &fnet_interfaces_ext::Properties {
            id: ID1.try_into().expect("should be nonzero"),
            name: ETHERNET_INTERFACE_NAME.to_string(),
            port_class: fnet_interfaces_ext::PortClass::Ethernet,
            online: true,
            addresses: vec![],
            has_default_ipv4_route: false,
            has_default_ipv6_route: true,
            port_identity_koid: Default::default(),
        };
        let neighbors = InterfaceNeighborCache {
            neighbors: [(
                IPV6_ADDR,
                NeighborState::new(NeighborHealth::Healthy {
                    last_observed: zx::MonotonicInstant::default(),
                }),
            )]
            .into_iter()
            .collect::<HashMap<fnet::IpAddress, NeighborState>>(),
        };

        let got = run_network_check(
            &mut exec,
            properties,
            &routes,
            Some(&neighbors),
            FakePing::default(),
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect("run_network_check failed")
        .expect("interface state not found");

        let want_ipv4 =
            StateEvent { state: State { link: LinkState::Up, ..Default::default() }, time };
        let want_ipv6 =
            StateEvent { state: State { link: LinkState::Gateway, ..Default::default() }, time };
        assert_eq!(got.ipv4, want_ipv4);
        assert_eq!(got.ipv6, want_ipv6);
    }

    #[fuchsia::test]
    fn test_network_check_ipv4_and_ipv6_gateway() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let time = fasync::MonotonicInstant::from_nanos(1_000_000_000);
        let () = exec.set_fake_time(time.into());

        // The next_hop of the default route must be the same as a known neighbor. This is used
        // to determine this neighbor as a valid gateway.
        let routes = testutil::build_route_table_from_flattened_routes([
            Route {
                destination: UNSPECIFIED_V4,
                outbound_interface: ID1,
                next_hop: Some(IPV4_ADDR),
            },
            Route {
                destination: UNSPECIFIED_V6,
                outbound_interface: ID1,
                next_hop: Some(IPV6_ADDR),
            },
        ]);
        let properties = &fnet_interfaces_ext::Properties {
            id: ID1.try_into().expect("should be nonzero"),
            name: ETHERNET_INTERFACE_NAME.to_string(),
            port_class: fnet_interfaces_ext::PortClass::Ethernet,
            online: true,
            addresses: vec![],
            has_default_ipv4_route: true,
            has_default_ipv6_route: true,
            port_identity_koid: Default::default(),
        };
        let neighbors = InterfaceNeighborCache {
            neighbors: [
                (
                    IPV4_ADDR,
                    NeighborState::new(NeighborHealth::Healthy {
                        last_observed: zx::MonotonicInstant::default(),
                    }),
                ),
                (
                    IPV6_ADDR,
                    NeighborState::new(NeighborHealth::Healthy {
                        last_observed: zx::MonotonicInstant::default(),
                    }),
                ),
            ]
            .into_iter()
            .collect::<HashMap<fnet::IpAddress, NeighborState>>(),
        };

        let got = run_network_check(
            &mut exec,
            properties,
            &routes,
            Some(&neighbors),
            FakePing::default(),
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect("run_network_check failed")
        .expect("interface state not found");

        assert_eq!(
            got,
            IpVersions::construct(StateEvent {
                state: State { link: LinkState::Gateway, ..Default::default() },
                time
            })
        );
    }

    #[test]
    fn test_network_check_ethernet_ipv4() {
        test_network_check_ethernet::<ip::Ipv4>(
            fidl_ip!("1.2.3.0"),
            fidl_ip!("1.2.3.4"),
            fidl_ip!("1.2.3.1"),
            fidl_ip!("2.2.3.0"),
            fidl_ip!("2.2.3.1"),
            UNSPECIFIED_V4,
            fidl_subnet!("0.0.0.0/1"),
            IPV4_INTERNET_CONNECTIVITY_CHECK_ADDRESS,
            24,
        );
    }

    #[test]
    fn test_network_check_ethernet_ipv6() {
        test_network_check_ethernet::<ip::Ipv6>(
            fidl_ip!("123::"),
            fidl_ip!("123::4"),
            fidl_ip!("123::1"),
            fidl_ip!("223::"),
            fidl_ip!("223::1"),
            UNSPECIFIED_V6,
            fidl_subnet!("::/1"),
            IPV6_INTERNET_CONNECTIVITY_CHECK_ADDRESS,
            64,
        );
    }

    fn test_network_check_ethernet<I: ip::Ip>(
        net1: fnet::IpAddress,
        _net1_addr: fnet::IpAddress,
        net1_gateway: fnet::IpAddress,
        net2: fnet::IpAddress,
        net2_gateway: fnet::IpAddress,
        unspecified_addr: fnet::Subnet,
        non_default_addr: fnet::Subnet,
        ping_internet_addr: std::net::IpAddr,
        prefix_len: u8,
    ) {
        let route_table = testutil::build_route_table_from_flattened_routes([
            Route {
                destination: unspecified_addr,
                outbound_interface: ID1,
                next_hop: Some(net1_gateway),
            },
            Route {
                destination: fnet::Subnet { addr: net1, prefix_len },
                outbound_interface: ID1,
                next_hop: None,
            },
        ]);
        let route_table_2 = testutil::build_route_table_from_flattened_routes([
            Route {
                destination: unspecified_addr,
                outbound_interface: ID1,
                next_hop: Some(net2_gateway),
            },
            Route {
                destination: fnet::Subnet { addr: net1, prefix_len },
                outbound_interface: ID1,
                next_hop: None,
            },
            Route {
                destination: fnet::Subnet { addr: net2, prefix_len },
                outbound_interface: ID1,
                next_hop: None,
            },
        ]);
        let route_table_3 = testutil::build_route_table_from_flattened_routes([
            Route {
                destination: unspecified_addr,
                outbound_interface: ID2,
                next_hop: Some(net1_gateway),
            },
            Route {
                destination: fnet::Subnet { addr: net1, prefix_len },
                outbound_interface: ID2,
                next_hop: None,
            },
        ]);
        let route_table_4 = testutil::build_route_table_from_flattened_routes([
            Route {
                destination: non_default_addr,
                outbound_interface: ID1,
                next_hop: Some(net1_gateway),
            },
            Route {
                destination: fnet::Subnet { addr: net1, prefix_len },
                outbound_interface: ID1,
                next_hop: None,
            },
        ]);

        let fnet_ext::IpAddress(net1_gateway_ext) = net1_gateway.into();
        let mut exec = fasync::TestExecutor::new();

        // TODO(fxrev.dev/120580): Extract test cases into variants/helper function
        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: true,
                    internet_response: true,
                },
                FakeDig::new(vec![std_ip!("1.2.3.0"), std_ip!("123::")]),
                FakeFetch {
                    expected_url: Some("http://www.gstatic.com/generate_204"),
                    response: Some(Box::new(|| Ok(204))),
                },
                None,
                ping_internet_addr,
            ),
            State {
                link: LinkState::Internet,
                application: ApplicationState { dns_resolved: true, http_fetch_succeeded: true },
            },
            "All is good. Can reach internet"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: true,
                    internet_response: true,
                },
                FakeDig::new(vec![std_ip!("1.2.3.0"), std_ip!("123::")]),
                FakeFetch::default(),
                None,
                ping_internet_addr,
            ),
            State {
                link: LinkState::Internet,
                application: ApplicationState { dns_resolved: true, ..Default::default() },
            },
            "HTTP Fetch fails"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: true,
                    internet_response: true,
                },
                FakeDig::new(vec![std_ip!("1.2.3.0"), std_ip!("1.2.4.0")]),
                FakeFetch::default(),
                None,
                ping_internet_addr,
            ),
            State {
                link: LinkState::Internet,
                application: ApplicationState {
                    dns_resolved: ping_internet_addr.is_ipv4(),
                    ..Default::default()
                },
            },
            "DNS Resolves only IPV4",
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: true,
                    internet_response: true,
                },
                FakeDig::new(vec![std_ip!("123::"), std_ip!("124::")]),
                FakeFetch::default(),
                None,
                ping_internet_addr,
            ),
            State {
                link: LinkState::Internet,
                application: ApplicationState {
                    dns_resolved: ping_internet_addr.is_ipv6(),
                    ..Default::default()
                },
            },
            "DNS Resolves only IPV6",
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: false,
                    internet_response: true,
                },
                FakeDig::default(),
                FakeFetch::default(),
                Some(&InterfaceNeighborCache {
                    neighbors: [(
                        net1_gateway,
                        NeighborState::new(NeighborHealth::Healthy {
                            last_observed: zx::MonotonicInstant::default(),
                        })
                    )]
                    .into_iter()
                    .collect::<HashMap<fnet::IpAddress, NeighborState>>()
                }),
                ping_internet_addr,
            ),
            LinkState::Internet.into(),
            "Can reach internet, gateway responding via ARP/ND"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: false,
                    internet_response: true,
                },
                FakeDig::default(),
                FakeFetch::default(),
                Some(&InterfaceNeighborCache {
                    neighbors: [(
                        net1,
                        NeighborState::new(NeighborHealth::Healthy {
                            last_observed: zx::MonotonicInstant::default(),
                        })
                    )]
                    .into_iter()
                    .collect::<HashMap<fnet::IpAddress, NeighborState>>()
                }),
                ping_internet_addr,
            ),
            LinkState::Internet.into(),
            "Gateway not responding via ping or ARP/ND. Can reach internet"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table_4,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: true,
                    internet_response: true,
                },
                FakeDig::default(),
                FakeFetch::default(),
                Some(&InterfaceNeighborCache {
                    neighbors: [(
                        net1_gateway,
                        NeighborState::new(NeighborHealth::Healthy {
                            last_observed: zx::MonotonicInstant::default(),
                        })
                    )]
                    .into_iter()
                    .collect::<HashMap<fnet::IpAddress, NeighborState>>()
                }),
                ping_internet_addr,
            ),
            LinkState::Internet.into(),
            "No default route, but healthy gateway with internet/gateway response"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: true,
                    internet_response: false,
                },
                FakeDig::default(),
                FakeFetch::default(),
                None,
                ping_internet_addr,
            ),
            LinkState::Gateway.into(),
            "Can reach gateway via ping"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing::default(),
                FakeDig::default(),
                FakeFetch::default(),
                Some(&InterfaceNeighborCache {
                    neighbors: [(
                        net1_gateway,
                        NeighborState::new(NeighborHealth::Healthy {
                            last_observed: zx::MonotonicInstant::default(),
                        })
                    )]
                    .into_iter()
                    .collect::<HashMap<fnet::IpAddress, NeighborState>>()
                }),
                ping_internet_addr,
            ),
            LinkState::Gateway.into(),
            "Can reach gateway via ARP/ND"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: false,
                    internet_response: false,
                },
                FakeDig::default(),
                FakeFetch::default(),
                None,
                ping_internet_addr,
            ),
            LinkState::Local.into(),
            "Local only, Cannot reach gateway"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table_2,
                FakePing::default(),
                FakeDig::default(),
                FakeFetch::default(),
                None,
                ping_internet_addr,
            ),
            LinkState::Local.into(),
            "No default route"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table_4,
                FakePing {
                    gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                    gateway_response: true,
                    internet_response: false,
                },
                FakeDig::default(),
                FakeFetch::default(),
                None,
                ping_internet_addr,
            ),
            LinkState::Local.into(),
            "No default route, with only gateway response"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table_2,
                FakePing::default(),
                FakeDig::default(),
                FakeFetch::default(),
                Some(&InterfaceNeighborCache {
                    neighbors: [(
                        net1,
                        NeighborState::new(NeighborHealth::Healthy {
                            last_observed: zx::MonotonicInstant::default(),
                        })
                    )]
                    .into_iter()
                    .collect::<HashMap<fnet::IpAddress, NeighborState>>()
                }),
                ping_internet_addr,
            ),
            LinkState::Local.into(),
            "Local only, neighbors responsive with no default route"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing::default(),
                FakeDig::default(),
                FakeFetch::default(),
                Some(&InterfaceNeighborCache {
                    neighbors: [(
                        net1,
                        NeighborState::new(NeighborHealth::Healthy {
                            last_observed: zx::MonotonicInstant::default(),
                        })
                    )]
                    .into_iter()
                    .collect::<HashMap<fnet::IpAddress, NeighborState>>()
                }),
                ping_internet_addr
            ),
            LinkState::Local.into(),
            "Local only, neighbors responsive with a default route"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table_3,
                FakePing::default(),
                FakeDig::default(),
                FakeFetch::default(),
                Some(&InterfaceNeighborCache {
                    neighbors: [(
                        net1,
                        NeighborState::new(NeighborHealth::Healthy {
                            last_observed: zx::MonotonicInstant::default(),
                        })
                    )]
                    .into_iter()
                    .collect::<HashMap<fnet::IpAddress, NeighborState>>()
                }),
                ping_internet_addr,
            ),
            LinkState::Local.into(),
            "Local only, neighbors responsive with no routes"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                FakePing::default(),
                FakeDig::default(),
                FakeFetch::default(),
                Some(&InterfaceNeighborCache {
                    neighbors: [
                        (
                            net1,
                            NeighborState::new(NeighborHealth::Healthy {
                                last_observed: zx::MonotonicInstant::default(),
                            })
                        ),
                        (
                            net1_gateway,
                            NeighborState::new(NeighborHealth::Unhealthy { last_healthy: None })
                        )
                    ]
                    .into_iter()
                    .collect::<HashMap<fnet::IpAddress, NeighborState>>()
                }),
                ping_internet_addr,
            ),
            LinkState::Local.into(),
            "Local only, gateway unhealthy with healthy neighbor"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table_3,
                FakePing::default(),
                FakeDig::default(),
                FakeFetch::default(),
                Some(&InterfaceNeighborCache {
                    neighbors: [(
                        net1_gateway,
                        NeighborState::new(NeighborHealth::Unhealthy { last_healthy: None })
                    )]
                    .into_iter()
                    .collect::<HashMap<fnet::IpAddress, NeighborState>>()
                }),
                ping_internet_addr,
            ),
            LinkState::Up.into(),
            "No routes and unhealthy gateway"
        );

        assert_eq!(
            run_network_check_partial_properties(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table_3,
                FakePing::default(),
                FakeDig::default(),
                FakeFetch::default(),
                None,
                ping_internet_addr,
            ),
            LinkState::Up.into(),
            "No routes",
        );

        assert_eq!(
            run_network_check_partial_properties_repeated(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                vec![
                    (
                        FakePing {
                            gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                            gateway_response: true,
                            internet_response: true,
                        },
                        FakeDig::new(vec![std_ip!("1.2.3.0"), std_ip!("123::")]), // First, use a good digger
                        FakeFetch {
                            expected_url: Some("http://www.gstatic.com/generate_204"),
                            response: Some(Box::new(|| Ok(204))),
                        },
                    ),
                    (
                        FakePing {
                            gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                            gateway_response: true,
                            internet_response: true,
                        },
                        FakeDig { response: None }, // Then, use one that fails
                        FakeFetch {
                            expected_url: Some("http://www.gstatic.com/generate_204"),
                            response: Some(Box::new(|| Ok(204))),
                        },
                    ),
                ],
                None,
                ping_internet_addr,
                None,
            ),
            vec![
                State {
                    link: LinkState::Internet,
                    application: ApplicationState {
                        dns_resolved: true,
                        http_fetch_succeeded: true
                    }
                },
                State {
                    link: LinkState::Internet,
                    application: ApplicationState {
                        dns_resolved: true,
                        http_fetch_succeeded: true
                    }
                }
            ],
            "Fail DNS on second check; fetch succeeds; no pause"
        );

        assert_eq!(
            run_network_check_partial_properties_repeated(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                vec![
                    (
                        FakePing {
                            gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                            gateway_response: true,
                            internet_response: true,
                        },
                        FakeDig::new(vec![std_ip!("1.2.3.0"), std_ip!("123::")]), // First, use a good digger
                        FakeFetch {
                            expected_url: Some("http://www.gstatic.com/generate_204"),
                            response: Some(Box::new(|| Ok(204))),
                        },
                    ),
                    (
                        FakePing {
                            gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                            gateway_response: true,
                            internet_response: true,
                        },
                        FakeDig { response: None }, // Then, use one that fails
                        FakeFetch {
                            expected_url: Some("http://www.gstatic.com/generate_204"),
                            response: Some(Box::new(|| Ok(204))),
                        },
                    ),
                ],
                None,
                ping_internet_addr,
                Some(DNS_PROBE_PERIOD),
            ),
            vec![
                State {
                    link: LinkState::Internet,
                    application: ApplicationState {
                        dns_resolved: true,
                        http_fetch_succeeded: true
                    }
                },
                State {
                    link: LinkState::Internet,
                    application: ApplicationState {
                        dns_resolved: false,
                        http_fetch_succeeded: true
                    }
                }
            ],
            "Fail DNS on second check; fetch succeeds"
        );

        assert_eq!(
            run_network_check_partial_properties_repeated(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                vec![
                    (
                        FakePing {
                            gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                            gateway_response: true,
                            internet_response: true,
                        },
                        FakeDig::new(vec![std_ip!("1.2.3.0"), std_ip!("123::")]), // First, use a good digger
                        FakeFetch {
                            expected_url: Some("http://www.gstatic.com/generate_204"),
                            response: Some(Box::new(|| Err(
                                fetch::FetchError::ReadTcpStreamTimeout
                            ))),
                        },
                    ),
                    (
                        FakePing {
                            gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                            gateway_response: true,
                            internet_response: true,
                        },
                        FakeDig { response: None }, // Then, use one that fails
                        FakeFetch {
                            expected_url: Some("http://www.gstatic.com/generate_204"),
                            response: Some(Box::new(|| Err(
                                fetch::FetchError::ReadTcpStreamTimeout
                            ))),
                        },
                    ),
                ],
                None,
                ping_internet_addr,
                None,
            ),
            vec![
                State {
                    link: LinkState::Internet,
                    application: ApplicationState { dns_resolved: true, ..Default::default() }
                },
                State {
                    link: LinkState::Internet,
                    application: ApplicationState { dns_resolved: true, ..Default::default() }
                }
            ],
            "Fail DNS on second check; fetch fails; no pause"
        );

        assert_eq!(
            run_network_check_partial_properties_repeated(
                &mut exec,
                ETHERNET_INTERFACE_NAME,
                ID1,
                &route_table,
                vec![
                    (
                        FakePing {
                            gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                            gateway_response: true,
                            internet_response: true,
                        },
                        FakeDig::new(vec![std_ip!("1.2.3.0"), std_ip!("123::")]), // First, use a good digger
                        FakeFetch {
                            expected_url: Some("http://www.gstatic.com/generate_204"),
                            response: Some(Box::new(|| Err(
                                fetch::FetchError::ReadTcpStreamTimeout
                            ))),
                        },
                    ),
                    (
                        FakePing {
                            gateway_addrs: std::iter::once(net1_gateway_ext).collect(),
                            gateway_response: true,
                            internet_response: true,
                        },
                        FakeDig { response: None }, // Then, use one that fails
                        FakeFetch {
                            expected_url: Some("http://www.gstatic.com/generate_204"),
                            response: Some(Box::new(|| Err(
                                fetch::FetchError::ReadTcpStreamTimeout
                            ))),
                        },
                    ),
                ],
                None,
                ping_internet_addr,
                Some(DNS_PROBE_PERIOD),
            ),
            vec![
                State {
                    link: LinkState::Internet,
                    application: ApplicationState { dns_resolved: true, ..Default::default() }
                },
                State {
                    link: LinkState::Internet,
                    application: ApplicationState { dns_resolved: false, ..Default::default() }
                }
            ],
            "Fail DNS on second check; fetch fails"
        );
    }

    #[test]
    fn test_network_check_varying_properties() {
        let properties = fnet_interfaces_ext::Properties {
            id: ID1.try_into().expect("should be nonzero"),
            name: ETHERNET_INTERFACE_NAME.to_string(),
            port_class: fnet_interfaces_ext::PortClass::Ethernet,
            has_default_ipv4_route: true,
            has_default_ipv6_route: true,
            online: true,
            addresses: vec![
                fnet_interfaces_ext::Address {
                    addr: fidl_subnet!("1.2.3.0/24"),
                    valid_until: fnet_interfaces_ext::NoInterest,
                    preferred_lifetime_info: fnet_interfaces_ext::NoInterest,
                    assignment_state: fnet_interfaces::AddressAssignmentState::Assigned,
                },
                fnet_interfaces_ext::Address {
                    addr: fidl_subnet!("123::4/64"),
                    valid_until: fnet_interfaces_ext::NoInterest,
                    preferred_lifetime_info: fnet_interfaces_ext::NoInterest,
                    assignment_state: fnet_interfaces::AddressAssignmentState::Assigned,
                },
            ],
            port_identity_koid: Default::default(),
        };
        let local_routes = testutil::build_route_table_from_flattened_routes([
            Route {
                destination: fidl_subnet!("1.2.3.0/24"),
                outbound_interface: ID1,
                next_hop: None,
            },
            Route {
                destination: fidl_subnet!("123::/64"),
                outbound_interface: ID1,
                next_hop: None,
            },
        ]);
        let route_table = testutil::build_route_table_from_flattened_routes([
            Route {
                destination: fidl_subnet!("0.0.0.0/0"),
                outbound_interface: ID1,
                next_hop: Some(fidl_ip!("1.2.3.1")),
            },
            Route {
                destination: fidl_subnet!("::0/0"),
                outbound_interface: ID1,
                next_hop: Some(fidl_ip!("123::1")),
            },
        ]);
        let route_table2 = testutil::build_route_table_from_flattened_routes([
            Route {
                destination: fidl_subnet!("0.0.0.0/0"),
                outbound_interface: ID1,
                next_hop: Some(fidl_ip!("2.2.3.1")),
            },
            Route {
                destination: fidl_subnet!("::0/0"),
                outbound_interface: ID1,
                next_hop: Some(fidl_ip!("223::1")),
            },
        ]);

        const NON_ETHERNET_INTERFACE_NAME: &str = "test01";

        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let time = fasync::MonotonicInstant::from_nanos(1_000_000_000);
        let () = exec.set_fake_time(time.into());

        let got = run_network_check(
            &mut exec,
            &fnet_interfaces_ext::Properties {
                id: ID1.try_into().expect("should be nonzero"),
                name: NON_ETHERNET_INTERFACE_NAME.to_string(),
                port_class: fnet_interfaces_ext::PortClass::Virtual,
                online: false,
                has_default_ipv4_route: false,
                has_default_ipv6_route: false,
                addresses: vec![],
                port_identity_koid: Default::default(),
            },
            &Default::default(),
            None,
            FakePing::default(),
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect(
            "error calling network check with non-ethernet interface, no addresses, interface down",
        );
        assert_eq!(
            got,
            Some(IpVersions::construct(StateEvent {
                state: State { link: LinkState::Down, ..Default::default() },
                time
            }))
        );

        let got = run_network_check(
            &mut exec,
            &fnet_interfaces_ext::Properties { online: false, ..properties.clone() },
            &Default::default(),
            None,
            FakePing::default(),
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect("error calling network check, want Down state");
        let want = Some(IpVersions::<StateEvent>::construct(StateEvent {
            state: State { link: LinkState::Down, ..Default::default() },
            time,
        }));
        assert_eq!(got, want);

        let got = run_network_check(
            &mut exec,
            &fnet_interfaces_ext::Properties {
                has_default_ipv4_route: false,
                has_default_ipv6_route: false,
                ..properties.clone()
            },
            &local_routes,
            None,
            FakePing::default(),
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect("error calling network check, want Local state due to no default routes");
        let want = Some(IpVersions::<StateEvent>::construct(StateEvent {
            state: State { link: LinkState::Local, ..Default::default() },
            time,
        }));
        assert_eq!(got, want);

        let got = run_network_check(
            &mut exec,
            &properties,
            &route_table2,
            None,
            FakePing::default(),
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect("error calling network check, want Local state due to no matching default route");
        let want = Some(IpVersions::<StateEvent>::construct(StateEvent {
            state: State { link: LinkState::Local, ..Default::default() },
            time,
        }));
        assert_eq!(got, want);

        let got = run_network_check(
            &mut exec,
            &properties,
            &route_table,
            None,
            FakePing {
                gateway_addrs: [std_ip!("1.2.3.1"), std_ip!("123::1")].into_iter().collect(),
                gateway_response: true,
                internet_response: false,
            },
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect("error calling network check, want Gateway state");
        let want = Some(IpVersions::<StateEvent>::construct(StateEvent {
            state: State { link: LinkState::Gateway, ..Default::default() },
            time,
        }));
        assert_eq!(got, want);

        let got = run_network_check(
            &mut exec,
            &properties,
            &route_table,
            None,
            FakePing {
                gateway_addrs: [std_ip!("1.2.3.1"), std_ip!("123::1")].into_iter().collect(),
                gateway_response: true,
                internet_response: true,
            },
            FakeDig::default(),
            FakeFetch::default(),
        )
        .expect("error calling network check, want Internet state");
        let want = Some(IpVersions::<StateEvent>::construct(StateEvent {
            state: State { link: LinkState::Internet, ..Default::default() },
            time,
        }));
        assert_eq!(got, want);
    }

    fn update_delta(port: Delta<StateEvent>, system: Delta<SystemState>) -> StateDelta {
        StateDelta {
            port: IpVersions { ipv4: port.clone(), ipv6: port },
            system: IpVersions { ipv4: system.clone(), ipv6: system },
        }
    }

    #[test]
    fn test_state_info_update() {
        let if1_local_event = StateEvent::construct(LinkState::Local);
        let if1_local = IpVersions::<StateEvent>::construct(if1_local_event);
        // Post-update the system state should be Local due to interface 1.
        let mut state = StateInfo::default();
        let want = update_delta(
            Delta { previous: None, current: if1_local_event },
            Delta { previous: None, current: SystemState { id: ID1, state: if1_local_event } },
        );
        assert_eq!(state.update(ID1, if1_local.clone()), want);
        let want_state = StateInfo {
            per_interface: std::iter::once((ID1, if1_local.clone())).collect::<HashMap<_, _>>(),
            system: IpVersions { ipv4: Some(ID1), ipv6: Some(ID1) },
        };
        assert_eq!(state, want_state);

        let if2_gateway_event = StateEvent::construct(LinkState::Gateway);
        let if2_gateway = IpVersions::<StateEvent>::construct(if2_gateway_event);
        // Pre-update, the system state is Local due to interface 1; post-update the system state
        // will be Gateway due to interface 2.
        let want = update_delta(
            Delta { previous: None, current: if2_gateway_event },
            Delta {
                previous: Some(SystemState { id: ID1, state: if1_local_event }),
                current: SystemState { id: ID2, state: if2_gateway_event },
            },
        );
        assert_eq!(state.update(ID2, if2_gateway.clone()), want);
        let want_state = StateInfo {
            per_interface: [(ID1, if1_local.clone()), (ID2, if2_gateway.clone())]
                .into_iter()
                .collect::<HashMap<_, _>>(),
            system: IpVersions { ipv4: Some(ID2), ipv6: Some(ID2) },
        };
        assert_eq!(state, want_state);

        let if2_removed_event = StateEvent::construct(LinkState::Removed);
        let if2_removed = IpVersions::<StateEvent>::construct(if2_removed_event);
        // Pre-update, the system state is Gateway due to interface 2; post-update the system state
        // will be Local due to interface 1.
        let want = update_delta(
            Delta { previous: Some(if2_gateway_event), current: if2_removed_event },
            Delta {
                previous: Some(SystemState { id: ID2, state: if2_gateway_event }),
                current: SystemState { id: ID1, state: if1_local_event },
            },
        );
        assert_eq!(state.update(ID2, if2_removed.clone()), want);
        let want_state = StateInfo {
            per_interface: [(ID1, if1_local.clone()), (ID2, if2_removed.clone())]
                .into_iter()
                .collect::<HashMap<_, _>>(),
            system: IpVersions { ipv4: Some(ID1), ipv6: Some(ID1) },
        };
        assert_eq!(state, want_state);
    }

    // Regression test against https://fxbug.dev/439597080
    // Confirm that a new event with the same state as the current system state does not change
    // the id of the system state value.
    #[test]
    fn test_state_info_update_same_link_state() {
        let if_local_event = StateEvent::construct(LinkState::Local);
        let if_local = IpVersions::<StateEvent>::construct(if_local_event);
        // Post-update the system state should be Local due to interface 1.
        let mut state = StateInfo::default();
        let want = update_delta(
            Delta { previous: None, current: if_local_event },
            Delta { previous: None, current: SystemState { id: ID1, state: if_local_event } },
        );
        assert_eq!(state.update(ID1, if_local.clone()), want);
        let want_state = StateInfo {
            per_interface: std::iter::once((ID1, if_local.clone())).collect::<HashMap<_, _>>(),
            system: IpVersions { ipv4: Some(ID1), ipv6: Some(ID1) },
        };
        assert_eq!(state, want_state);

        // Post-update the system state should be the same due to the interface 2 having the
        // same state.
        let want = update_delta(
            Delta { previous: None, current: if_local_event },
            Delta {
                previous: Some(SystemState { id: ID1, state: if_local_event }),
                current: SystemState { id: ID1, state: if_local_event },
            },
        );
        assert_eq!(state.update(ID2, if_local.clone()), want);
        let want_state = StateInfo {
            per_interface: [(ID1, if_local.clone()), (ID2, if_local.clone())]
                .into_iter()
                .collect::<HashMap<_, _>>(),
            system: IpVersions { ipv4: Some(ID1), ipv6: Some(ID1) },
        };
        assert_eq!(state, want_state);

        // Post-update the system state should reflect interface 2 having the system state since
        // interface 1 is now at a strictly worse state.
        let if_removed_event = StateEvent::construct(LinkState::Removed);
        let if_removed = IpVersions::<StateEvent>::construct(if_removed_event);
        let want = update_delta(
            Delta { previous: Some(if_local_event), current: if_removed_event },
            Delta {
                previous: Some(SystemState { id: ID1, state: if_local_event }),
                current: SystemState { id: ID2, state: if_local_event },
            },
        );
        assert_eq!(state.update(ID1, if_removed.clone()), want);
        let want_state = StateInfo {
            per_interface: [(ID1, if_removed.clone()), (ID2, if_local.clone())]
                .into_iter()
                .collect::<HashMap<_, _>>(),
            system: IpVersions { ipv4: Some(ID2), ipv6: Some(ID2) },
        };
        assert_eq!(state, want_state);
    }

    #[test_case(None::<LinkState>, None::<LinkState>, false, false, false, false;
        "no interfaces available")]
    #[test_case(Some(LinkState::Local), Some(LinkState::Local), false, false, false, false;
        "no interfaces with gateway or internet state")]
    #[test_case(Some(LinkState::Local), Some(LinkState::Gateway), false, false, false, true;
        "only one interface with gateway state or above")]
    #[test_case(Some(LinkState::Local), Some(LinkState::Internet), false, false, true, true;
        "only one interface with internet state")]
    #[test_case(Some(LinkState::Internet), Some(LinkState::Internet), false, false, true, true;
        "all interfaces with internet")]
    #[test_case(Some(LinkState::Internet), None::<LinkState>, false, false, true, true;
        "only one interface available, has internet state")]
    #[test_case(Some(LinkState::Local), Some((LinkState::Internet, true, false)), false, true, true, true;
        "only one interface with DNS resolved state")]
    #[test_case(Some((LinkState::Internet, true, false)), Some((LinkState::Internet, true, false)), false, true, true, true;
        "all interfaces with DNS resolved state")]
    #[test_case(Some((LinkState::Internet, true, false)), None::<LinkState>, false, true, true, true;
        "only one interface available, has DNS resolved state")]
    #[test_case(Some(LinkState::Local), Some((LinkState::Internet, true, true)), true, true, true, true;
        "only one interface with HTTP resolved state")]
    #[test_case(Some((LinkState::Internet, true, true)), Some((LinkState::Internet, true, true)), true, true, true, true;
        "all interfaces with HTTP resolved state")]
    #[test_case(Some((LinkState::Internet, true, true)), None::<LinkState>, true, true, true, true;
        "only one interface available, has HTTP resolved state")]
    #[test_case(Some((LinkState::Internet, false, true)), None::<LinkState>, true, false, true, true;
        "only one interface available, has HTTP resolved state, but no DNS")]
    fn test_system_has_state<S1, S2>(
        ipv4_state: Option<S1>,
        ipv6_state: Option<S2>,
        expect_http: bool,
        expect_dns: bool,
        expect_internet: bool,
        expect_gateway: bool,
    ) where
        StateEvent: Construct<S1>,
        StateEvent: Construct<S2>,
    {
        let if1 = ipv4_state
            .map(|state| IpVersions::<StateEvent>::construct(StateEvent::construct(state)));
        let if2 = ipv6_state
            .map(|state| IpVersions::<StateEvent>::construct(StateEvent::construct(state)));

        let mut system_interfaces: HashMap<u64, IpVersions<StateEvent>> = HashMap::new();

        let system_interface_ipv4 = if1.map(|interface| {
            let _ = system_interfaces.insert(ID1, interface);
            ID1
        });

        let system_interface_ipv6 = if2.map(|interface| {
            let _ = system_interfaces.insert(ID2, interface);
            ID2
        });

        let state = StateInfo {
            per_interface: system_interfaces,
            system: IpVersions { ipv4: system_interface_ipv4, ipv6: system_interface_ipv6 },
        };

        assert_eq!(state.system_has_http(), expect_http);
        assert_eq!(state.system_has_dns(), expect_dns);
        assert_eq!(state.system_has_internet(), expect_internet);
        assert_eq!(state.system_has_gateway(), expect_gateway);
    }

    #[test]
    fn test_resume_after_interface_removed() {
        use assert_matches::assert_matches;

        let _exec = fasync::TestExecutor::new();
        let (sender, _receiver) = mpsc::unbounded::<(NetworkCheckAction, NetworkCheckCookie)>();
        let mut monitor: Monitor<MonotonicInstant> = Monitor::new(sender).unwrap();

        let properties = fnet_interfaces_ext::Properties {
            id: ID1.try_into().expect("should be nonzero"),
            name: ETHERNET_INTERFACE_NAME.to_string(),
            port_class: fnet_interfaces_ext::PortClass::Ethernet,
            online: false,
            addresses: vec![],
            has_default_ipv4_route: false,
            has_default_ipv6_route: false,
            port_identity_koid: Default::default(),
        };

        // Insert a placeholder state so that the interface is tracked.
        let initial_state = IpVersions {
            ipv4: StateEvent {
                state: State { link: LinkState::None, ..Default::default() },
                time: fasync::MonotonicInstant::now(),
            },
            ipv6: StateEvent {
                state: State { link: LinkState::None, ..Default::default() },
                time: fasync::MonotonicInstant::now(),
            },
        };
        monitor.update_state(ID1, ETHERNET_INTERFACE_NAME, initial_state);

        // Remove the interface. All future updates involving this interface should cause no
        // change to the interface's state.
        monitor.handle_interface_removed(properties.clone());

        // Assert that the state is now `Removed`.
        let removed_state = monitor.state().get(ID1).unwrap();
        assert_eq!(removed_state.ipv4.state.link, LinkState::Removed);
        assert_eq!(removed_state.ipv6.state.link, LinkState::Removed);

        // Start another iteration of the network check to ensure that any future state updates
        // do not affect the `Removed` state. In practice, the network check may be in-progress
        // when a removal event is received. That new state should not override the
        // `Removed` state.
        let routes = testutil::build_route_table_from_flattened_routes([]);
        let view = InterfaceView { properties: &properties, routes: &routes, neighbors: None };
        assert_matches!(monitor.begin(view), Ok(NetworkCheckerOutcome::Complete));

        // Confirm that the LinkState discovered from the network check was `Down` and
        // not `Removed`.
        let interface_context = monitor.interface_context.get(&ID1).unwrap();
        assert_matches!(interface_context.discovered_state.ipv4.link, LinkState::Down);
        assert_matches!(interface_context.discovered_state.ipv6.link, LinkState::Down);

        // Assert that the state is still `Removed`, and was not updated to `Down`
        // by the completed network check's result.
        let final_state = monitor.state().get(ID1).unwrap();
        assert_eq!(final_state.ipv4.state.link, LinkState::Removed);
        assert_eq!(final_state.ipv6.state.link, LinkState::Removed);
    }

    #[test]
    fn test_ping_and_fetch_telemetry_events_sent() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let time = fasync::MonotonicInstant::from_nanos(1_000_000_000);
        let () = exec.set_fake_time(time.into());

        let (action_sender, action_receiver) = mpsc::unbounded();
        let mut monitor = Monitor::new_with_time_provider(
            action_sender,
            FakeTime {
                increment: zx::MonotonicDuration::from_nanos(10),
                time: zx::MonotonicInstant::get(),
            },
        )
        .unwrap();

        let (telemetry_tx, mut telemetry_rx) = mpsc::channel(100);
        monitor.set_telemetry_sender(TelemetrySender::new(telemetry_tx));

        let properties = &fnet_interfaces_ext::Properties {
            id: ID1.try_into().unwrap(),
            name: ETHERNET_INTERFACE_NAME.to_string(),
            port_class: fnet_interfaces_ext::PortClass::Ethernet,
            online: true,
            addresses: vec![],
            has_default_ipv4_route: true,
            has_default_ipv6_route: false,
            port_identity_koid: Default::default(),
        };

        let net_gateway = fidl_ip!("192.168.0.254");
        let net_gateway_std = std_ip!("192.168.0.254");

        // Needs to have at least a default route so it attempts internet test
        let routes = testutil::build_route_table_from_flattened_routes([Route {
            destination: UNSPECIFIED_V4,
            outbound_interface: ID1,
            next_hop: Some(net_gateway),
        }]);

        // Setup neighbors for gateway caching
        let neighbors_map = [(
            net_gateway,
            NeighborState::new(NeighborHealth::Healthy {
                last_observed: zx::MonotonicInstant::default(),
            }),
        )]
        .into_iter()
        .collect::<HashMap<fnet::IpAddress, NeighborState>>();
        let neighbors = InterfaceNeighborCache { neighbors: neighbors_map };

        let view = InterfaceView { properties, routes: &routes, neighbors: Some(&neighbors) };

        let mut network_check_responder = NetworkCheckTestResponder::new(action_receiver);

        let pinger = FakePing {
            gateway_addrs: vec![net_gateway_std].into_iter().collect(),
            gateway_response: true,
            internet_response: true,
        };
        let digger = FakeDig::new(vec![std_ip!("1.2.3.0")]);
        let fetcher = FakeFetch::default();

        let network_check_fut = async {
            match monitor.begin(view) {
                Ok(NetworkCheckerOutcome::MustResume) => {
                    let () = network_check_responder
                        .respond_to_messages(&mut monitor, pinger, digger, fetcher)
                        .await;
                }
                _ => panic!("Expected MustResume"),
            }
        };

        let mut network_check_fut = pin!(network_check_fut);
        match exec.run_until_stalled(&mut network_check_fut) {
            Poll::Ready(()) => {}
            Poll::Pending => panic!("network_check blocked unexpectedly"),
        }

        let mut events = Vec::new();
        while let Poll::Ready(Some(event)) = exec.run_until_stalled(&mut telemetry_rx.next()) {
            events.push(event);
        }

        let has_gateway =
            events.iter().any(|e| matches!(e, TelemetryEvent::GatewayPingResult { .. }));
        let has_internet =
            events.iter().any(|e| matches!(e, TelemetryEvent::InternetPingResult { .. }));
        let has_fetch = events.iter().any(|e| matches!(e, TelemetryEvent::FetchResult { .. }));

        assert!(has_gateway, "Expected GatewayPingResult telemetry event");
        assert!(has_internet, "Expected InternetPingResult telemetry event");
        assert!(has_fetch, "Expected FetchResult telemetry event");
    }
}
