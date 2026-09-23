// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The generic implementation for FIDL rotue/rule watchers.

use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::Hash;
use std::pin::pin;

use async_utils::event::Event;
use derivative::Derivative;
use fidl::endpoints::ProtocolMarker;
use fidl_fuchsia_net_routes as fnet_routes;
use futures::channel::mpsc;
use futures::future::FusedFuture as _;
use futures::{FutureExt, StreamExt as _, TryStreamExt as _};
use itertools::Itertools;
use log::{debug, warn};
use netstack3_core::sync::Mutex;
use thiserror::Error;

use crate::bindings::util::ErrorLogExt;

#[derive(Debug, Error)]
pub(crate) enum ServeWatcherError {
    #[error("the request stream contained a FIDL error")]
    ErrorInStream(fidl::Error),
    #[error("a FIDL error was encountered while sending the response")]
    FailedToRespond(fidl::Error),
    #[error("the client called `Watch` while a previous call was already pending")]
    PreviousPendingWatch,
    #[error("the client was canceled")]
    Canceled,
}

impl ErrorLogExt for ServeWatcherError {
    fn log_level(&self) -> log::Level {
        match self {
            Self::ErrorInStream(fidl) | Self::FailedToRespond(fidl) => fidl.log_level(),
            Self::PreviousPendingWatch | Self::Canceled => log::Level::Warn,
        }
    }
}

pub(crate) async fn serve_watcher<E: FidlWatcherEvent, WI: WatcherInterest<E>>(
    server_end: fidl::endpoints::ServerEnd<E::WatcherMarker>,
    interest: WI,
    UpdateDispatcher(dispatcher): UpdateDispatcher<E, WI>,
) -> Result<(), ServeWatcherError> {
    let watcher = {
        let mut dispatcher = dispatcher.lock();
        dispatcher.connect_new_client(interest)
    };

    // Always disconnect the client when we finish this future.
    let mut watcher = scopeguard::guard(watcher, |watcher| {
        let mut dispatcher = dispatcher.lock();
        dispatcher.disconnect_client(watcher);
    });

    let request_stream = server_end.into_stream();
    let canceled_fut = watcher.canceled.wait();

    let mut request_stream = request_stream.map_err(ServeWatcherError::ErrorInStream).fuse();
    let mut canceled_fut = pin!(canceled_fut);
    let mut pending_watch_request = futures::future::OptionFuture::default();
    loop {
        pending_watch_request = futures::select! {
            request = request_stream.try_next() => match request? {
                Some(req) => if pending_watch_request.is_terminated() {
                    // Convince the compiler that we're not holding on to a
                    // borrow of watcher.
                    std::mem::drop(pending_watch_request);
                    // Old request is terminated, accept this new one.
                    Some(watcher.watch().map(move |events| (req, events))).into()
                } else {
                    break Err(ServeWatcherError::PreviousPendingWatch);
                },
                None => break Ok(()),
            },
            r = pending_watch_request => {
                let (request, events) = r.expect("OptionFuture is not selected if empty");
                match E::respond_to_watch_request(request, events) {
                    Ok(()) => None.into(),
                    Err(e) => break Err(ServeWatcherError::FailedToRespond(e)),
                }
            },
            () = canceled_fut => break Err(ServeWatcherError::Canceled),
        };
    }
}

/// An update to the routing/rule table.
#[derive(Clone)]
pub(crate) enum Update<E: WatcherEvent> {
    Added(E::Resource),
    Removed(E::Resource),
}

/// An event to be watched.
///
/// For example [`fnet_routes_ext::Event`] for the routes watcher. This is a
/// separate trait from [`FidlWatcherEvent`] so that we can tie this trait
/// bound to logic that is unrelated to FIDL. This gives us the benefit to not
/// mention the fidl IP extension traits everywhere.
pub(crate) trait WatcherEvent: From<Update<Self>> + Debug + Clone {
    /// The installed resource that is being tracked.
    ///
    /// E.g. [`fnet_routes_ext::InstalledRoute`] for [`fnet_routes_ext::Event`].
    type Resource: Hash + PartialEq + Eq + Clone + Debug;

    /// The maximum number of events that can be returned by one watch.
    const MAX_EVENTS: usize;

    /// The idle event.
    const IDLE: Self;

    /// Turn the installed type into an existing event.
    fn existing(installed: Self::Resource) -> Self;
}

const fn max_pending_events<E: WatcherEvent>() -> usize {
    // The maximum number of events for the fidl watcher client is allowed
    // to have queued. Clients will be dropped if they exceed this limit.
    // Keep this a multiple of `fnet_routes::MAX_EVENTS` (5 is somewhat
    // arbitrary) so we don't artificially truncate the allowed batch size.
    E::MAX_EVENTS * 5
}

/// The trait that abstracts the FIDL behavior of a [`WatcherEvent`].
pub(crate) trait FidlWatcherEvent: WatcherEvent {
    /// The protocol marker for the watcher protocol
    type WatcherMarker: ProtocolMarker;

    /// Responds to the FIDL `watch` request.
    fn respond_to_watch_request(
        req: fidl::endpoints::Request<Self::WatcherMarker>,
        events: Vec<Self>,
    ) -> Result<(), fidl::Error>;
}

/// Expresses interest in certain events for each watcher client.
pub(crate) trait WatcherInterest<E>: Debug {
    /// Interested in the given event.
    fn has_interest_in(&self, event: &E) -> bool;
}

// Consumes updates to the system routing table and dispatches them to clients
// of the `fuchsia.net.routes/{Rule}WatcherV{4,6}` protocols.
#[derive(Derivative)]
#[derivative(Clone(bound = ""))]
#[derivative(Default(bound = ""))]
pub(crate) struct UpdateDispatcher<E: WatcherEvent, WI>(
    std::sync::Arc<Mutex<UpdateDispatcherInner<E, WI>>>,
);

// The inner representation of a `UpdateDispatcher` holding state for the
// given IP protocol.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
struct UpdateDispatcherInner<E: WatcherEvent, WI> {
    // The set of currently installed routes/rules.
    installed: HashSet<E::Resource>,
    // The list of currently connected clients.
    clients: Vec<WatcherSink<E, WI>>,
}

// The error type returned by `UpdateDispatcher.notify()`.
#[derive(Debug, PartialEq)]
pub(crate) enum UpdateNotifyError<E: WatcherEvent> {
    // `notify` was called with `TableUpdate::Added` for a route
    // that already exists.
    AlreadyExists(E::Resource),
    // `notify` was called with `TableUpdate::Removed` for a route
    // that does not exist.
    NotFound(E::Resource),
}

impl<E: WatcherEvent, WI: WatcherInterest<E>> UpdateDispatcherInner<E, WI> {
    /// Notifies this `UpdateDispatcher` of an update to the routing table.
    /// The update will be dispatched to all active watcher clients.
    fn notify(&mut self, update: Update<E>) -> Result<(), UpdateNotifyError<E>> {
        let UpdateDispatcherInner { installed, clients } = self;
        let event = E::from(update.clone());
        match update {
            Update::Added(added) => {
                if !installed.insert(added.clone()) {
                    return Err(UpdateNotifyError::AlreadyExists(added));
                }
            }
            Update::Removed(removed) => {
                if !installed.remove(&removed) {
                    return Err(UpdateNotifyError::NotFound(removed));
                }
            }
        };
        for client in clients {
            client.send(event.clone())
        }
        Ok(())
    }

    /// Registers a new client with this `UpdateDispatcher`.
    fn connect_new_client(&mut self, client_interest: WI) -> Watcher<E> {
        let UpdateDispatcherInner { installed: routes, clients } = self;
        let (watcher, sink) = Watcher::new_with_existing(routes.iter().cloned(), client_interest);
        clients.push(sink);
        watcher
    }

    /// Disconnects the given watcher from this `UpdateDispatcher`.
    fn disconnect_client(&mut self, watcher: Watcher<E>) {
        let UpdateDispatcherInner { installed: _, clients } = self;
        let (idx, _): (usize, &WatcherSink<E, WI>) = Itertools::exactly_one(
            clients.iter().enumerate().filter(|(_idx, client)| client.is_connected_to(&watcher)),
        )
        .expect("expected exactly one sink");
        let _: WatcherSink<E, WI> = clients.swap_remove(idx);
    }
}

impl<E: WatcherEvent, WI: WatcherInterest<E>> UpdateDispatcher<E, WI> {
    pub(crate) fn notify(&self, update: Update<E>) -> Result<(), UpdateNotifyError<E>> {
        let Self(inner) = self;
        inner.lock().notify(update)
    }
}

/// Consumes events for a single client of the
/// `fuchsia.net.routes/{Rule}WatcherV{4,6}` protocols.
#[derive(Debug)]
struct WatcherSink<E, WI> {
    // The sink with which to send changes to this client.
    sink: mpsc::Sender<E>,
    // The mechanism with which to cancel the client.
    cancel: Event,
    // What table is this client interested in.
    interest: WI,
}

impl<E: WatcherEvent, WI: WatcherInterest<E>> WatcherSink<E, WI> {
    // Send this [`WatcherSink`] a new event.
    fn send(&mut self, event: E) {
        let interested = self.interest.has_interest_in(&event);

        if !interested {
            debug!(
                "The client for sink {:?} is not interested in this event {:?}, skipping",
                self, event
            );
            return;
        }

        self.sink.try_send(event).unwrap_or_else(|e| {
            if e.is_full() {
                if self.cancel.signal() {
                    warn!(
                        "too many unconsumed events (the client may not be \
                        calling Watch frequently enough): {}",
                        max_pending_events::<E>(),
                    )
                }
            } else {
                panic!("unexpected error trying to send: {:?}", e)
            }
        })
    }

    // Returns `true` if this sink forwards events to the given watcher.
    fn is_connected_to(&self, watcher: &Watcher<E>) -> bool {
        self.sink.is_connected_to(&watcher.receiver)
    }
}

/// An implementation of the `fuchsia.net.routes.{Rule}WatcherV{4,6}` protocols
/// for a single client.
#[derive(Debug)]
pub(crate) struct Watcher<E> {
    /// The `Existing` + `Idle` events for this client, capturing all of the
    /// routes that existed at the time it was instantiated.
    // NB: storing this as an `IntoIter` makes `watch` easier to implement.
    existing_events: std::vec::IntoIter<E>,
    /// The receiver of routing changes for this client.
    receiver: mpsc::Receiver<E>,
    /// The mechanism to observe that this client has been canceled.
    canceled: Event,
}

impl<E: WatcherEvent> Watcher<E> {
    /// Creates a new `Watcher` with the given existing resources.
    fn new_with_existing<R: Iterator<Item = E::Resource>, WI: WatcherInterest<E>>(
        installed: R,
        client_interest: WI,
    ) -> (Self, WatcherSink<E, WI>) {
        let (sender, receiver) = mpsc::channel(max_pending_events::<E>());
        let cancel = Event::new();
        (
            Watcher {
                existing_events: installed
                    .map(E::existing)
                    .filter(|event| client_interest.has_interest_in(event))
                    .chain(std::iter::once(E::IDLE))
                    .collect::<Vec<_>>()
                    .into_iter(),
                receiver,
                canceled: cancel.clone(),
            },
            WatcherSink { sink: sender, cancel, interest: client_interest },
        )
    }

    // Watch returns the currently available events (up to
    // [`fnet_routes::MAX_EVENTS`]). This call will block if there are no
    // available events.
    fn watch(&mut self) -> impl futures::Future<Output = Vec<E>> + Unpin + '_ {
        let Watcher { existing_events, receiver, canceled: _ } = self;
        futures::stream::iter(existing_events.by_ref())
            .chain(receiver)
            // Note: `ready_chunks` blocks until at least 1 event is ready.
            .ready_chunks(fnet_routes::MAX_EVENTS.into())
            .into_future()
            .map(|(r, _ready_chunks)| r.expect("underlying event stream unexpectedly ended"))
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
    use ip_test_macro::ip_test;
    use net_declare::{net_subnet_v4, net_subnet_v6};
    use net_types::ip::Ip;

    use super::*;
    use crate::bindings::routes;
    use crate::bindings::routes::state::RouteTableInterest;

    // We test the common watcher code based on the route table watcher.
    type RouteUpdateDispatcherInner<I> =
        UpdateDispatcherInner<fnet_routes_ext::Event<I>, RouteTableInterest>;
    type RouteUpdateNotifyError<I> = UpdateNotifyError<fnet_routes_ext::Event<I>>;

    fn arbitrary_route_on_interface<I: Ip>(interface: u64) -> fnet_routes_ext::InstalledRoute<I> {
        fnet_routes_ext::InstalledRoute {
            route: fnet_routes_ext::Route {
                destination: I::map_ip(
                    (),
                    |()| net_subnet_v4!("192.168.0.0/24"),
                    |()| net_subnet_v6!("fe80::/64"),
                ),
                action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                    outbound_interface: interface,
                    next_hop: None,
                }),
                properties: fnet_routes_ext::RouteProperties {
                    specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                        metric: fnet_routes::SpecifiedMetric::ExplicitMetric(0),
                    },
                },
            },
            effective_properties: fnet_routes_ext::EffectiveRouteProperties { metric: 0 },
            table_id: fnet_routes_ext::TableId::new(0),
        }
    }

    // Tests that `UpdateDispatcher` returns an error when it receives a
    // `Removed` update for a non-existent route.
    #[ip_test(I)]
    fn dispatcher_fails_to_remove_non_existent<I: Ip>() {
        let route = arbitrary_route_on_interface::<I>(1);
        assert_eq!(
            RouteUpdateDispatcherInner::default().notify(Update::Removed(route.clone())),
            Err(RouteUpdateNotifyError::NotFound(route))
        );
    }

    // Tests that `UpdateDispatcher` returns an error when it receives an
    // `Add` update for an already existing route.
    #[ip_test(I)]
    fn dispatcher_fails_to_add_existing<I: Ip>() {
        let mut dispatcher = RouteUpdateDispatcherInner::default();
        let route = arbitrary_route_on_interface::<I>(1);
        assert_eq!(dispatcher.notify(Update::Added(route)), Ok(()));
        assert_eq!(
            dispatcher.notify(Update::Added(route)),
            Err(RouteUpdateNotifyError::AlreadyExists(route))
        );
    }

    // Tests the basic functionality of the `UpdateDispatcher`,
    // `WatcherSink`, and `Watcher`.
    #[ip_test(I)]
    fn notify_dispatch_watch<I: Ip>() {
        let mut dispatcher = RouteUpdateDispatcherInner::default();

        // Add a new watcher and verify there are no existing routes.
        let mut watcher1 = dispatcher.connect_new_client(RouteTableInterest::All);
        assert_eq!(watcher1.watch().now_or_never().unwrap(), [fnet_routes_ext::Event::<I>::Idle]);

        // Add a route and verify that the watcher is notified.
        let route = arbitrary_route_on_interface(1);
        dispatcher.notify(Update::Added(route)).expect("failed to notify");
        assert_eq!(
            watcher1.watch().now_or_never().unwrap(),
            [fnet_routes_ext::Event::Added(route)]
        );

        // Connect a second watcher and verify it sees the route as `Existing`.
        let mut watcher2 = dispatcher.connect_new_client(RouteTableInterest::All);
        assert_eq!(
            watcher2.watch().now_or_never().unwrap(),
            [fnet_routes_ext::Event::Existing(route), fnet_routes_ext::Event::<I>::Idle]
        );

        // Remove the route and verify both watchers are notified.
        dispatcher.notify(Update::Removed(route)).expect("failed to notify");
        assert_eq!(
            watcher1.watch().now_or_never().unwrap(),
            [fnet_routes_ext::Event::Removed(route)]
        );
        assert_eq!(
            watcher2.watch().now_or_never().unwrap(),
            [fnet_routes_ext::Event::Removed(route)]
        );

        // Disconnect the first client, and verify the second client is still
        // able to be notified.
        dispatcher.disconnect_client(watcher1);
        dispatcher.notify(Update::Added(route)).expect("failed to notify");
        assert_eq!(
            watcher2.watch().now_or_never().unwrap(),
            [fnet_routes_ext::Event::Added(route)]
        );
    }

    // Tests that a `RouteWatcher` is canceled if it exceeds
    // `MAX_PENDING_EVENTS` in its queue.
    #[ip_test(I)]
    fn cancel_watcher_with_too_many_pending_events<I: Ip>() {
        // Helper function to drain the watcher of a specific number of events,
        // which may be spread across multiple batches of size
        // `fnet_routes::MAX_EVENTS`.
        fn drain_watcher<I: Ip>(
            watcher: &mut Watcher<fnet_routes_ext::Event<I>>,
            num_required_events: usize,
        ) {
            let mut num_observed_events = 0;
            while num_observed_events < num_required_events {
                num_observed_events += watcher.watch().now_or_never().unwrap().len()
            }
            assert_eq!(num_observed_events, num_required_events);
        }

        let mut dispatcher = RouteUpdateDispatcherInner::default();
        // `Existing` routes shouldn't count against the client's quota.
        // Exceed the quota, and then verify new clients can still connect.
        // Note that `EXCESS` is 2, because mpsc::channel implicitly adds +1 to
        // the buffer size for every connected sender (and the dispatcher holds
        // a sender).
        const EXCESS: usize = 2;
        const TOO_MANY_EVENTS: usize =
            max_pending_events::<fnet_routes_ext::Event<net_types::ip::Ipv4>>() + EXCESS;
        for i in 0..TOO_MANY_EVENTS {
            let route = arbitrary_route_on_interface::<I>(i.try_into().unwrap());
            dispatcher.notify(Update::Added(route)).expect("failed to notify");
        }
        let mut watcher1 = dispatcher.connect_new_client(RouteTableInterest::All);
        let mut watcher2 = dispatcher.connect_new_client(RouteTableInterest::All);
        assert_eq!(watcher1.canceled.wait().now_or_never(), None);
        assert_eq!(watcher2.canceled.wait().now_or_never(), None);
        // Drain all of the `Existing` events (and +1 for the `Idle` event).
        drain_watcher(&mut watcher1, TOO_MANY_EVENTS + 1);
        drain_watcher(&mut watcher2, TOO_MANY_EVENTS + 1);

        // Generate `TOO_MANY_EVENTS`, consuming the excess on `watcher1` but
        // not on `watcher2`; `watcher2` should be canceled.
        for i in 0..EXCESS {
            assert_eq!(watcher1.canceled.wait().now_or_never(), None);
            assert_eq!(watcher2.canceled.wait().now_or_never(), None);
            let route = arbitrary_route_on_interface::<I>(i.try_into().unwrap());
            dispatcher.notify(Update::Removed(route)).expect("failed to notify");
        }
        drain_watcher(&mut watcher1, EXCESS);
        for i in EXCESS..TOO_MANY_EVENTS {
            assert_eq!(watcher1.canceled.wait().now_or_never(), None);
            assert_eq!(watcher2.canceled.wait().now_or_never(), None);
            let route = arbitrary_route_on_interface::<I>(i.try_into().unwrap());
            dispatcher.notify(Update::Removed(route)).expect("failed to notify");
        }
        assert_eq!(watcher1.canceled.wait().now_or_never(), None);
        assert_eq!(watcher2.canceled.wait().now_or_never(), Some(()));
    }

    #[ip_test(I)]
    fn watcher_respects_interest<I: Ip>() {
        let mut dispatcher = RouteUpdateDispatcherInner::default();
        let main_table_id = routes::main_table_id::<I>();
        let other_table_id = main_table_id.next().expect("no next ID");
        let main_route = fnet_routes_ext::InstalledRoute {
            table_id: main_table_id.into(),
            ..arbitrary_route_on_interface::<I>(0)
        };
        dispatcher.notify(Update::Added(main_route)).expect("failed to notify");
        let other_route = fnet_routes_ext::InstalledRoute {
            table_id: other_table_id.into(),
            ..arbitrary_route_on_interface::<I>(0)
        };
        dispatcher.notify(Update::Added(other_route)).expect("failed to notify");
        let mut all_watcher = dispatcher.connect_new_client(RouteTableInterest::All);
        let mut main_watcher = dispatcher.connect_new_client(RouteTableInterest::Only {
            table_id: routes::main_table_id::<I>().into(),
        });
        let mut other_watcher = dispatcher
            .connect_new_client(RouteTableInterest::Only { table_id: other_table_id.into() });
        // They can get out of order because installed routes are stored in
        // HashSet.
        assert_matches!(
            &all_watcher.watch().now_or_never().unwrap()[..],
            [
                fnet_routes_ext::Event::Existing(installed_1),
                fnet_routes_ext::Event::Existing(installed_2),
                fnet_routes_ext::Event::<I>::Idle
            ] => {
                assert_eq!(
                    HashSet::<fnet_routes_ext::InstalledRoute<I>>::from_iter(
                        [*installed_1, *installed_2]
                    ), HashSet::from_iter([main_route, other_route]));
            }
        );
        assert_eq!(
            main_watcher.watch().now_or_never().unwrap(),
            [fnet_routes_ext::Event::Existing(main_route), fnet_routes_ext::Event::<I>::Idle]
        );
        assert_eq!(
            other_watcher.watch().now_or_never().unwrap(),
            [fnet_routes_ext::Event::Existing(other_route), fnet_routes_ext::Event::<I>::Idle]
        );
        dispatcher.notify(Update::Removed(main_route)).expect("failed to notify");
        dispatcher.notify(Update::Removed(other_route)).expect("failed to notify");
        // For a watcher interested in all tables, it should observe all route
        // changes in all tables.
        assert_eq!(
            all_watcher.watch().now_or_never().unwrap(),
            [
                fnet_routes_ext::Event::Removed(main_route),
                fnet_routes_ext::Event::Removed(other_route),
            ]
        );
        // For a watcher interested in only the main table, it should observe
        // route changes in only the main table.
        assert_eq!(
            main_watcher.watch().now_or_never().unwrap(),
            [fnet_routes_ext::Event::Removed(main_route)]
        );
        // For a watcher interested in only the other table, it should observe
        // route changes in only the other table.
        assert_eq!(
            other_watcher.watch().now_or_never().unwrap(),
            [fnet_routes_ext::Event::Removed(other_route)]
        );
    }
}
