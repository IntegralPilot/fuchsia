// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Test Utilities for the fuchsia.net.routes FIDL library.
//!
//! This library defines a mix of internal and external test utilities,
//! supporting tests of this `fidl_fuchsia_net_routes_ext` crate and tests
//! of clients of the `fuchsia.net.routes` FIDL library, respectively.

use crate::FidlRouteIpExt;

use flex_fuchsia_net_routes as fnet_routes;
use futures::{Future, Stream, StreamExt as _};
use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv6};

/// Responds to the given `Watch` request with the given batch of events.
pub fn handle_watch<I: FidlRouteIpExt>(
    request: <<I::WatcherMarker as flex_client::fidl::ProtocolMarker>::RequestStream as Stream>::Item,
    event_batch: Vec<I::WatchEvent>,
) {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct HandleInputs<I: FidlRouteIpExt> {
        request:
            <<I::WatcherMarker as flex_client::fidl::ProtocolMarker>::RequestStream as Stream>::Item,
        event_batch: Vec<I::WatchEvent>,
    }
    I::map_ip_in(
        HandleInputs { request, event_batch },
        |HandleInputs { request, event_batch }| match request
            .expect("failed to receive `Watch` request")
        {
            fnet_routes::WatcherV4Request::Watch { responder } => {
                responder.send(&event_batch).expect("failed to respond to `Watch`")
            }
        },
        |HandleInputs { request, event_batch }| match request
            .expect("failed to receive `Watch` request")
        {
            fnet_routes::WatcherV6Request::Watch { responder } => {
                responder.send(&event_batch).expect("failed to respond to `Watch`")
            }
        },
    );
}

/// A fake implementation of the `WatcherV4` and `WatcherV6` protocols.
///
/// Feeds events received in `events` as responses to `Watch()`.
pub async fn fake_watcher_impl<I: FidlRouteIpExt>(
    events: impl Stream<Item = Vec<I::WatchEvent>>,
    server_end: flex_client::fidl::ServerEnd<I::WatcherMarker>,
) {
    let (request_stream, _control_handle) = server_end.into_stream_and_control_handle();
    request_stream
        .zip(events)
        .for_each(|(request, event_batch)| {
            handle_watch::<I>(request, event_batch);
            futures::future::ready(())
        })
        .await
}

/// Serve a `GetWatcher` request to the `State` protocol by instantiating a
/// watcher client backed by the given event stream. The returned future
/// drives the watcher implementation.
pub async fn serve_state_request<'a, I: FidlRouteIpExt>(
    request: <<I::StateMarker as flex_client::fidl::ProtocolMarker>::RequestStream as Stream>::Item,
    event_stream: impl Stream<Item = Vec<I::WatchEvent>> + 'a,
) {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct GetWatcherInputs<'a, I: FidlRouteIpExt> {
        request:
            <<I::StateMarker as flex_client::fidl::ProtocolMarker>::RequestStream as Stream>::Item,
        // Use `Box<dyn>` here because `event_stream` needs to have a know size.
        event_stream: Box<dyn Stream<Item = Vec<I::WatchEvent>> + 'a>,
    }
    I::map_ip_in::<
        _,
        // Use `Box<dyn>` here because `event_stream` needs to have a know size.
        // `Pin` ensures that `watcher_fut` implements `Future`.
        std::pin::Pin<Box<dyn Future<Output = ()> + 'a>>,
    >(
        GetWatcherInputs { request, event_stream: Box::new(event_stream) },
        |GetWatcherInputs { request, event_stream }| match request
            .expect("failed to receive `GetWatcherV4` request")
        {
            fnet_routes::StateV4Request::GetWatcherV4 {
                options: _,
                watcher,
                control_handle: _,
            } => Box::pin(fake_watcher_impl::<Ipv4>(Box::into_pin(event_stream), watcher)),
            fnet_routes::StateV4Request::GetRuleWatcherV4 {
                options: _,
                watcher: _,
                control_handle: _,
            } => todo!("TODO(https://fxbug.dev/336204757): Implement rules watcher"),
        },
        |GetWatcherInputs { request, event_stream }| match request
            .expect("failed to receive `GetWatcherV6` request")
        {
            fnet_routes::StateV6Request::GetWatcherV6 {
                options: _,
                watcher,
                control_handle: _,
            } => Box::pin(fake_watcher_impl::<Ipv6>(Box::into_pin(event_stream), watcher)),
            fnet_routes::StateV6Request::GetRuleWatcherV6 {
                options: _,
                watcher: _,
                control_handle: _,
            } => todo!("TODO(https://fxbug.dev/336204757): Implement rules watcher"),
        },
    )
    .await
}

/// Provides a stream of watcher events such that the stack appears to contain
/// no routes and never installs any.
pub fn empty_watch_event_stream<'a, I: FidlRouteIpExt>()
-> impl Stream<Item = Vec<I::WatchEvent>> + 'a {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct Wrap<I: FidlRouteIpExt>(I::WatchEvent);

    let Wrap(event) = I::map_ip(
        (),
        |()| Wrap(fnet_routes::EventV4::Idle(fnet_routes::Empty)),
        |()| Wrap(fnet_routes::EventV6::Idle(fnet_routes::Empty)),
    );
    futures::stream::once(futures::future::ready(vec![event])).chain(futures::stream::pending())
}

/// Provides testutils for testing implementations of clients and servers of
/// fuchsia.net.routes.admin.
pub mod admin {
    use flex_client::fidl::ProtocolMarker;
    use flex_fuchsia_net_routes_admin as fnet_routes_admin;
    use futures::{Stream, StreamExt as _, TryStreamExt as _};
    use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv6};

    use crate::Responder;
    use crate::admin::{FidlRouteAdminIpExt, RouteSetRequest, RouteTableRequest};

    /// Provides a RouteTable implementation that provides one RouteSet and
    /// then panics on subsequent invocations. Returns the request stream for
    /// that RouteSet.
    pub fn serve_one_route_set<I: FidlRouteAdminIpExt>(
        server_end: flex_client::fidl::ServerEnd<I::RouteTableMarker>,
    ) -> impl Stream<
            Item = <
                    <<I as FidlRouteAdminIpExt>::RouteSetMarker as ProtocolMarker>
                        ::RequestStream as Stream
                >::Item
    >{
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct In<I: FidlRouteAdminIpExt>(
            <<<I as FidlRouteAdminIpExt>::RouteTableMarker as ProtocolMarker>
                ::RequestStream as Stream
            >::Item,
        );
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Out<I: FidlRouteAdminIpExt>(flex_client::fidl::ServerEnd<I::RouteSetMarker>);

        let stream = server_end.into_stream();
        stream
            .scan(false, |responded, item| {
                let responded = std::mem::replace(responded, true);
                if responded {
                    panic!("received multiple RouteTable requests");
                }

                futures::future::ready(Some(item))
            })
            .map(|item| {
                let Out(route_set_server_end) = I::map_ip(
                    In(item),
                    |In(item)| match item.expect("set provider FIDL error") {
                        fnet_routes_admin::RouteTableV4Request::NewRouteSet {
                            route_set,
                            control_handle: _,
                        } => Out(route_set),
                        req => unreachable!("unexpected request: {:?}", req),
                    },
                    |In(item)| match item.expect("set provider FIDL error") {
                        fnet_routes_admin::RouteTableV6Request::NewRouteSet {
                            route_set,
                            control_handle: _,
                        } => Out(route_set),
                        req => unreachable!("unexpected request: {:?}", req),
                    },
                );
                route_set_server_end.into_stream()
            })
            .fuse()
            .flatten_unordered(None)
    }

    /// TODO(https://fxbug.dev/337298251): Change this to return a RouteSet
    /// index beside the RouteSet item to make it easier to determine which
    /// RouteSet pertains to the transmitted requests.
    ///
    /// Provides a RouteTable implementation that consolidates all RouteSets into
    /// a single RouteSet stream. Returns a request stream that vends items as
    /// they arrive in any RouteSet.
    pub fn serve_all_route_sets_with_table_id<I: FidlRouteAdminIpExt>(
        server_end: flex_client::fidl::ServerEnd<I::RouteTableMarker>,
        table_id: Option<crate::TableId>,
    ) -> impl Stream<
            Item = <
                    <<I as FidlRouteAdminIpExt>::RouteSetMarker as ProtocolMarker>
                        ::RequestStream as Stream
                >::Item
    >{
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct In<I: FidlRouteAdminIpExt>(
            <<<I as FidlRouteAdminIpExt>::RouteTableMarker as ProtocolMarker>
                ::RequestStream as Stream
            >::Item,
        );
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Out<I: FidlRouteAdminIpExt>(Option<flex_client::fidl::ServerEnd<I::RouteSetMarker>>);

        let stream = server_end.into_stream();
        stream
            .map(move |item| {
                let Out(route_set_server_end) = I::map_ip(
                    In(item),
                    |In(item)| match item.expect("set provider FIDL error") {
                        fnet_routes_admin::RouteTableV4Request::NewRouteSet {
                            route_set,
                            control_handle: _,
                        } => Out(Some(route_set)),
                        fnet_routes_admin::RouteTableV4Request::GetTableId { responder } => {
                            responder
                                .send(
                                    table_id
                                        .unwrap_or_else(|| panic!("GetTableId not supported"))
                                        .get(),
                                )
                                .expect("error responding to GetTableId");
                            Out(None)
                        }
                        req => unreachable!("unexpected request: {:?}", req),
                    },
                    |In(item)| match item.expect("set provider FIDL error") {
                        fnet_routes_admin::RouteTableV6Request::NewRouteSet {
                            route_set,
                            control_handle: _,
                        } => Out(Some(route_set)),
                        fnet_routes_admin::RouteTableV6Request::GetTableId { responder } => {
                            responder
                                .send(
                                    table_id
                                        .unwrap_or_else(|| panic!("GetTableId not supported"))
                                        .get(),
                                )
                                .expect("error responding to GetTableId");
                            Out(None)
                        }
                        req => unreachable!("unexpected request: {:?}", req),
                    },
                );
                match route_set_server_end {
                    None => futures::stream::empty().left_stream(),
                    Some(route_set_server_end) => route_set_server_end.into_stream().right_stream(),
                }
            })
            .fuse()
            .flatten_unordered(None)
    }

    /// Provides a RouteTable implementation that serves no-op RouteSets and identifies itself
    /// with the given ID.
    pub async fn serve_noop_route_sets_with_table_id<I: FidlRouteAdminIpExt>(
        server_end: flex_client::fidl::ServerEnd<I::RouteTableMarker>,
        table_id: crate::TableId,
    ) {
        let stream = server_end.into_stream();
        stream
            .try_for_each_concurrent(None, |item| async move {
                let request = I::into_route_table_request(item);
                match request {
                    RouteTableRequest::NewRouteSet { route_set, control_handle: _ } => {
                        serve_noop_route_set::<I>(route_set).await
                    }
                    RouteTableRequest::GetTableId { responder } => {
                        responder.send(table_id.get()).expect("responding should succeed");
                    }
                    request => panic!("unexpected request: {request:?}"),
                }
                Ok(())
            })
            .await
            .expect("serving no-op route sets should succeed");
    }

    /// Provides a RouteTable implementation that serves no-op RouteSets.
    pub async fn serve_noop_route_sets<I: FidlRouteAdminIpExt>(
        server_end: flex_client::fidl::ServerEnd<I::RouteTableMarker>,
    ) {
        serve_noop_route_sets_with_table_id::<I>(server_end, crate::TableId::new(0)).await
    }

    /// Serves a RouteSet that returns OK for everything and does nothing.
    async fn serve_noop_route_set<I: FidlRouteAdminIpExt>(
        server_end: flex_client::fidl::ServerEnd<I::RouteSetMarker>,
    ) {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Wrap<I: FidlRouteAdminIpExt>(
            <<<I as FidlRouteAdminIpExt>::RouteSetMarker as ProtocolMarker>
                ::RequestStream as Stream
            >::Item,
        );

        let stream = server_end.into_stream();
        stream
            .for_each(|item| async move {
                let request: RouteSetRequest<I> = I::map_ip(
                    Wrap(item),
                    |Wrap(item)| RouteSetRequest::<Ipv4>::from(item.expect("route set FIDL error")),
                    |Wrap(item)| RouteSetRequest::<Ipv6>::from(item.expect("route set FIDL error")),
                );
                match request {
                    RouteSetRequest::AddRoute { route, responder } => {
                        let _: crate::Route<I> = route.expect("AddRoute called with invalid route");
                        responder.send(Ok(true)).expect("respond to AddRoute");
                    }
                    RouteSetRequest::RemoveRoute { route, responder } => {
                        let _: crate::Route<I> =
                            route.expect("RemoveRoute called with invalid route");
                        responder.send(Ok(true)).expect("respond to RemoveRoute");
                    }
                    RouteSetRequest::AuthenticateForInterface { credential: _, responder } => {
                        responder.send(Ok(())).expect("respond to AuthenticateForInterface");
                    }
                }
            })
            .await;
    }
}

/// Provides testutils for testing implementations of clients and servers for routes rules FIDL.
pub mod rules {
    use flex_fuchsia_net_routes as fnet_routes;
    use futures::{Stream, StreamExt as _, TryStreamExt as _};
    use net_types::ip::{GenericOverIp, Ip};

    use crate::Responder;
    use crate::rules::{FidlRuleAdminIpExt, FidlRuleIpExt, RuleSetRequest, RuleTableRequest};

    // Responds to the given `Watch` request with the given batch of events.
    fn handle_watch<I: FidlRuleIpExt>(
        request: <<I::RuleWatcherMarker as flex_client::fidl::ProtocolMarker>::RequestStream as Stream>::Item,
        event_batch: Vec<I::RuleEvent>,
    ) {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct HandleInputs<I: FidlRuleIpExt> {
            request:
                <<I::RuleWatcherMarker as flex_client::fidl::ProtocolMarker>::RequestStream as Stream>::Item,
            event_batch: Vec<I::RuleEvent>,
        }
        I::map_ip_in(
            HandleInputs { request, event_batch },
            |HandleInputs { request, event_batch }| match request
                .expect("failed to receive `Watch` request")
            {
                fnet_routes::RuleWatcherV4Request::Watch { responder } => {
                    responder.send(&event_batch).expect("failed to respond to `Watch`")
                }
            },
            |HandleInputs { request, event_batch }| match request
                .expect("failed to receive `Watch` request")
            {
                fnet_routes::RuleWatcherV6Request::Watch { responder } => {
                    responder.send(&event_batch).expect("failed to respond to `Watch`")
                }
            },
        );
    }

    /// A fake implementation of the `WatcherV4` and `WatcherV6` protocols.
    ///
    /// Feeds events received in `events` as responses to `Watch()`.
    pub async fn fake_rules_watcher_impl<I: FidlRuleIpExt>(
        events: impl Stream<Item = Vec<I::RuleEvent>>,
        server_end: flex_client::fidl::ServerEnd<I::RuleWatcherMarker>,
    ) {
        let (request_stream, _control_handle) = server_end.into_stream_and_control_handle();
        request_stream
            .zip(events)
            .for_each(|(request, event_batch)| {
                handle_watch::<I>(request, event_batch);
                futures::future::ready(())
            })
            .await
    }

    /// Provides a stream of watcher events such that the stack appears to contain
    /// no routes and never installs any.
    pub fn empty_watch_event_stream<'a, I: FidlRuleIpExt>()
    -> impl Stream<Item = Vec<I::RuleEvent>> + 'a {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Wrap<I: FidlRuleIpExt>(I::RuleEvent);

        let Wrap(event) = I::map_ip(
            (),
            |()| Wrap(fnet_routes::RuleEventV4::Idle(fnet_routes::Empty)),
            |()| Wrap(fnet_routes::RuleEventV6::Idle(fnet_routes::Empty)),
        );
        futures::stream::once(futures::future::ready(vec![event])).chain(futures::stream::pending())
    }

    /// Provides a RuleTable implementation that serves no-op RuleSets.
    pub async fn serve_noop_rule_sets<I: FidlRuleAdminIpExt>(
        server_end: flex_client::fidl::ServerEnd<I::RuleTableMarker>,
    ) {
        let stream = server_end.into_stream();
        stream
            .and_then(|item| async move {
                let RuleTableRequest::NewRuleSet { priority: _, rule_set, control_handle: _ } =
                    I::into_rule_table_request(item);
                serve_noop_rule_set::<I>(rule_set).await;
                Ok(())
            })
            .for_each_concurrent(None, |item| {
                item.expect("should not get error");
                futures::future::ready(())
            })
            .await;
    }

    /// Serves a RuleSet that returns OK for everything and does nothing.
    pub async fn serve_noop_rule_set<I: FidlRuleAdminIpExt>(
        server_end: flex_client::fidl::ServerEnd<I::RuleSetMarker>,
    ) {
        let stream = server_end.into_stream();
        stream
            .try_for_each(|item| async move {
                let request = I::into_rule_set_request(item);
                match request {
                    RuleSetRequest::AddRule { index: _, matcher, action: _, responder } => {
                        let _: crate::rules::RuleMatcher<_> =
                            matcher.expect("AddRule called with invalid matcher");
                        responder.send(Ok(())).expect("respond to AddRule");
                    }
                    RuleSetRequest::RemoveRule { index: _, responder } => {
                        responder.send(Ok(())).expect("respond to RemoveRule");
                    }
                    RuleSetRequest::AuthenticateForRouteTable { table: _, token: _, responder } => {
                        responder.send(Ok(())).expect("respond to AuthenticateForRouteTable")
                    }
                    RuleSetRequest::Close { control_handle: _ } => {}
                };
                Ok(())
            })
            .await
            .expect("should not get error");
    }
}

#[cfg(test)]
pub(crate) mod internal {
    use super::*;
    use net_declare::{fidl_ip_v4_with_prefix, fidl_ip_v6_with_prefix};

    // Generates an arbitrary `I::WatchEvent` that is unique for the given `seed`.
    pub(crate) fn generate_event<I: FidlRouteIpExt>(seed: u32) -> I::WatchEvent {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct BuildEventOutput<I: FidlRouteIpExt>(I::WatchEvent);
        let BuildEventOutput(event) = I::map_ip_out(
            seed,
            |seed| {
                BuildEventOutput(fnet_routes::EventV4::Added(fnet_routes::InstalledRouteV4 {
                    route: Some(fnet_routes::RouteV4 {
                        destination: fidl_ip_v4_with_prefix!("192.168.0.0/24"),
                        action: fnet_routes::RouteActionV4::Forward(fnet_routes::RouteTargetV4 {
                            outbound_interface: 1,
                            next_hop: None,
                        }),
                        properties: fnet_routes::RoutePropertiesV4 {
                            specified_properties: Some(fnet_routes::SpecifiedRouteProperties {
                                metric: Some(fnet_routes::SpecifiedMetric::ExplicitMetric(seed)),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    }),
                    effective_properties: Some(fnet_routes::EffectiveRouteProperties {
                        metric: Some(seed),
                        ..Default::default()
                    }),
                    table_id: Some(0),
                    ..Default::default()
                }))
            },
            |seed| {
                BuildEventOutput(fnet_routes::EventV6::Added(fnet_routes::InstalledRouteV6 {
                    route: Some(fnet_routes::RouteV6 {
                        destination: fidl_ip_v6_with_prefix!("fe80::0/64"),
                        action: fnet_routes::RouteActionV6::Forward(fnet_routes::RouteTargetV6 {
                            outbound_interface: 1,
                            next_hop: None,
                        }),
                        properties: fnet_routes::RoutePropertiesV6 {
                            specified_properties: Some(fnet_routes::SpecifiedRouteProperties {
                                metric: Some(fnet_routes::SpecifiedMetric::ExplicitMetric(seed)),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    }),
                    effective_properties: Some(fnet_routes::EffectiveRouteProperties {
                        metric: Some(seed),
                        ..Default::default()
                    }),
                    table_id: Some(0),
                    ..Default::default()
                }))
            },
        );
        event
    }

    // Same as `generate_event()` except that it operates over a range of `seeds`,
    // producing `n` `I::WatchEvents` where `n` is the size of the range.
    pub(crate) fn generate_events_in_range<I: FidlRouteIpExt>(
        seeds: std::ops::Range<u32>,
    ) -> Vec<I::WatchEvent> {
        seeds.into_iter().map(|seed| generate_event::<I>(seed)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::FidlRouteAdminIpExt;
    use crate::testutil::internal as internal_testutil;
    use crate::{get_watcher, watch};
    use assert_matches::assert_matches;
    use futures::FutureExt;
    use ip_test_macro::ip_test;
    use test_case::test_case;

    // Tests the `fake_watcher_impl` with various "shapes". The test parameter
    // is a vec of ranges, where each range corresponds to the batch of events
    // that will be sent in response to a single call to `Watch().
    #[ip_test(I)]
    #[test_case(Vec::new(); "no events")]
    #[test_case(vec![0..1]; "single_batch_single_event")]
    #[test_case(vec![0..10]; "single_batch_many_events")]
    #[test_case(vec![0..10, 10..20, 20..30]; "many_batches_many_events")]
    #[fuchsia_async::run_singlethreaded(test)]
    async fn fake_watcher_impl_against_shape<I: FidlRouteIpExt>(
        test_shape: Vec<std::ops::Range<u32>>,
    ) {
        // Build the event stream based on the `test_shape`. Use a channel
        // so that the stream stays open until `close_channel` is called later.
        let (event_stream_sender, event_stream_receiver) =
            futures::channel::mpsc::unbounded::<Vec<I::WatchEvent>>();
        for batch_shape in &test_shape {
            event_stream_sender
                .unbounded_send(internal_testutil::generate_events_in_range::<I>(
                    batch_shape.clone(),
                ))
                .expect("failed to send event batch");
        }

        // Instantiate the fake Watcher implementation.
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        #[cfg(not(feature = "fdomain"))]
        let client = fidl::endpoints::ZirconClient;
        let (state, state_server_end) = client.create_proxy::<I::StateMarker>();
        let (mut state_request_stream, _control_handle) =
            state_server_end.into_stream_and_control_handle();
        let watcher_fut = state_request_stream
            .next()
            .then(|req| {
                serve_state_request::<I>(
                    req.expect("State request_stream unexpectedly ended"),
                    event_stream_receiver,
                )
            })
            .fuse();
        futures::pin_mut!(watcher_fut);

        // Drive the watcher, asserting it observes the expected data.
        let watcher = get_watcher::<I>(&state, Default::default()).expect("failed to get watcher");
        for batch_shape in test_shape {
            futures::select!(
                 () = watcher_fut => panic!("fake watcher implementation unexpectedly finished"),
                events = watch::<I>(&watcher).fuse() => assert_eq!(
                    events.expect("failed to watch for events"),
                    internal_testutil::generate_events_in_range::<I>(batch_shape.clone())));
        }

        // Close the event_stream_sender and observe the watcher_impl finish.
        event_stream_sender.close_channel();
        watcher_fut.await;

        // Trying to watch again after we've exhausted the data should
        // result in `PEER_CLOSED`.
        assert_matches!(
            watch::<I>(&watcher).await,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );
    }

    // `serve_one_route_set` should panic if the caller makes a call
    // to `new_route_set` more than once.
    #[ip_test(I)]
    #[fuchsia_async::run_singlethreaded]
    #[should_panic(expected = "received multiple RouteTable requests")]
    async fn test_serve_one_route_set_panic<I: FidlRouteAdminIpExt>() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        #[cfg(not(feature = "fdomain"))]
        let client = fidl::endpoints::ZirconClient;
        let (routes_set_provider_proxy, routes_set_provider_server_end) =
            client.create_proxy::<I::RouteTableMarker>();
        let mut provider = admin::serve_one_route_set::<I>(routes_set_provider_server_end);
        let _rs1 = crate::admin::new_route_set::<I>(&routes_set_provider_proxy)
            .expect("created first RouteSet");
        let _rs2 = crate::admin::new_route_set::<I>(&routes_set_provider_proxy)
            .expect("created second RouteSet");

        // This should panic, as the route set provider pushes `RouteSet` server
        // ends through by handling the `new_route_set` requests, which will
        // cause a panic on the second iteration.
        let _ = provider.next().await;
    }
}
