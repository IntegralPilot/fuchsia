// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};

use fidl::prelude::*;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces::{
    self as finterfaces, StateRequest, StateRequestStream, WatcherRequest, WatcherRequestStream,
    WatcherWatchResponder,
};
use fidl_fuchsia_net_interfaces_ext as finterfaces_ext;
use futures::channel::mpsc;
use futures::sink::SinkExt as _;
use futures::task::Poll;
use futures::{
    Future, FutureExt as _, StreamExt as _, TryFutureExt as _, TryStreamExt as _, ready,
};
use log::{debug, error, warn};
use net_types::ip::{AddrSubnetEither, IpAddr, IpVersion};
use netstack3_core::ip::{IpAddressState, PreferredLifetime};
use thiserror::Error;

use crate::bindings::devices::BindingId;
use crate::bindings::util::{ErrorLogExt, IntoFidl, ResultExt as _, TryIntoFidl as _};

/// Possible errors when serving `fuchsia.net.interfaces/State`.
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
            Error::Send(WorkerClosedError {}) => log::Level::Error,
            Error::Fidl(fidl) => fidl.log_level(),
        }
    }
}

#[cfg_attr(test, derive(Default))]
pub(crate) struct WatcherOptions {
    address_properties_interest: finterfaces::AddressPropertiesInterest,
    include_non_assigned_addresses: bool,
    port_identity_koid_filter: Option<zx::Koid>,
}

/// Serves the `fuchsia.net.interfaces/State` protocol.
pub(crate) async fn serve(
    stream: StateRequestStream,
    sink: WorkerWatcherSink,
) -> Result<(), Error> {
    stream
        .err_into()
        .try_fold(sink, |mut sink, req| async move {
            let _ = &req;
            let StateRequest::GetWatcher {
                options:
                    finterfaces::WatcherOptions {
                        address_properties_interest,
                        include_non_assigned_addresses,
                        port_identity_koid_filter,
                        __source_breaking,
                    },
                watcher,
                control_handle: _,
            } = req;
            let watcher = watcher.into_stream();
            sink.add_watcher(
                watcher,
                WatcherOptions {
                    address_properties_interest: address_properties_interest
                        .unwrap_or_else(finterfaces::AddressPropertiesInterest::default),
                    include_non_assigned_addresses: include_non_assigned_addresses.unwrap_or(false),
                    port_identity_koid_filter: port_identity_koid_filter.map(zx::Koid::from_raw),
                },
            )
            .await?;
            Ok(sink)
        })
        .map_ok(|_: WorkerWatcherSink| ())
        .await
}

/// The maximum events to buffer at server side before the client consumes them.
///
/// The value is currently kept in sync with the netstack2 implementation.
const MAX_EVENTS: usize = 128;

#[derive(Debug)]
/// A bounded queue of [`Events`] for `fuchsia.net.interfaces/Watcher` protocol.
struct EventQueue {
    events: VecDeque<finterfaces::Event>,
}

impl EventQueue {
    /// Creates a new event queue containing all the interfaces in `state`
    /// wrapped in a [`finterfaces::Event::Existing`] followed by a
    /// [`finterfaces::Event::Idle`].
    fn from_state(
        state: &HashMap<BindingId, InterfaceState>,
        watcher_options: &WatcherOptions,
    ) -> Result<Self, zx::Status> {
        // NB: Leave room for idle event.
        if state.len() >= MAX_EVENTS {
            return Err(zx::Status::BUFFER_TOO_SMALL);
        }
        // NB: Compiler can't infer the parameter types.
        let state_to_event = |(id, state): (&BindingId, &InterfaceState)| {
            let InterfaceState {
                properties,
                addresses,
                has_default_ipv4_route,
                has_default_ipv6_route,
                enabled,
                publish_to_fidl,
            } = state;
            let InterfaceProperties { name, port_class, port_identity_koid } = properties;
            let publish = *publish_to_fidl && watcher_options.should_track_interface(properties);
            publish.then(|| {
                let mut event = finterfaces::Event::Existing(
                    finterfaces_ext::Properties {
                        id: *id,
                        name: name.clone(),
                        online: enabled.online(),
                        addresses: Worker::collect_addresses(addresses),
                        has_default_ipv4_route: *has_default_ipv4_route,
                        has_default_ipv6_route: *has_default_ipv6_route,
                        port_class: *port_class,
                        port_identity_koid: *port_identity_koid,
                    }
                    .into(),
                );
                watcher_options.apply_interest(&mut event);
                event
            })
        };
        Ok(Self {
            events: state
                .iter()
                .filter_map(state_to_event)
                .chain(std::iter::once(finterfaces::Event::Idle(finterfaces::Empty {})))
                .collect(),
        })
    }

    /// Adds a [`finterfaces::Event`] to the back of the queue.
    fn push(&mut self, event: finterfaces::Event) -> Result<(), finterfaces::Event> {
        let Self { events } = self;
        if events.len() >= MAX_EVENTS {
            return Err(event);
        }
        // NB: We could perform event consolidation here, but that's not
        // implemented in NS2 at the moment of writing so it's easier to match
        // behavior.
        events.push_back(event);
        Ok(())
    }

    /// Removes an [`Event`] from the front of the queue.
    fn pop_front(&mut self) -> Option<finterfaces::Event> {
        let Self { events } = self;
        events.pop_front()
    }
}

/// The task that serves `fuchsia.net.interfaces/Watcher`.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub(crate) struct Watcher {
    stream: WatcherRequestStream,
    options: WatcherOptions,
    events: EventQueue,
    responder: Option<WatcherWatchResponder>,
}

impl Future for Watcher {
    type Output = Result<(), fidl::Error>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        loop {
            let next_request = self.as_mut().stream.poll_next_unpin(cx)?;
            match ready!(next_request) {
                Some(WatcherRequest::Watch { responder }) => match self.events.pop_front() {
                    Some(e) => responder.send(&e).unwrap_or_log("failed to respond"),
                    None => match &self.responder {
                        Some(existing) => {
                            existing
                                .control_handle()
                                .shutdown_with_epitaph(zx::Status::ALREADY_EXISTS);
                            return Poll::Ready(Ok(()));
                        }
                        None => {
                            self.responder = Some(responder);
                        }
                    },
                },
                None => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl WatcherOptions {
    fn should_push_event_from_outcome(&self, outcome: &ConsumeEventOutcome<'_>) -> bool {
        let Self {
            address_properties_interest,
            include_non_assigned_addresses,
            port_identity_koid_filter: _,
        } = self;
        let ConsumeEventOutcome { event: _, changed_address_properties, properties } = outcome;

        if !self.should_track_interface(properties) {
            return false;
        }

        let is_address_visible = |involves_assigned| {
            // The address is visible if the change involves an assigned address
            // since all watchers receive updates involving an assigned address.
            // If the address change involves state(s) that are not considered
            // assigned, then the change is only visible to watchers that
            // request addresses that are not assigned.
            involves_assigned || *include_non_assigned_addresses
        };

        let should_push = match &changed_address_properties {
            ChangedAddressProperties::InterestNotApplicable => true,
            ChangedAddressProperties::PropertiesChanged { address_properties, is_assigned } => {
                address_properties_interest.intersects(address_properties.clone())
                    && is_address_visible(*is_assigned)
            }
            ChangedAddressProperties::AssignmentStateChanged { involves_assigned } => {
                is_address_visible(*involves_assigned)
            }
        };

        should_push
    }

    fn should_track_interface(&self, properties: &InterfaceProperties) -> bool {
        let Self {
            address_properties_interest: _,
            include_non_assigned_addresses: _,
            port_identity_koid_filter,
        } = self;
        port_identity_koid_filter.is_none_or(|f| properties.port_identity_koid == Some(f))
    }

    fn apply_interest(&self, event: &mut finterfaces::Event) {
        let Self {
            address_properties_interest,
            include_non_assigned_addresses,
            port_identity_koid_filter: _,
        } = self;
        let addresses = match event {
            finterfaces::Event::Existing(finterfaces::Properties { addresses, .. })
            | finterfaces::Event::Added(finterfaces::Properties { addresses, .. })
            | finterfaces::Event::Changed(finterfaces::Properties { addresses, .. }) => {
                addresses.as_mut()
            }
            finterfaces::Event::Idle(finterfaces::Empty {}) => None,
            finterfaces::Event::Removed(id) => {
                let _: &mut u64 = id;
                None
            }
        };

        let Some(addresses) = addresses else {
            return;
        };
        addresses.retain_mut(
            |finterfaces::Address {
                 assignment_state,
                 addr: _,
                 valid_until,
                 preferred_lifetime_info,
                 __source_breaking: fidl::marker::SourceBreaking,
             }| {
                // Clear all fields that the watcher has no interest in.
                if !address_properties_interest
                    .contains(finterfaces::AddressPropertiesInterest::VALID_UNTIL)
                {
                    *valid_until = None;
                }
                if !address_properties_interest
                    .contains(finterfaces::AddressPropertiesInterest::PREFERRED_LIFETIME_INFO)
                {
                    *preferred_lifetime_info = None;
                }

                match assignment_state.expect("required field") {
                    finterfaces::AddressAssignmentState::Assigned => true,
                    finterfaces::AddressAssignmentState::Unavailable
                    | finterfaces::AddressAssignmentState::Tentative => {
                        *include_non_assigned_addresses
                    }
                }
            },
        )
    }
}

impl Watcher {
    /// Attempts to push an `outcome` to the watcher queue.
    ///
    /// Returns `true` if the event was cloned and put in the watcher queue
    /// successfully.
    fn push(&mut self, outcome: &ConsumeEventOutcome<'_>) -> bool {
        let Self { stream, events, responder, options } = self;

        if !options.should_push_event_from_outcome(outcome) {
            return false;
        }
        let ConsumeEventOutcome { event, changed_address_properties: _, properties: _ } = outcome;
        // Clone the event to apply interest.
        let mut event = event.clone();
        options.apply_interest(&mut event);

        if let Some(responder) = responder.take() {
            let published = match responder.send(&event) {
                Ok(()) => true,
                Err(e) => {
                    if !e.is_closed() {
                        error!("error sending event {:?} to watcher: {:?}", event, e)
                    }
                    false
                }
            };
            return published;
        }
        match events.push(event) {
            Ok(()) => true,
            Err(event) => {
                warn!("failed to enqueue event {:?} on watcher, closing channel", event);
                stream.control_handle().shutdown();
                false
            }
        }
    }
}

/// Interface specific events.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone, Eq, PartialEq))]
pub(crate) enum InterfaceUpdate {
    AddressAdded {
        addr: AddrSubnetEither,
        assignment_state: IpAddressState,
        valid_until: zx::MonotonicInstant,
        preferred_lifetime: PreferredLifetime<zx::MonotonicInstant>,
    },
    AddressAssignmentStateChanged {
        addr: IpAddr,
        new_state: IpAddressState,
    },
    AddressPropertiesChanged {
        addr: IpAddr,
        update: AddressPropertiesUpdate,
    },
    AddressRemoved(IpAddr),
    DefaultRouteChanged {
        version: IpVersion,
        has_default_route: bool,
    },
    IpEnabledChanged {
        version: IpVersion,
        enabled: bool,
    },
}

/// Changes to address properties (e.g. via interfaces-admin).
#[derive(Debug)]
#[cfg_attr(test, derive(Clone, Eq, PartialEq))]
pub(crate) struct AddressPropertiesUpdate {
    pub(crate) valid_until: zx::MonotonicInstant,
    pub(crate) preferred_lifetime: PreferredLifetime<zx::MonotonicInstant>,
}

/// Immutable interface properties.
#[derive(Debug, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub(crate) struct InterfaceProperties {
    pub(crate) name: String,
    pub(crate) port_class: finterfaces_ext::PortClass,
    pub(crate) port_identity_koid: Option<zx::Koid>,
}

/// Cached interface state by the worker.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone, Eq, PartialEq))]
pub(crate) struct InterfaceState {
    properties: InterfaceProperties,
    enabled: IpEnabledState,
    addresses: HashMap<IpAddr, AddressProperties>,
    has_default_ipv4_route: bool,
    has_default_ipv6_route: bool,
    publish_to_fidl: bool,
}

/// Caches IPv4 and IPv6 enabled state to produce a unified `online` signal.
///
/// The split signals come directly from core. We consider an interface online
/// if _either_ v4 or v6 are up.
#[derive(Debug, Default, Copy, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
struct IpEnabledState {
    v4: bool,
    v6: bool,
}

impl IpEnabledState {
    /// Updates the enabled state for `version` returning the new value for
    /// `online` if it changed.
    fn update(&mut self, version: IpVersion, enabled: bool) -> Option<bool> {
        let before = self.online();
        let Self { v4, v6 } = self;
        let target = match version {
            IpVersion::V4 => v4,
            IpVersion::V6 => v6,
        };
        *target = enabled;
        let after = self.online();
        (after != before).then_some(after)
    }

    fn online(&self) -> bool {
        let Self { v4, v6 } = self;
        *v4 || *v6
    }
}

#[derive(Debug)]
#[cfg_attr(test, derive(Clone, Eq, PartialEq))]
struct AddressProperties {
    prefix_len: u8,
    state: AddressState,
}

/// Cached address state by the worker.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone, Eq, PartialEq))]
pub(crate) struct AddressState {
    pub(crate) valid_until: zx::MonotonicInstant,
    pub(crate) assignment_state: IpAddressState,
    pub(crate) preferred_lifetime: PreferredLifetime<zx::MonotonicInstant>,
}

#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub(crate) enum InterfaceEvent {
    Added { id: BindingId },
    Reserved { id: BindingId, properties: InterfaceProperties },
    Changed { id: BindingId, event: InterfaceUpdate },
    Removed(BindingId),
}

#[derive(Debug)]
pub(crate) struct InterfaceEventProducer {
    id: BindingId,
    channel: mpsc::UnboundedSender<InterfaceEvent>,
}

impl InterfaceEventProducer {
    /// Notifies the interface state [`Worker`] of [`event`] on this
    /// [`InterfaceEventProducer`]'s interface.
    pub(crate) fn notify(&self, event: InterfaceUpdate) -> Result<(), InterfaceUpdate> {
        let Self { id, channel } = self;
        channel.unbounded_send(InterfaceEvent::Changed { id: *id, event }).map_err(|e| {
            match e.into_inner() {
                InterfaceEvent::Changed { id: _, event } => event,
                // All other patterns are unreachable, this is only so we can get
                // back the event we just created above.
                e => unreachable!("{:?}", e),
            }
        })
    }

    pub(crate) fn start_publishing(&self) -> Result<(), WorkerClosedError> {
        let Self { id, channel } = self;
        channel
            .unbounded_send(InterfaceEvent::Added { id: *id })
            .map_err(|_: mpsc::TrySendError<_>| WorkerClosedError {})
    }
}

impl Drop for InterfaceEventProducer {
    fn drop(&mut self) {
        let Self { id, channel } = self;
        channel.unbounded_send(InterfaceEvent::Removed(*id)).unwrap_or({
            // If the worker was closed before its producers we can assume
            // it's no longer interested in events, so we simply drop these
            // errors.
        })
    }
}

#[derive(Error, Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub(crate) enum WorkerError {
    #[error("attempted to reinsert interface {interface} over old {old:?}")]
    AddedDuplicateInterface { interface: BindingId, old: InterfaceState },
    #[error("attempted to remove nonexisting interface with id {0}")]
    RemoveNonexistentInterface(BindingId),
    #[error("attempted to update nonexisting interface with id {0}")]
    UpdateNonexistentInterface(BindingId),
    #[error("attempted to publish nonexisting interface with id {0}")]
    PublishedNonexistentInterface(BindingId),
    #[error("attempted to publish interface {0} again")]
    PublishedDuplicateInterface(BindingId),
    #[error("attempted to assign already assigned address {addr} on interface {interface}")]
    AssignExistingAddr { interface: BindingId, addr: IpAddr },
    #[error("attempted to unassign nonexisting interface address {addr} on interface {interface}")]
    UnassignNonexistentAddr { interface: BindingId, addr: IpAddr },
    #[error(
        "attempted to update assignment state to {state:?} on non existing interface address {addr} on interface {interface}"
    )]
    UpdateStateOnNonexistentAddr { interface: BindingId, addr: IpAddr, state: IpAddressState },
    #[error(
        "attempted to update properties with {update:?} on non existing interface address {addr} on interface {interface}"
    )]
    UpdatePropertiesOnNonexistentAddr {
        interface: BindingId,
        addr: IpAddr,
        update: AddressPropertiesUpdate,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ChangedAddressProperties {
    InterestNotApplicable,
    PropertiesChanged {
        address_properties: finterfaces::AddressPropertiesInterest,
        is_assigned: bool,
    },
    AssignmentStateChanged {
        involves_assigned: bool,
    },
}

#[derive(Default)]
pub(crate) struct RunOptions {
    #[cfg(test)]
    pub(crate) spy_interface_events:
        Option<futures::channel::mpsc::UnboundedSender<InterfaceEvent>>,
}

pub(crate) struct Worker {
    events: mpsc::UnboundedReceiver<InterfaceEvent>,
    watchers: mpsc::Receiver<NewWatcher>,
    pub(crate) run_options: RunOptions,
}
/// Arbitrarily picked constant to force backpressure on FIDL requests.
const WATCHER_CHANNEL_CAPACITY: usize = 32;

fn is_assigned(state: IpAddressState) -> bool {
    match state {
        IpAddressState::Assigned => true,
        IpAddressState::Unavailable | IpAddressState::Tentative => false,
    }
}

impl Worker {
    pub(crate) fn new() -> (Worker, WorkerWatcherSink, WorkerInterfaceSink) {
        let (events_sender, events_receiver) = mpsc::unbounded();
        let (watchers_sender, watchers_receiver) = mpsc::channel(WATCHER_CHANNEL_CAPACITY);
        (
            Worker {
                events: events_receiver,
                watchers: watchers_receiver,
                run_options: RunOptions::default(),
            },
            WorkerWatcherSink { sender: watchers_sender },
            WorkerInterfaceSink { sender: events_sender },
        )
    }

    /// Runs the worker until all [`WorkerWatcherSink`]s and
    /// [`WorkerInterfaceSink`]s are closed.
    ///
    /// On success, returns the set of currently opened [`Watcher`]s that the
    /// `Worker` was polling on when all its sinks were closed.
    pub(crate) async fn run(
        self,
    ) -> Result<futures::stream::FuturesUnordered<Watcher>, WorkerError> {
        let Self { events, watchers: watchers_stream, run_options } = self;
        let mut current_watchers = futures::stream::FuturesUnordered::<Watcher>::new();
        let mut interface_state = HashMap::new();

        enum SinkAction {
            NewWatcher(NewWatcher),
            Event(InterfaceEvent),
        }
        let mut sink_actions = futures::stream::select_with_strategy(
            watchers_stream.map(SinkAction::NewWatcher),
            events.map(SinkAction::Event),
            // Always consume events before watchers. That allows external
            // observers to assume all side effects of a call are already
            // applied before a watcher observes its initial existing set of
            // properties.
            |_: &mut ()| futures::stream::PollNext::Right,
        );

        loop {
            let mut poll_watchers = if current_watchers.is_empty() {
                futures::future::pending().left_future()
            } else {
                current_watchers.by_ref().next().right_future()
            };

            // NB: Declare an enumeration with actions to prevent too much logic
            // in select macro.
            enum Action {
                WatcherEnded(Option<Result<(), fidl::Error>>),
                Sink(Option<SinkAction>),
            }
            let action = futures::select! {
                r = poll_watchers => Action::WatcherEnded(r),
                a = sink_actions.next() => Action::Sink(a),
            };

            match action {
                Action::WatcherEnded(r) => match r {
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        if !e.is_closed() {
                            error!("error operating interface watcher {:?}", e);
                        }
                    }
                    // This should not be observable since we check if our
                    // watcher collection is empty above and replace it with a
                    // pending future.
                    None => unreachable!("should not observe end of FuturesUnordered"),
                },
                Action::Sink(Some(SinkAction::NewWatcher(NewWatcher {
                    watcher: stream,
                    options,
                }))) => match EventQueue::from_state(&interface_state, &options) {
                    Ok(events) => {
                        current_watchers.push(Watcher { stream, options, events, responder: None })
                    }
                    Err(status) => {
                        warn!("failed to construct events for watcher: {}", status);
                        stream.control_handle().shutdown_with_epitaph(status);
                    }
                },
                Action::Sink(Some(SinkAction::Event(e))) => {
                    debug!("consuming event {:?}", e);

                    #[cfg(test)]
                    {
                        let RunOptions { spy_interface_events } = &run_options;
                        match spy_interface_events {
                            Some(sink) => sink.unbounded_send(e.clone()).unwrap_or(
                                // To simplify teardown, we allow tests to
                                // drop the spy receiver.
                                (),
                            ),
                            None => (),
                        };
                    }
                    #[cfg(not(test))]
                    let RunOptions {} = &run_options;

                    match Self::consume_event(&mut interface_state, e)? {
                        None => debug!("not publishing No-Op event."),
                        Some(outcome) => {
                            let num_published = current_watchers
                                .iter_mut()
                                .filter_map(|watcher| watcher.push(&outcome).then_some(()))
                                .count();
                            debug!(
                                "published event to {} of {} watchers",
                                num_published,
                                current_watchers.len()
                            );
                        }
                    }
                }
                // If all of the sinks close, shutdown the worker.
                Action::Sink(None) => {
                    return Ok(current_watchers);
                }
            }
        }
    }

    /// Consumes a single worker event, mutating state.
    ///
    /// On `Err`, the worker must be stopped and `state` can't be considered
    /// valid anymore.
    fn consume_event(
        state: &mut HashMap<BindingId, InterfaceState>,
        event: InterfaceEvent,
    ) -> Result<Option<ConsumeEventOutcome<'_>>, WorkerError> {
        match event {
            InterfaceEvent::Reserved {
                id,
                properties: InterfaceProperties { name, port_class, port_identity_koid },
            } => {
                let enabled = IpEnabledState::default();
                let has_default_ipv4_route = false;
                let has_default_ipv6_route = false;
                let publish_to_fidl = false;
                match state.insert(
                    id,
                    InterfaceState {
                        properties: InterfaceProperties { name, port_class, port_identity_koid },
                        enabled,
                        addresses: HashMap::new(),
                        has_default_ipv4_route,
                        has_default_ipv6_route,
                        publish_to_fidl,
                    },
                ) {
                    Some(old) => Err(WorkerError::AddedDuplicateInterface { interface: id, old }),
                    None => Ok(None),
                }
            }
            InterfaceEvent::Added { id } => match state.get_mut(&id) {
                None => Err(WorkerError::PublishedNonexistentInterface(id)),
                Some(state) => {
                    let &mut InterfaceState {
                        ref properties,
                        ref enabled,
                        ref addresses,
                        ref has_default_ipv4_route,
                        ref has_default_ipv6_route,
                        ref mut publish_to_fidl,
                    } = state;
                    let InterfaceProperties { name, port_class, port_identity_koid } = properties;
                    if std::mem::replace(publish_to_fidl, true) {
                        return Err(WorkerError::PublishedDuplicateInterface(id));
                    }
                    let name = name.clone();
                    let port_class = port_class.clone();
                    let has_default_ipv4_route = *has_default_ipv4_route;
                    let has_default_ipv6_route = *has_default_ipv6_route;
                    let port_identity_koid = *port_identity_koid;
                    let online = enabled.online();
                    Ok(Some(ConsumeEventOutcome {
                        event: finterfaces::Event::Added(
                            finterfaces_ext::Properties::<finterfaces_ext::AllInterest> {
                                id,
                                name,
                                port_class,
                                online,
                                addresses: Self::collect_addresses(addresses),
                                has_default_ipv4_route,
                                has_default_ipv6_route,
                                port_identity_koid,
                            }
                            .into(),
                        ),
                        changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
                        properties: Cow::Borrowed(properties),
                    }))
                }
            },
            InterfaceEvent::Removed(rm) => match state.remove(&rm) {
                Some(InterfaceState { publish_to_fidl, properties, .. }) => Ok(publish_to_fidl
                    .then(|| ConsumeEventOutcome {
                        event: finterfaces::Event::Removed(rm.get()),
                        changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
                        properties: Cow::Owned(properties),
                    })),
                None => Err(WorkerError::RemoveNonexistentInterface(rm)),
            },
            InterfaceEvent::Changed { id, event } => {
                let InterfaceState {
                    properties,
                    enabled,
                    addresses,
                    has_default_ipv4_route,
                    has_default_ipv6_route,
                    publish_to_fidl,
                } = state.get_mut(&id).ok_or(WorkerError::UpdateNonexistentInterface(id))?;
                let publish_to_fidl = *publish_to_fidl;
                match event {
                    InterfaceUpdate::AddressAdded {
                        addr,
                        assignment_state,
                        valid_until,
                        preferred_lifetime,
                    } => {
                        let (addr, prefix_len) = addr.addr_prefix();
                        let addr = *addr;
                        match addresses.insert(
                            addr,
                            AddressProperties {
                                prefix_len,
                                state: AddressState {
                                    assignment_state,
                                    valid_until,
                                    preferred_lifetime,
                                },
                            },
                        ) {
                            Some(AddressProperties { .. }) => {
                                Err(WorkerError::AssignExistingAddr { interface: id, addr })
                            }
                            None => Ok(publish_to_fidl.then(|| ConsumeEventOutcome {
                                event: finterfaces::Event::Changed(finterfaces::Properties {
                                    id: Some(id.get()),
                                    addresses: Some(Self::collect_addresses(addresses)),
                                    ..Default::default()
                                }),
                                changed_address_properties:
                                    ChangedAddressProperties::AssignmentStateChanged {
                                        involves_assigned: is_assigned(assignment_state),
                                    },
                                properties: Cow::Borrowed(properties),
                            })),
                        }
                    }
                    InterfaceUpdate::AddressAssignmentStateChanged { addr, new_state } => {
                        let AddressProperties { prefix_len: _, state } =
                            addresses.get_mut(&addr).ok_or({
                                WorkerError::UpdateStateOnNonexistentAddr {
                                    interface: id,
                                    addr,
                                    state: new_state,
                                }
                            })?;
                        let AddressState {
                            assignment_state,
                            valid_until: _,
                            preferred_lifetime: _,
                        } = state;

                        if *assignment_state == new_state {
                            return Ok(None);
                        }

                        let involves_assigned =
                            is_assigned(*assignment_state) || is_assigned(new_state);

                        *assignment_state = new_state;

                        Ok(publish_to_fidl.then(|| ConsumeEventOutcome {
                            event: finterfaces::Event::Changed(finterfaces::Properties {
                                id: Some(id.get()),
                                addresses: Some(Self::collect_addresses(addresses)),
                                ..Default::default()
                            }),
                            changed_address_properties:
                                ChangedAddressProperties::AssignmentStateChanged {
                                    involves_assigned,
                                },
                            properties: Cow::Borrowed(properties),
                        }))
                    }
                    InterfaceUpdate::AddressRemoved(addr) => match addresses.remove(&addr) {
                        Some(AddressProperties { prefix_len: _, state }) => {
                            let AddressState {
                                assignment_state,
                                valid_until: _,
                                preferred_lifetime: _,
                            } = state;
                            Ok(publish_to_fidl.then(|| ConsumeEventOutcome {
                                event: finterfaces::Event::Changed(finterfaces::Properties {
                                    id: Some(id.get()),
                                    addresses: (Some(Self::collect_addresses(addresses))),
                                    ..Default::default()
                                }),
                                changed_address_properties:
                                    ChangedAddressProperties::AssignmentStateChanged {
                                        involves_assigned: is_assigned(assignment_state),
                                    },
                                properties: Cow::Borrowed(properties),
                            }))
                        }
                        None => Err(WorkerError::UnassignNonexistentAddr { interface: id, addr }),
                    },
                    InterfaceUpdate::DefaultRouteChanged {
                        version,
                        has_default_route: new_value,
                    } => {
                        let mut table =
                            finterfaces::Properties { id: Some(id.get()), ..Default::default() };
                        let (state, prop) = match version {
                            IpVersion::V4 => {
                                (has_default_ipv4_route, &mut table.has_default_ipv4_route)
                            }
                            IpVersion::V6 => {
                                (has_default_ipv6_route, &mut table.has_default_ipv6_route)
                            }
                        };
                        Ok((*state != new_value)
                            .then(|| {
                                *state = new_value;
                                *prop = Some(new_value);
                            })
                            .and_then(|()| {
                                publish_to_fidl.then_some(ConsumeEventOutcome {
                                    event: finterfaces::Event::Changed(table),
                                    changed_address_properties:
                                        ChangedAddressProperties::InterestNotApplicable,
                                    properties: Cow::Borrowed(properties),
                                })
                            }))
                    }
                    InterfaceUpdate::IpEnabledChanged { version, enabled: new_enabled } => {
                        Ok(enabled.update(version, new_enabled).and_then(|new_online| {
                            publish_to_fidl.then(|| ConsumeEventOutcome {
                                event: finterfaces::Event::Changed(finterfaces::Properties {
                                    id: Some(id.get()),
                                    online: Some(new_online),
                                    ..Default::default()
                                }),
                                changed_address_properties:
                                    ChangedAddressProperties::InterestNotApplicable,
                                properties: Cow::Borrowed(properties),
                            })
                        }))
                    }
                    InterfaceUpdate::AddressPropertiesChanged {
                        addr,
                        update:
                            update @ AddressPropertiesUpdate {
                                valid_until: new_valid_until,
                                preferred_lifetime: new_preferred_lifetime,
                            },
                    } => {
                        let AddressState { assignment_state, valid_until, preferred_lifetime } =
                            match addresses.get_mut(&addr) {
                                Some(AddressProperties { prefix_len: _, state }) => state,
                                None => {
                                    return Err(WorkerError::UpdatePropertiesOnNonexistentAddr {
                                        interface: id,
                                        addr,
                                        update,
                                    });
                                }
                            };

                        let mut changed_properties =
                            finterfaces::AddressPropertiesInterest::empty();
                        if new_valid_until != std::mem::replace(valid_until, new_valid_until) {
                            changed_properties |=
                                finterfaces::AddressPropertiesInterest::VALID_UNTIL;
                        }
                        if new_preferred_lifetime
                            != std::mem::replace(preferred_lifetime, new_preferred_lifetime)
                        {
                            changed_properties |=
                                finterfaces::AddressPropertiesInterest::PREFERRED_LIFETIME_INFO;
                        }
                        if changed_properties.is_empty() {
                            return Ok(None);
                        }

                        let is_assigned = is_assigned(*assignment_state);

                        Ok(publish_to_fidl.then(|| ConsumeEventOutcome {
                            event: finterfaces::Event::Changed(finterfaces::Properties {
                                id: Some(id.get()),
                                addresses: Some(Self::collect_addresses(addresses)),
                                ..Default::default()
                            }),
                            changed_address_properties:
                                ChangedAddressProperties::PropertiesChanged {
                                    address_properties: changed_properties,
                                    is_assigned,
                                },
                            properties: Cow::Borrowed(properties),
                        }))
                    }
                }
            }
        }
    }

    fn collect_addresses<T: SortableInterfaceAddress>(
        addrs: &HashMap<IpAddr, AddressProperties>,
    ) -> Vec<T> {
        let mut addrs = addrs
            .iter()
            .map(
                |(
                    addr,
                    AddressProperties {
                        prefix_len,
                        state: AddressState { valid_until, preferred_lifetime, assignment_state },
                    },
                )| {
                    finterfaces_ext::Address {
                        addr: fnet::Subnet { addr: addr.into_fidl(), prefix_len: *prefix_len },
                        valid_until: valid_until.clone().try_into().expect("invalid valid_until"),
                        assignment_state: assignment_state.into_fidl(),
                        preferred_lifetime_info: preferred_lifetime
                            .clone()
                            .try_into_fidl()
                            .expect("invalid preferred lifetime"),
                    }
                    .into()
                },
            )
            .collect::<Vec<T>>();
        // Provide a stably ordered vector of addresses.
        addrs.sort_by_key(|addr| addr.get_sort_key());
        addrs
    }
}

/// The value returned by [`Worker::consume_event`] when an event is to be
/// yielded by watchers.
#[cfg_attr(test, derive(Debug, PartialEq, Clone))]
struct ConsumeEventOutcome<'a> {
    /// The FIDL event to send to watchers.
    event: finterfaces::Event,
    /// Information about changed address properties, for filtering.
    changed_address_properties: ChangedAddressProperties,
    /// The interface properties for which the event was generated.
    properties: Cow<'a, InterfaceProperties>,
}

/// This trait enables the implementation of [`Worker::collect_addresses`] to be
/// agnostic to extension and pure FIDL types, it is not meant to be used in
/// other contexts.
trait SortableInterfaceAddress: From<finterfaces_ext::Address<finterfaces_ext::AllInterest>> {
    type Key: Ord;
    fn get_sort_key(&self) -> Self::Key;
}

impl SortableInterfaceAddress for finterfaces_ext::Address<finterfaces_ext::AllInterest> {
    type Key = fnet::Subnet;
    fn get_sort_key(&self) -> fnet::Subnet {
        self.addr.clone()
    }
}

impl SortableInterfaceAddress for finterfaces::Address {
    type Key = Option<fnet::Subnet>;
    fn get_sort_key(&self) -> Option<fnet::Subnet> {
        self.addr.clone()
    }
}

struct NewWatcher {
    watcher: finterfaces::WatcherRequestStream,
    options: WatcherOptions,
}

#[derive(Error, Debug)]
#[error("connection to interfaces worker closed")]
pub(crate) struct WorkerClosedError {}

#[derive(Clone)]
pub(crate) struct WorkerWatcherSink {
    sender: mpsc::Sender<NewWatcher>,
}

impl WorkerWatcherSink {
    /// Adds a new interface watcher to be operated on by [`Worker`].
    pub(crate) async fn add_watcher(
        &mut self,
        watcher: finterfaces::WatcherRequestStream,
        options: WatcherOptions,
    ) -> Result<(), WorkerClosedError> {
        self.sender
            .send(NewWatcher { watcher, options })
            .await
            .map_err(|_: mpsc::SendError| WorkerClosedError {})
    }
}

#[derive(Clone)]
pub(crate) struct WorkerInterfaceSink {
    sender: mpsc::UnboundedSender<InterfaceEvent>,
}

impl WorkerInterfaceSink {
    /// Reserves a new interface `id` with fixed properties `properties`.
    ///
    /// Added interfaces are always assumed to be offline and have no assigned
    /// address or default routes.
    ///
    /// The returned [`InterfaceEventProducer`] can be used to feed interface
    /// changes to the interface watcher server to build up-to-date interface
    /// state for the reserved id. However those events are not published to
    /// FIDL watchers until [`InterfaceEvent::Added`] is subsequently sent
    /// on the producer. On drop, `InterfaceEventProducer` notifies the
    /// [`Worker`] that the interface was removed.
    ///
    /// Note that the [`Worker`] will exit with an error if two interfaces with
    /// the same identifier are created at the same time, but that is not
    /// observable from `reserve_interface`. It does not provide guardrails to
    /// prevent identifier reuse, however.
    pub(crate) fn reserve_interface(
        &self,
        id: BindingId,
        properties: InterfaceProperties,
    ) -> Result<InterfaceEventProducer, WorkerClosedError> {
        self.sender
            .unbounded_send(InterfaceEvent::Reserved { id, properties })
            .map_err(|_: mpsc::TrySendError<_>| WorkerClosedError {})?;
        Ok(InterfaceEventProducer { id, channel: self.sender.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::util::{IntoCore as _, TryIntoCore as _};
    use assert_matches::assert_matches;
    use fixture::fixture;
    use futures::Stream;
    use itertools::Itertools as _;
    use net_types::Witness as _;
    use net_types::ip::{AddrSubnet, Ip as _, IpAddress as _, Ipv4, Ipv6, Ipv6Addr};
    use std::convert::{TryFrom as _, TryInto as _};
    use std::num::NonZeroU64;
    use std::pin::pin;
    use test_case::test_case;

    impl WorkerWatcherSink {
        fn create_watcher(&mut self) -> finterfaces::WatcherProxy {
            let (watcher, stream) =
                fidl::endpoints::create_proxy_and_stream::<finterfaces::WatcherMarker>();
            self.add_watcher(stream, WatcherOptions::default())
                .now_or_never()
                .expect("unexpected backpressure on sink")
                .expect("failed to send watcher to worker");
            watcher
        }

        fn create_watcher_event_stream(
            &mut self,
        ) -> impl Stream<Item = finterfaces::Event> + Unpin + use<> {
            futures::stream::unfold(self.create_watcher(), |watcher| {
                watcher.watch().map(move |e| match e {
                    Ok(event) => Some((event, watcher)),
                    Err(e) => {
                        if e.is_closed() {
                            None
                        } else {
                            panic!("error fetching next event on watcher {:?}", e);
                        }
                    }
                })
            })
        }
    }

    async fn with_worker<
        Fut: Future<Output = ()>,
        F: FnOnce(WorkerWatcherSink, WorkerInterfaceSink) -> Fut,
    >(
        _name: &str,
        f: F,
    ) {
        let (worker, watcher_sink, interface_sink) = Worker::new();
        let (r, ()) = futures::future::join(worker.run(), f(watcher_sink, interface_sink)).await;
        let watchers = r.expect("worker failed");
        watchers.try_collect::<()>().await.expect("watchers error");
    }

    const IFACE1_ID: BindingId = NonZeroU64::new(111).unwrap();
    const IFACE1_NAME: &str = "iface1";
    const IFACE1_TYPE: finterfaces_ext::PortClass = finterfaces_ext::PortClass::Ethernet;
    const IFACE1_PORT_IDENTITY_KOID: Option<zx::Koid> = Some(zx::Koid::from_raw(1000));

    const IFACE2_ID: BindingId = NonZeroU64::new(222).unwrap();
    const IFACE2_NAME: &str = "iface2";
    const IFACE2_TYPE: finterfaces_ext::PortClass = finterfaces_ext::PortClass::Loopback;
    const IFACE2_PORT_IDENTITY_KOID: Option<zx::Koid> = None;

    fn iface1_properties() -> InterfaceProperties {
        InterfaceProperties {
            name: IFACE1_NAME.to_string(),
            port_class: IFACE1_TYPE,
            port_identity_koid: IFACE1_PORT_IDENTITY_KOID,
        }
    }

    fn iface2_properties() -> InterfaceProperties {
        InterfaceProperties {
            name: IFACE2_NAME.to_string(),
            port_class: IFACE2_TYPE,
            port_identity_koid: IFACE2_PORT_IDENTITY_KOID,
        }
    }

    /// Tests full integration between [`Worker`] and [`Watcher`]s through basic
    /// state updates.
    #[fixture(with_worker)]
    #[fuchsia::test(logging = false)]
    async fn basic_state_updates(
        mut watcher_sink: WorkerWatcherSink,
        interface_sink: WorkerInterfaceSink,
    ) {
        let mut watcher = watcher_sink.create_watcher_event_stream();
        assert_eq!(watcher.next().await, Some(finterfaces::Event::Idle(finterfaces::Empty {})));

        let producer = interface_sink
            .reserve_interface(IFACE1_ID, iface1_properties())
            .expect("add interface");
        producer.start_publishing().expect("start publishing");

        assert_eq!(
            watcher.next().await,
            Some(finterfaces::Event::Added(
                finterfaces_ext::Properties::<finterfaces_ext::DefaultInterest> {
                    id: IFACE1_ID,
                    addresses: Vec::new(),
                    online: false,
                    port_class: IFACE1_TYPE,
                    has_default_ipv4_route: false,
                    has_default_ipv6_route: false,
                    name: IFACE1_NAME.to_string(),
                    port_identity_koid: IFACE1_PORT_IDENTITY_KOID,
                }
                .into()
            ))
        );

        let addr1 = AddrSubnetEither::V6(
            AddrSubnet::new(*Ipv6::LOOPBACK_IPV6_ADDRESS, Ipv6Addr::BYTES * 8).unwrap(),
        );
        const ADDR_VALID_UNTIL: zx::MonotonicInstant = zx::MonotonicInstant::from_nanos(12345);
        const ADDR_PREFERRED_LIFETIME: PreferredLifetime<zx::MonotonicInstant> =
            PreferredLifetime::preferred_until(zx::MonotonicInstant::from_nanos(99999));
        let base_properties =
            finterfaces::Properties { id: Some(IFACE1_ID.get()), ..Default::default() };

        for (event, expect) in [
            (
                InterfaceUpdate::AddressAdded {
                    addr: addr1.clone(),
                    assignment_state: IpAddressState::Assigned,
                    valid_until: ADDR_VALID_UNTIL,
                    preferred_lifetime: ADDR_PREFERRED_LIFETIME,
                },
                finterfaces::Event::Changed(finterfaces::Properties {
                    addresses: Some(vec![
                        finterfaces_ext::Address::<finterfaces_ext::DefaultInterest> {
                            addr: addr1.clone().into_fidl(),
                            assignment_state: finterfaces::AddressAssignmentState::Assigned,
                            valid_until: finterfaces_ext::NoInterest,
                            preferred_lifetime_info: finterfaces_ext::NoInterest,
                        }
                        .into(),
                    ]),
                    ..base_properties.clone()
                }),
            ),
            (
                InterfaceUpdate::DefaultRouteChanged {
                    version: IpVersion::V4,
                    has_default_route: true,
                },
                finterfaces::Event::Changed(finterfaces::Properties {
                    has_default_ipv4_route: Some(true),
                    ..base_properties.clone()
                }),
            ),
            (
                InterfaceUpdate::DefaultRouteChanged {
                    version: IpVersion::V6,
                    has_default_route: true,
                },
                finterfaces::Event::Changed(finterfaces::Properties {
                    has_default_ipv6_route: Some(true),
                    ..base_properties.clone()
                }),
            ),
            (
                InterfaceUpdate::DefaultRouteChanged {
                    version: IpVersion::V6,
                    has_default_route: false,
                },
                finterfaces::Event::Changed(finterfaces::Properties {
                    has_default_ipv6_route: Some(false),
                    ..base_properties.clone()
                }),
            ),
            (
                InterfaceUpdate::IpEnabledChanged { enabled: true, version: IpVersion::V4 },
                finterfaces::Event::Changed(finterfaces::Properties {
                    online: Some(true),
                    ..base_properties.clone()
                }),
            ),
        ] {
            producer.notify(event.clone()).expect("notify event");
            assert_eq!(watcher.next().await, Some(expect), "event: {event:?}");
        }

        // Install a new watcher and observe accumulated interface state.
        let mut new_watcher = watcher_sink.create_watcher_event_stream();
        assert_eq!(
            new_watcher.next().await,
            Some(finterfaces::Event::Existing(
                finterfaces_ext::Properties::<finterfaces_ext::DefaultInterest> {
                    id: IFACE1_ID,
                    name: IFACE1_NAME.to_string(),
                    port_class: IFACE1_TYPE,
                    online: true,
                    addresses: vec![finterfaces_ext::Address {
                        addr: addr1.into_fidl(),
                        assignment_state: finterfaces::AddressAssignmentState::Assigned,
                        valid_until: finterfaces_ext::NoInterest,
                        preferred_lifetime_info: finterfaces_ext::NoInterest,
                    }],
                    has_default_ipv4_route: true,
                    has_default_ipv6_route: false,
                    port_identity_koid: IFACE1_PORT_IDENTITY_KOID,
                }
                .into()
            ))
        );
        assert_eq!(new_watcher.next().await, Some(finterfaces::Event::Idle(finterfaces::Empty {})));
    }

    /// Tests [`Drop`] implementation for [`InterfaceEventProducer`].
    #[fixture(with_worker)]
    #[fuchsia::test(logging = false)]
    async fn drop_producer_removes_interface(
        mut watcher_sink: WorkerWatcherSink,
        interface_sink: WorkerInterfaceSink,
    ) {
        let mut watcher = watcher_sink.create_watcher_event_stream();
        assert_eq!(watcher.next().await, Some(finterfaces::Event::Idle(finterfaces::Empty {})));
        let producer1 = interface_sink
            .reserve_interface(IFACE1_ID, iface1_properties())
            .expect(" add interface");
        producer1.start_publishing().expect("start publishing");

        let producer2 = interface_sink
            .reserve_interface(IFACE2_ID, iface2_properties())
            .expect("add interface");
        producer2.start_publishing().expect("start publishing");
        assert_matches!(
            watcher.next().await,
            Some(finterfaces::Event::Added(finterfaces::Properties {
                id: Some(id),
                ..
            })) if id == IFACE1_ID.get()
        );
        assert_matches!(
            watcher.next().await,
            Some(finterfaces::Event::Added(finterfaces::Properties {
                id: Some(id),
                ..
            })) if id == IFACE2_ID.get()
        );
        std::mem::drop(producer1);
        assert_eq!(watcher.next().await, Some(finterfaces::Event::Removed(IFACE1_ID.get())));

        // Create new watcher and enumerate, only interface 2 should be
        // around now.
        let mut new_watcher = watcher_sink.create_watcher_event_stream();
        assert_matches!(
            new_watcher.next().await,
            Some(finterfaces::Event::Existing(finterfaces::Properties {
                id: Some(id),
                ..
            })) if id == IFACE2_ID.get()
        );
        assert_eq!(new_watcher.next().await, Some(finterfaces::Event::Idle(finterfaces::Empty {})));
    }

    fn iface1_initial_state() -> (BindingId, InterfaceState) {
        (
            IFACE1_ID,
            InterfaceState {
                properties: iface1_properties(),
                enabled: IpEnabledState { v4: false, v6: false },
                addresses: Default::default(),
                has_default_ipv4_route: false,
                has_default_ipv6_route: false,
                publish_to_fidl: true,
            },
        )
    }

    #[test]
    fn consume_interface_added_without_reserve() {
        let mut state = HashMap::new();

        assert_eq!(
            Worker::consume_event(&mut state, InterfaceEvent::Added { id: IFACE1_ID },),
            Err(WorkerError::PublishedNonexistentInterface(IFACE1_ID))
        );
    }

    #[test]
    fn consume_interface_added_twice() {
        let mut state = HashMap::new();

        let (id, initial_state) = iface1_initial_state();

        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Reserved { id, properties: initial_state.properties.clone() }
            ),
            Ok(None)
        );

        assert_eq!(
            Worker::consume_event(&mut state, InterfaceEvent::Added { id }),
            Ok(Some(ConsumeEventOutcome {
                event: finterfaces::Event::Added(
                    finterfaces_ext::Properties::<finterfaces_ext::AllInterest> {
                        id,
                        name: initial_state.properties.name.clone(),
                        port_class: initial_state.properties.port_class.clone(),
                        online: false,
                        addresses: Default::default(),
                        has_default_ipv4_route: false,
                        has_default_ipv6_route: false,
                        port_identity_koid: initial_state.properties.port_identity_koid,
                    }
                    .into()
                ),
                changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
                properties: Cow::Borrowed(&initial_state.properties),
            }))
        );

        assert_eq!(
            Worker::consume_event(&mut state, InterfaceEvent::Added { id }),
            Err(WorkerError::PublishedDuplicateInterface(IFACE1_ID))
        );
    }

    #[test]
    fn consume_interface_reserved_consolidate_after_added() {
        let mut state = HashMap::new();
        let (id, initial_state) = iface1_initial_state();

        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Reserved { id, properties: initial_state.properties.clone() }
            ),
            Ok(None)
        );
        assert_eq!(
            state.get(&id),
            Some(&InterfaceState { publish_to_fidl: false, ..initial_state.clone() })
        );

        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::IpEnabledChanged {
                        enabled: true,
                        version: IpVersion::V4
                    }
                }
            ),
            Ok(None)
        );

        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::DefaultRouteChanged {
                        version: IpVersion::V6,
                        has_default_route: true
                    },
                }
            ),
            Ok(None)
        );

        let addr = AddrSubnetEither::V6(
            AddrSubnet::new(*Ipv6::LOOPBACK_IPV6_ADDRESS, Ipv6Addr::BYTES * 8).unwrap(),
        );
        let valid_until = finterfaces_ext::PositiveMonotonicInstant::from_nanos(1234).unwrap();
        let preferred_lifetime_info = finterfaces_ext::PreferredLifetimeInfo::PreferredUntil(
            finterfaces_ext::PositiveMonotonicInstant::from_nanos(999).unwrap(),
        );
        let assignment_state = IpAddressState::Assigned;

        // Add address.
        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::AddressAdded {
                        addr: addr.clone(),
                        assignment_state,
                        valid_until: valid_until.into(),
                        preferred_lifetime: preferred_lifetime_info.into_core(),
                    },
                }
            ),
            Ok(None)
        );

        assert_eq!(
            Worker::consume_event(&mut state, InterfaceEvent::Added { id }),
            Ok(Some(ConsumeEventOutcome {
                event: finterfaces::Event::Added(
                    finterfaces_ext::Properties::<finterfaces_ext::AllInterest> {
                        id,
                        name: initial_state.properties.name.clone(),
                        port_class: initial_state.properties.port_class.clone(),
                        online: true,
                        addresses: vec![finterfaces_ext::Address::<finterfaces_ext::AllInterest> {
                            addr: addr.clone().into_fidl(),
                            valid_until,
                            assignment_state: assignment_state.into_fidl(),
                            preferred_lifetime_info,
                        }],
                        has_default_ipv4_route: false,
                        has_default_ipv6_route: true,
                        port_identity_koid: initial_state.properties.port_identity_koid,
                    }
                    .into()
                ),
                changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
                properties: Cow::Borrowed(&initial_state.properties),
            }))
        );
    }

    #[test]
    fn consume_interface_removed_after_reserved() {
        let mut state = HashMap::new();
        let (id, initial_state) = iface1_initial_state();

        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Reserved { id, properties: initial_state.properties }
            ),
            Ok(None)
        );

        assert!(state.contains_key(&id));

        // Remove interface but it should not publish the event over fidl.
        assert_eq!(Worker::consume_event(&mut state, InterfaceEvent::Removed(id)), Ok(None));

        assert!(!state.contains_key(&id));
    }

    #[test]
    fn consume_interface_added() {
        let mut state = HashMap::new();
        let (id, initial_state) = iface1_initial_state();

        let event = InterfaceEvent::Reserved { id, properties: initial_state.properties.clone() };

        // Add interface.
        assert_eq!(Worker::consume_event(&mut state, event.clone()), Ok(None));

        assert_eq!(
            Worker::consume_event(&mut state, InterfaceEvent::Added { id }),
            Ok(Some(ConsumeEventOutcome {
                event: finterfaces::Event::Added(
                    finterfaces_ext::Properties::<finterfaces_ext::AllInterest> {
                        id,
                        name: initial_state.properties.name.clone(),
                        port_class: initial_state.properties.port_class.clone(),
                        online: false,
                        addresses: Vec::new(),
                        has_default_ipv4_route: false,
                        has_default_ipv6_route: false,
                        port_identity_koid: initial_state.properties.port_identity_koid,
                    }
                    .into()
                ),
                changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
                properties: Cow::Borrowed(&initial_state.properties),
            }))
        );

        // Verify state has been updated.
        assert_eq!(state.get(&id), Some(&initial_state));

        // Adding again causes error.
        assert_eq!(
            Worker::consume_event(&mut state, event),
            Err(WorkerError::AddedDuplicateInterface { interface: id, old: initial_state })
        );
    }

    #[test]
    fn consume_interface_removed() {
        let (id, initial_state) = iface1_initial_state();

        let mut state = HashMap::from([(id, initial_state.clone())]);

        // Remove interface.
        assert_eq!(
            Worker::consume_event(&mut state, InterfaceEvent::Removed(id)),
            Ok(Some(ConsumeEventOutcome {
                event: finterfaces::Event::Removed(id.get()),
                changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
                properties: Cow::Borrowed(&initial_state.properties),
            }))
        );
        // State is updated.
        assert_eq!(state.get(&id), None);
        // Can't remove again.
        assert_eq!(
            Worker::consume_event(&mut state, InterfaceEvent::Removed(id)),
            Err(WorkerError::RemoveNonexistentInterface(id))
        );
    }

    #[test]
    fn consume_changed_bad_id() {
        let mut state = HashMap::new();
        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id: IFACE1_ID,
                    event: InterfaceUpdate::IpEnabledChanged {
                        enabled: true,
                        version: IpVersion::V4
                    }
                }
            ),
            Err(WorkerError::UpdateNonexistentInterface(IFACE1_ID))
        );
    }

    #[test_case(IpAddressState::Assigned, true; "assigned")]
    #[test_case(IpAddressState::Unavailable, false; "unavailable")]
    #[test_case(IpAddressState::Tentative, false; "tentative")]
    fn consume_changed_address_add_and_removed(
        assignment_state: IpAddressState,
        involves_assigned: bool,
    ) {
        let addr = AddrSubnetEither::V6(
            AddrSubnet::new(*Ipv6::LOOPBACK_IPV6_ADDRESS, Ipv6Addr::BYTES * 8).unwrap(),
        );
        let valid_until = finterfaces_ext::PositiveMonotonicInstant::from_nanos(1234).unwrap();
        let preferred_lifetime_info = finterfaces_ext::PreferredLifetimeInfo::PreferredUntil(
            finterfaces_ext::PositiveMonotonicInstant::from_nanos(999).unwrap(),
        );
        let (id, initial_state) = iface1_initial_state();

        let mut state = HashMap::from([(id, initial_state.clone())]);

        let (ip_addr, prefix_len) = addr.addr_prefix();

        // Add address.
        {
            let event = InterfaceEvent::Changed {
                id,
                event: InterfaceUpdate::AddressAdded {
                    addr: addr.clone(),
                    assignment_state,
                    valid_until: valid_until.into(),
                    preferred_lifetime: preferred_lifetime_info.into_core(),
                },
            };
            assert_eq!(
                Worker::consume_event(&mut state, event.clone()),
                Ok(Some(ConsumeEventOutcome {
                    event: finterfaces::Event::Changed(finterfaces::Properties {
                        id: Some(id.get()),
                        addresses: Some(vec![
                            finterfaces_ext::Address::<finterfaces_ext::AllInterest> {
                                addr: addr.clone().into_fidl(),
                                valid_until,
                                assignment_state: assignment_state.into_fidl(),
                                preferred_lifetime_info,
                            }
                            .into()
                        ]),
                        ..Default::default()
                    }),
                    changed_address_properties: ChangedAddressProperties::AssignmentStateChanged {
                        involves_assigned
                    },
                    properties: Cow::Borrowed(&initial_state.properties),
                })),
            );
            // Check state is updated.
            assert_eq!(
                state.get(&id).expect("missing interface entry").addresses.get(&*ip_addr),
                Some(&AddressProperties {
                    prefix_len,
                    state: AddressState {
                        valid_until: valid_until.into(),
                        assignment_state,
                        preferred_lifetime: preferred_lifetime_info.into_core()
                    }
                })
            );
            // Can't add again.
            assert_eq!(
                Worker::consume_event(&mut state, event),
                Err(WorkerError::AssignExistingAddr { addr: *ip_addr, interface: id })
            );
        }

        // Remove address.
        {
            let event = InterfaceEvent::Changed {
                id,
                event: InterfaceUpdate::AddressRemoved(ip_addr.get()),
            };
            assert_eq!(
                Worker::consume_event(&mut state, event.clone()),
                Ok(Some(ConsumeEventOutcome {
                    event: finterfaces::Event::Changed(finterfaces::Properties {
                        id: Some(id.get()),
                        addresses: Some(Vec::new()),
                        ..Default::default()
                    }),
                    changed_address_properties: ChangedAddressProperties::AssignmentStateChanged {
                        involves_assigned
                    },
                    properties: Cow::Borrowed(&initial_state.properties),
                }))
            );
            // Check state is updated.
            assert_eq!(
                state.get(&id).expect("missing interface entry").addresses.get(&ip_addr),
                None
            );
            // Can't remove again.
            assert_eq!(
                Worker::consume_event(&mut state, event),
                Err(WorkerError::UnassignNonexistentAddr { interface: id, addr: ip_addr.get() })
            );
        }
    }

    #[test]
    fn adding_and_removing_tentative_addresses_do_not_trigger_events() {
        let addr = AddrSubnetEither::V6(
            AddrSubnet::new(*Ipv6::LOOPBACK_IPV6_ADDRESS, Ipv6Addr::BYTES * 8).unwrap(),
        );
        let valid_until = zx::MonotonicInstant::from_nanos(1234);
        let preferred_lifetime =
            PreferredLifetime::preferred_until(zx::MonotonicInstant::from_nanos(1313));
        let (id, initial_state) = iface1_initial_state();

        let mut state = HashMap::from([(id, initial_state.clone())]);

        let event = InterfaceEvent::Changed {
            id,
            event: InterfaceUpdate::AddressAdded {
                addr: addr.clone(),
                assignment_state: IpAddressState::Tentative,
                valid_until,
                preferred_lifetime,
            },
        };

        // Add address, generated event doesn't involve assigned.
        assert_eq!(
            Worker::consume_event(&mut state, event),
            Ok(Some(ConsumeEventOutcome {
                event: finterfaces::Event::Changed(finterfaces::Properties {
                    id: Some(id.get()),
                    addresses: Some(vec![
                        finterfaces_ext::Address::<finterfaces_ext::AllInterest> {
                            addr: addr.into_fidl(),
                            valid_until: valid_until.try_into().unwrap(),
                            assignment_state: finterfaces::AddressAssignmentState::Tentative,
                            preferred_lifetime_info: preferred_lifetime.try_into_fidl().unwrap(),
                        }
                        .into()
                    ]),
                    ..Default::default()
                }),
                changed_address_properties: ChangedAddressProperties::AssignmentStateChanged {
                    involves_assigned: false
                },
                properties: Cow::Borrowed(&initial_state.properties),
            })),
        );
        let (ip_addr, prefix_len) = addr.addr_prefix();
        // Check state is updated.
        assert_eq!(
            state.get(&id).expect("missing interface entry").addresses.get(&*ip_addr),
            Some(&AddressProperties {
                prefix_len,
                state: AddressState {
                    valid_until,
                    assignment_state: IpAddressState::Tentative,
                    preferred_lifetime
                }
            })
        );

        let event =
            InterfaceEvent::Changed { id, event: InterfaceUpdate::AddressRemoved(*ip_addr) };
        // Remove address, generated event doesn't involve assigned.
        assert_eq!(
            Worker::consume_event(&mut state, event),
            Ok(Some(ConsumeEventOutcome {
                event: finterfaces::Event::Changed(finterfaces::Properties {
                    id: Some(id.get()),
                    addresses: Some(Vec::new()),
                    ..Default::default()
                }),
                changed_address_properties: ChangedAddressProperties::AssignmentStateChanged {
                    involves_assigned: false
                },
                properties: Cow::Borrowed(&initial_state.properties),
            })),
        );

        // Check state is updated.
        assert_eq!(state.get(&id).expect("missing interface entry").addresses.get(&*ip_addr), None);
    }

    #[test_case(false; "no_changes_to_unavailable")]
    #[test_case(true; "intersperse_changes_to_unavailable")]
    fn consume_changed_address_state_change(intersperse_unavailable: bool) {
        let subnet = AddrSubnetEither::<net_types::SpecifiedAddr<_>>::V6(
            AddrSubnet::new(*Ipv6::LOOPBACK_IPV6_ADDRESS, Ipv6Addr::BYTES * 8).unwrap(),
        );
        let (addr, prefix_len) = subnet.addr_prefix();
        let addr = *addr;
        let valid_until = zx::MonotonicInstant::from_nanos(1234);
        let preferred_lifetime =
            PreferredLifetime::preferred_until(zx::MonotonicInstant::from_nanos(1313));
        let address_properties = AddressProperties {
            prefix_len,
            state: AddressState {
                valid_until,
                assignment_state: IpAddressState::Tentative,
                preferred_lifetime,
            },
        };
        let (id, initial_state) = iface1_initial_state();
        let initial_state = InterfaceState {
            addresses: HashMap::from([(addr, address_properties)]),
            ..initial_state
        };
        // When `intersperse_unavailable` is `true` a state change to
        // Unavailable is injected between all state changes: e.g.
        // Tentative -> Assigned becomes Tentative -> Unavailable -> Assigned.
        // This allows us to verify that changes to Unavailable do not influence
        // the events observed by the client.
        let maybe_change_state_to_unavailable =
            |state: &mut HashMap<_, InterfaceState>, involves_assigned| {
                if !intersperse_unavailable {
                    return;
                }
                let event = InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::AddressAssignmentStateChanged {
                        addr,
                        new_state: IpAddressState::Unavailable,
                    },
                };
                // Changing state to Unavailable should never generate an event.
                assert_eq!(
                    Worker::consume_event(state, event),
                    Ok(Some(ConsumeEventOutcome {
                        event: finterfaces::Event::Changed(finterfaces::Properties {
                            id: Some(id.get()),
                            addresses: Some(vec![
                                finterfaces_ext::Address::<finterfaces_ext::AllInterest> {
                                    addr: subnet.into_fidl(),
                                    valid_until: valid_until.try_into().unwrap(),
                                    assignment_state:
                                        finterfaces::AddressAssignmentState::Unavailable,
                                    preferred_lifetime_info: preferred_lifetime
                                        .try_into_fidl()
                                        .unwrap(),
                                }
                                .into()
                            ]),
                            ..Default::default()
                        }),
                        changed_address_properties:
                            ChangedAddressProperties::AssignmentStateChanged { involves_assigned },
                        properties: Cow::Borrowed(&initial_state.properties),
                    })),
                );
                // Check state is updated.
                assert_eq!(
                    state.get(&id).expect("missing interface entry").addresses.get(&addr),
                    Some(&AddressProperties {
                        prefix_len,
                        state: AddressState {
                            valid_until,
                            assignment_state: IpAddressState::Unavailable,
                            preferred_lifetime,
                        }
                    }),
                );
            };

        assert_eq!(
            Worker::collect_addresses::<finterfaces::Address>(&initial_state.addresses),
            [finterfaces_ext::Address::<finterfaces_ext::AllInterest> {
                addr: subnet.into_fidl(),
                valid_until: valid_until.try_into().unwrap(),
                assignment_state: finterfaces::AddressAssignmentState::Tentative,
                preferred_lifetime_info: preferred_lifetime.try_into_fidl().unwrap(),
            }
            .into()],
        );

        let mut state = HashMap::from([(id, initial_state.clone())]);

        maybe_change_state_to_unavailable(&mut state, false);

        let event = InterfaceEvent::Changed {
            id,
            event: InterfaceUpdate::AddressAssignmentStateChanged {
                addr,
                new_state: IpAddressState::Assigned,
            },
        };

        // State switch causes event.
        let expected_event = ConsumeEventOutcome {
            event: finterfaces::Event::Changed(finterfaces::Properties {
                id: Some(id.get()),
                addresses: Some(vec![
                    finterfaces_ext::Address::<finterfaces_ext::AllInterest> {
                        addr: subnet.into_fidl(),
                        valid_until: valid_until.try_into().unwrap(),
                        assignment_state: finterfaces::AddressAssignmentState::Assigned,
                        preferred_lifetime_info: preferred_lifetime.try_into_fidl().unwrap(),
                    }
                    .into(),
                ]),
                ..Default::default()
            }),
            changed_address_properties: ChangedAddressProperties::AssignmentStateChanged {
                involves_assigned: true,
            },
            properties: Cow::Borrowed(&initial_state.properties),
        };
        assert_eq!(
            Worker::consume_event(&mut state, event.clone()),
            Ok(Some(expected_event.clone())),
        );

        // Check state is updated.
        assert_eq!(
            state.get(&id).expect("missing interface entry").addresses.get(&addr),
            Some(&AddressProperties {
                prefix_len,
                state: AddressState {
                    valid_until,
                    assignment_state: IpAddressState::Assigned,
                    preferred_lifetime
                },
            })
        );

        maybe_change_state_to_unavailable(&mut state, true);
        assert_eq!(
            Worker::consume_event(&mut state, event),
            Ok(intersperse_unavailable.then_some(expected_event)),
        );

        maybe_change_state_to_unavailable(&mut state, true);

        // Switch the state back to tentative, which will trigger removal.
        let event = InterfaceEvent::Changed {
            id,
            event: InterfaceUpdate::AddressAssignmentStateChanged {
                addr,
                new_state: IpAddressState::Tentative,
            },
        };
        let expected_event = ConsumeEventOutcome {
            event: finterfaces::Event::Changed(finterfaces::Properties {
                id: Some(id.get()),
                addresses: Some(vec![
                    finterfaces_ext::Address::<finterfaces_ext::AllInterest> {
                        addr: subnet.into_fidl(),
                        valid_until: valid_until.try_into().unwrap(),
                        assignment_state: finterfaces::AddressAssignmentState::Tentative,
                        preferred_lifetime_info: preferred_lifetime.try_into_fidl().unwrap(),
                    }
                    .into(),
                ]),
                ..Default::default()
            }),
            changed_address_properties: ChangedAddressProperties::AssignmentStateChanged {
                involves_assigned: !intersperse_unavailable,
            },
            properties: Cow::Borrowed(&initial_state.properties),
        };
        assert_eq!(
            Worker::consume_event(&mut state, event.clone()),
            Ok(Some(expected_event.clone())),
        );

        // Check state is updated.
        assert_eq!(
            state.get(&id).expect("missing interface entry").addresses.get(&addr),
            Some(&AddressProperties {
                prefix_len,
                state: AddressState {
                    valid_until,
                    assignment_state: IpAddressState::Tentative,
                    preferred_lifetime
                },
            })
        );

        maybe_change_state_to_unavailable(&mut state, false);
        assert_eq!(
            Worker::consume_event(&mut state, event),
            Ok(intersperse_unavailable.then_some(expected_event)),
        );

        maybe_change_state_to_unavailable(&mut state, false);
    }

    #[test_case(IpAddressState::Assigned; "assigned")]
    #[test_case(IpAddressState::Unavailable; "unavailable")]
    #[test_case(IpAddressState::Tentative; "tentative")]
    fn consume_changed_start_unavailable(new_state: IpAddressState) {
        let addr = AddrSubnetEither::<net_types::SpecifiedAddr<_>>::V6(
            AddrSubnet::new(*Ipv6::LOOPBACK_IPV6_ADDRESS, Ipv6Addr::BYTES * 8).unwrap(),
        );
        let valid_until = zx::MonotonicInstant::from_nanos(1234);
        let preferred_lifetime =
            PreferredLifetime::preferred_until(zx::MonotonicInstant::from_nanos(111));
        let (ip_addr, prefix_len) = addr.addr_prefix();

        // Set up the initial state.
        let (id, mut initial_state) = iface1_initial_state();
        assert_eq!(
            initial_state.addresses.insert(
                *ip_addr,
                AddressProperties {
                    prefix_len,
                    state: AddressState {
                        valid_until,
                        assignment_state: IpAddressState::Unavailable,
                        preferred_lifetime,
                    }
                }
            ),
            None
        );
        let mut state = HashMap::from([(id, initial_state.clone())]);

        // Send an event to change the state.
        let event = InterfaceEvent::Changed {
            id,
            event: InterfaceUpdate::AddressAssignmentStateChanged { addr: *addr.addr(), new_state },
        };

        let expected_event = match new_state {
            // Changing from `Unavailable` to `Unavailable` is a No-Op.
            IpAddressState::Unavailable => None,
            new_state @ (IpAddressState::Assigned | IpAddressState::Tentative) => {
                Some(ConsumeEventOutcome {
                    event: finterfaces::Event::Changed(finterfaces::Properties {
                        id: Some(id.get()),
                        addresses: Some(vec![
                            finterfaces_ext::Address::<finterfaces_ext::AllInterest> {
                                addr: addr.into_fidl(),
                                valid_until: valid_until.try_into().unwrap(),
                                assignment_state: new_state.into_fidl(),
                                preferred_lifetime_info: preferred_lifetime
                                    .try_into_fidl()
                                    .unwrap(),
                            }
                            .into(),
                        ]),
                        ..Default::default()
                    }),
                    changed_address_properties: ChangedAddressProperties::AssignmentStateChanged {
                        involves_assigned: is_assigned(new_state),
                    },
                    properties: Cow::Borrowed(&initial_state.properties),
                })
            }
        };
        assert_eq!(Worker::consume_event(&mut state, event), Ok(expected_event));
        assert_eq!(
            state.get(&id).expect("missing interface entry").addresses.get(&addr.addr()),
            Some(&AddressProperties {
                prefix_len,
                state: AddressState {
                    valid_until,
                    assignment_state: new_state,
                    preferred_lifetime
                },
            })
        );
    }

    #[test]
    fn consume_changed_address_properties() {
        let addr = AddrSubnetEither::<net_types::SpecifiedAddr<_>>::V6(
            AddrSubnet::new(*Ipv6::LOOPBACK_IPV6_ADDRESS, Ipv6Addr::BYTES * 8).unwrap(),
        );
        let valid_until = zx::MonotonicInstant::from_nanos(1234);
        let preferred_lifetime =
            PreferredLifetime::preferred_until(zx::MonotonicInstant::from_nanos(111));
        let assignment_state = IpAddressState::Assigned;
        let (ip_addr, prefix_len) = addr.addr_prefix();

        // Set up the initial state.
        let (id, mut initial_state) = iface1_initial_state();
        assert_eq!(
            initial_state.addresses.insert(
                *ip_addr,
                AddressProperties {
                    prefix_len,
                    state: AddressState { valid_until, assignment_state, preferred_lifetime }
                }
            ),
            None
        );
        let mut state = HashMap::from([(id, initial_state.clone())]);

        // Event with bad interface generates error.
        let bad_id = BindingId::new(id.get() + 1).unwrap();
        let event = InterfaceEvent::Changed {
            id: bad_id,
            event: InterfaceUpdate::AddressPropertiesChanged {
                addr: *addr.addr(),
                update: AddressPropertiesUpdate { valid_until, preferred_lifetime },
            },
        };
        assert_eq!(
            Worker::consume_event(&mut state, event),
            Err(WorkerError::UpdateNonexistentInterface(bad_id))
        );

        // Event with bad address generates error.
        let bad_addr = Ipv4::LOOPBACK_ADDRESS.get().into();
        let event = InterfaceEvent::Changed {
            id,
            event: InterfaceUpdate::AddressPropertiesChanged {
                addr: bad_addr,
                update: AddressPropertiesUpdate { valid_until, preferred_lifetime },
            },
        };
        assert_eq!(
            Worker::consume_event(&mut state, event),
            Err(WorkerError::UpdatePropertiesOnNonexistentAddr {
                interface: id,
                addr: bad_addr,
                update: AddressPropertiesUpdate { valid_until, preferred_lifetime }
            })
        );

        // The same state generates no events.
        let event = InterfaceEvent::Changed {
            id,
            event: InterfaceUpdate::AddressPropertiesChanged {
                addr: *addr.addr(),
                update: AddressPropertiesUpdate { valid_until, preferred_lifetime },
            },
        };
        assert_eq!(Worker::consume_event(&mut state, event), Ok(None));

        let next_valid_until = zx::MonotonicInstant::from_nanos(valid_until.into_nanos() + 1);
        let next_preferred_lifetime = PreferredLifetime::Deprecated;
        for (valid_until, preferred_lifetime, changed) in [
            // Only valid_until changes.
            (
                next_valid_until,
                preferred_lifetime,
                finterfaces::AddressPropertiesInterest::VALID_UNTIL,
            ),
            // Only preferred_lifetime changes.
            (
                next_valid_until,
                next_preferred_lifetime,
                finterfaces::AddressPropertiesInterest::PREFERRED_LIFETIME_INFO,
            ),
            // valid_until and preferred_lifetime change.
            (
                valid_until,
                preferred_lifetime,
                finterfaces::AddressPropertiesInterest::PREFERRED_LIFETIME_INFO
                    | finterfaces::AddressPropertiesInterest::VALID_UNTIL,
            ),
        ] {
            let event = InterfaceEvent::Changed {
                id,
                event: InterfaceUpdate::AddressPropertiesChanged {
                    addr: *addr.addr(),
                    update: AddressPropertiesUpdate { valid_until, preferred_lifetime },
                },
            };
            let expect_event = finterfaces::Event::Changed(finterfaces::Properties {
                id: Some(id.get()),
                addresses: Some(vec![
                    finterfaces_ext::Address::<finterfaces_ext::AllInterest> {
                        addr: addr.into_fidl(),
                        valid_until: valid_until.try_into().unwrap(),
                        preferred_lifetime_info: preferred_lifetime.try_into_fidl().unwrap(),
                        assignment_state: assignment_state.into_fidl(),
                    }
                    .into(),
                ]),
                ..Default::default()
            });
            let expect_props = ChangedAddressProperties::PropertiesChanged {
                address_properties: changed,
                is_assigned: true,
            };
            assert_eq!(
                Worker::consume_event(&mut state, event),
                Ok(Some(ConsumeEventOutcome {
                    event: expect_event,
                    changed_address_properties: expect_props,
                    properties: Cow::Borrowed(&initial_state.properties),
                }))
            );
        }
    }

    #[test]
    fn consume_changed_online() {
        let (id, initial_state) = iface1_initial_state();
        let mut state = HashMap::from([(id, initial_state.clone())]);

        // Change V4 to online.
        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::IpEnabledChanged {
                        enabled: true,
                        version: IpVersion::V4
                    }
                }
            ),
            Ok(Some(ConsumeEventOutcome {
                event: finterfaces::Event::Changed(finterfaces::Properties {
                    id: Some(id.get()),
                    online: Some(true),
                    ..Default::default()
                }),
                changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
                properties: Cow::Borrowed(&initial_state.properties),
            }))
        );

        let entry = state.get(&id).expect("missing interface entry");
        assert_eq!(entry.enabled, IpEnabledState { v4: true, v6: false });

        // Change V6 to online.
        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::IpEnabledChanged {
                        enabled: true,
                        version: IpVersion::V6
                    }
                }
            ),
            Ok(None)
        );
        // Check state is updated.
        let entry = state.get(&id).expect("missing interface entry");
        assert_eq!(entry.enabled, IpEnabledState { v4: true, v6: true });
        // Change again produces no update.
        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::IpEnabledChanged {
                        enabled: true,
                        version: IpVersion::V4
                    }
                }
            ),
            Ok(None)
        );

        // Change just one side produces no update.
        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::IpEnabledChanged {
                        enabled: false,
                        version: IpVersion::V4
                    }
                }
            ),
            Ok(None)
        );

        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::IpEnabledChanged {
                        enabled: false,
                        version: IpVersion::V6
                    }
                }
            ),
            Ok(Some(ConsumeEventOutcome {
                event: finterfaces::Event::Changed(finterfaces::Properties {
                    id: Some(id.get()),
                    online: Some(false),
                    ..Default::default()
                }),
                changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
                properties: Cow::Borrowed(&initial_state.properties),
            }))
        );
    }

    #[test_case(IpVersion::V4; "ipv4")]
    #[test_case(IpVersion::V6; "ipv6")]
    fn consume_changed_default_route(version: IpVersion) {
        let (id, initial_state) = iface1_initial_state();
        let mut state = HashMap::from([(id, initial_state.clone())]);

        let expect_set_props = match version {
            IpVersion::V4 => {
                finterfaces::Properties { has_default_ipv4_route: Some(true), ..Default::default() }
            }
            IpVersion::V6 => {
                finterfaces::Properties { has_default_ipv6_route: Some(true), ..Default::default() }
            }
        };

        // Update default route.
        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::DefaultRouteChanged {
                        version,
                        has_default_route: true
                    }
                }
            ),
            Ok(Some(ConsumeEventOutcome {
                event: finterfaces::Event::Changed(finterfaces::Properties {
                    id: Some(id.get()),
                    ..expect_set_props
                }),
                changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
                properties: Cow::Borrowed(&initial_state.properties),
            }))
        );
        // Check only the proper state is updated.
        let InterfaceState { has_default_ipv4_route, has_default_ipv6_route, .. } =
            state.get(&id).expect("missing interface entry");
        assert_eq!(*has_default_ipv4_route, version == IpVersion::V4);
        assert_eq!(*has_default_ipv6_route, version == IpVersion::V6);
        // Change again produces no update.
        assert_eq!(
            Worker::consume_event(
                &mut state,
                InterfaceEvent::Changed {
                    id,
                    event: InterfaceUpdate::DefaultRouteChanged {
                        version,
                        has_default_route: true
                    }
                }
            ),
            Ok(None)
        );
    }

    #[fixture(with_worker)]
    #[fuchsia::test(logging = false)]
    async fn watcher_enqueues_events(
        mut watcher_sink: WorkerWatcherSink,
        interface_sink: WorkerInterfaceSink,
    ) {
        let mut create_watcher = || {
            let mut watcher = watcher_sink.create_watcher_event_stream();
            async move {
                assert_eq!(
                    watcher.next().await,
                    Some(finterfaces::Event::Idle(finterfaces::Empty {}))
                );
                watcher
            }
        };

        let watcher1 = create_watcher().await;
        let watcher2 = create_watcher().await;

        let range = 1..=10;
        let producers = watcher1
            .zip(futures::stream::iter(range.clone().map(|i| {
                let i = BindingId::new(i).unwrap();
                let producer = interface_sink
                    .reserve_interface(
                        i,
                        InterfaceProperties { name: format!("if{}", i), ..iface1_properties() },
                    )
                    .expect("failed to add interface");
                producer.start_publishing().expect("start publishing");
                (producer, i)
            })))
            .map(|(event, (producer, i))| {
                assert_matches!(
                    event,
                    finterfaces::Event::Added(finterfaces::Properties {
                        id: Some(id),
                        ..
                    }) if id == i.get()
                );
                producer
            })
            .collect::<Vec<_>>()
            .await;
        assert_eq!(producers.len(), usize::try_from(*range.end()).unwrap());

        let last = watcher2
            .zip(futures::stream::iter(range.clone()))
            .fold(None, |_, (event, i)| {
                assert_matches!(
                    event,
                    finterfaces::Event::Added(finterfaces::Properties {
                        id: Some(id),
                        ..
                    }) if id == i
                );
                futures::future::ready(Some(i))
            })
            .await;
        assert_eq!(last, Some(*range.end()));
    }

    #[fixture(with_worker)]
    #[fuchsia::test(logging = false)]
    async fn idle_watcher_gets_closed(
        mut watcher_sink: WorkerWatcherSink,
        interface_sink: WorkerInterfaceSink,
    ) {
        let watcher = watcher_sink.create_watcher();
        // Get the idle event to make sure the worker sees the watcher.
        assert_matches!(watcher.watch().await, Ok(finterfaces::Event::Idle(finterfaces::Empty {})));

        // NB: Every round generates two events, addition and removal because we
        // drop the producer.
        for i in 1..=(MAX_EVENTS / 2 + 1).try_into().unwrap() {
            let producer = interface_sink
                .reserve_interface(
                    BindingId::new(i).unwrap(),
                    InterfaceProperties { name: format!("if{}", i), ..iface1_properties() },
                )
                .expect("failed to add interface");
            producer.start_publishing().expect("start publishing");
        }
        // Watcher gets closed.
        assert_eq!(watcher.on_closed().await, Ok(zx::Signals::CHANNEL_PEER_CLOSED));
    }

    /// Tests that the worker can handle watchers coming and going.
    #[test]
    fn watcher_turnaround() {
        let mut executor = fuchsia_async::TestExecutor::new_with_fake_time();
        let (worker, mut watcher_sink, interface_sink) = Worker::new();
        let sink_keep = watcher_sink.clone();
        let create_watchers = fuchsia_async::Task::spawn(async move {
            let mut watcher = watcher_sink.create_watcher_event_stream();
            assert_eq!(watcher.next().await, Some(finterfaces::Event::Idle(finterfaces::Empty {})));
        });

        // NB: Map the output of the worker future so we can assert equality on
        // the return, since its return is not Debug otherwise.
        let worker_fut = worker.run().map(|result| {
            let pending_watchers = result.expect("worker finished with error");
            pending_watchers.len()
        });
        let mut worker_fut = pin!(worker_fut);
        assert_eq!(executor.run_until_stalled(&mut worker_fut), std::task::Poll::Pending);
        // If executor stalled then the task must've finished.
        assert_eq!(create_watchers.now_or_never(), Some(()));

        // Drop the sinks, should cause the worker to return.
        std::mem::drop((sink_keep, interface_sink));
        assert_eq!(executor.run_until_stalled(&mut worker_fut), std::task::Poll::Ready(0));
    }

    #[fixture(with_worker)]
    #[fuchsia::test(logging = false)]
    async fn address_sorting(
        mut watcher_sink: WorkerWatcherSink,
        interface_sink: WorkerInterfaceSink,
    ) {
        let mut watcher = watcher_sink.create_watcher_event_stream();
        assert_eq!(watcher.next().await, Some(finterfaces::Event::Idle(finterfaces::Empty {})));

        const ADDR1: fnet::Subnet = net_declare::fidl_subnet!("2000::1/64");
        const ADDR2: fnet::Subnet = net_declare::fidl_subnet!("192.168.1.1/24");
        const ADDR3: fnet::Subnet = net_declare::fidl_subnet!("192.168.1.2/24");

        for addrs in [ADDR1, ADDR2, ADDR3].into_iter().permutations(3) {
            let producer = interface_sink
                .reserve_interface(IFACE1_ID, iface1_properties())
                .expect("failed to add interface");
            producer.start_publishing().expect("start publishing");
            assert_matches!(
                watcher.next().await,
                Some(finterfaces::Event::Added(finterfaces::Properties {
                    id: Some(id), .. }
                )) if id == IFACE1_ID.get()
            );

            let mut expect = vec![];
            for addr in addrs {
                producer
                    .notify(InterfaceUpdate::AddressAdded {
                        addr: addr.try_into_core().expect("invalid address"),
                        assignment_state: IpAddressState::Assigned,
                        valid_until: zx::MonotonicInstant::INFINITE,
                        preferred_lifetime: PreferredLifetime::preferred_forever(),
                    })
                    .expect("failed to notify");
                expect.push(addr);
                expect.sort();

                let addresses = assert_matches!(
                    watcher.next().await,
                    Some(finterfaces::Event::Changed(finterfaces::Properties{
                        id: Some(id),
                        addresses: Some(addresses),
                        ..
                    })) if id == IFACE1_ID.get() => addresses
                );
                let addresses = addresses
                    .into_iter()
                    .map(|finterfaces::Address { addr, .. }| addr.expect("missing address"))
                    .collect::<Vec<_>>();
                assert_eq!(addresses, expect);
            }
            std::mem::drop(producer);
            assert_eq!(watcher.next().await, Some(finterfaces::Event::Removed(IFACE1_ID.get())));
        }
    }

    #[fixture(with_worker)]
    #[fuchsia::test(logging = false)]
    async fn watcher_disallows_double_get(
        mut watcher_sink: WorkerWatcherSink,
        _interface_sink: WorkerInterfaceSink,
    ) {
        let watcher = watcher_sink.create_watcher();
        assert_matches!(watcher.watch().await, Ok(finterfaces::Event::Idle(finterfaces::Empty {})));

        let (r1, r2) = futures::future::join(watcher.watch(), watcher.watch()).await;
        for r in [r1, r2] {
            assert_matches!(
                r,
                Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                    if epitaph == zx::Status::ALREADY_EXISTS
            );
        }
    }

    #[test]
    fn watcher_blocking_push() {
        let mut executor = fuchsia_async::TestExecutor::new_with_fake_time();
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<finterfaces::WatcherMarker>();
        let mut watcher = Watcher {
            stream,
            events: EventQueue { events: Default::default() },
            responder: None,
            options: WatcherOptions::default(),
        };
        let mut watch_fut = proxy.watch();
        assert_matches!(executor.run_until_stalled(&mut watch_fut), std::task::Poll::Pending);
        assert_matches!(executor.run_until_stalled(&mut watcher), std::task::Poll::Pending);
        // Got a responder, we're pending.
        assert_matches!(watcher.responder, Some(_));
        assert!(watcher.push(&ConsumeEventOutcome {
            event: finterfaces::Event::Idle(finterfaces::Empty {}),
            changed_address_properties: ChangedAddressProperties::InterestNotApplicable,
            properties: Cow::Owned(iface1_properties()),
        }));
        // Responder is executed.
        assert_matches!(watcher.responder, None);
        assert_matches!(
            executor.run_until_stalled(&mut watch_fut),
            std::task::Poll::Ready(Ok(finterfaces::Event::Idle(finterfaces::Empty {})))
        );
    }

    #[test]
    fn watcher_options_should_track_interface_by_port_event_id() {
        let options = WatcherOptions { port_identity_koid_filter: None, ..Default::default() };
        let iface1_props = iface1_properties();
        let iface2_props = iface2_properties();
        assert_eq!(options.should_track_interface(&iface1_props), true);
        assert_eq!(options.should_track_interface(&iface2_props), true);

        let options = WatcherOptions {
            port_identity_koid_filter: Some(iface1_props.port_identity_koid.unwrap()),
            ..Default::default()
        };
        assert_eq!(options.should_track_interface(&iface1_props), true);
        assert_eq!(options.should_track_interface(&iface2_props), false);
    }
}
