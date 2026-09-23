// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extension crate for the `fuchsia.net.multicast.admin` FIDL API.
//!
//! This crate provides types and traits to abstract the separate IPv4 and IPv6
//! FIDL APIs onto a single API surface that is generic over IP version.

use fidl_fuchsia_net_ext::IntoExt as _;
use fidl_fuchsia_net_multicast_admin::{self as fnet_multicast_admin, TableControllerCloseReason};
use futures::stream::TryStream;
use futures::{Stream, StreamExt, TryFutureExt};
use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv6};

/// A type capable of responding to FIDL requests.
pub trait FidlResponder<R> {
    /// Try to send the given response over this responder.
    fn try_send(self, response: R) -> Result<(), fidl::Error>;
}

/// A FIDL ControlHandle that can send a terminal event.
pub trait TerminalEventControlHandle<E> {
    /// Send the given terminal event
    fn send_terminal_event(&self, terminal_event: E) -> Result<(), fidl::Error>;
}

/// A FIDL multicast routing table controller Proxy.
///
/// An IP generic abstraction over
/// [`fnet_multicast_admin::Ipv4RoutingTableControllerProxy`], and
/// [`fnet_multicast_admin::Ipv6RoutingTableControllerProxy`].
pub trait TableControllerProxy<I: FidlMulticastAdminIpExt> {
    fn take_event_stream(
        &self,
    ) -> impl Stream<Item = Result<TableControllerCloseReason, fidl::Error>> + std::marker::Unpin;

    fn add_route(
        &self,
        addresses: UnicastSourceAndMulticastDestination<I>,
        route: Route,
    ) -> impl futures::Future<Output = Result<Result<(), AddRouteError>, fidl::Error>>;

    fn del_route(
        &self,
        addresses: UnicastSourceAndMulticastDestination<I>,
    ) -> impl futures::Future<Output = Result<Result<(), DelRouteError>, fidl::Error>>;

    fn get_route_stats(
        &self,
        addresses: UnicastSourceAndMulticastDestination<I>,
    ) -> impl futures::Future<
        Output = Result<Result<fnet_multicast_admin::RouteStats, GetRouteStatsError>, fidl::Error>,
    >;

    fn watch_routing_events(
        &self,
    ) -> impl futures::Future<Output = Result<WatchRoutingEventsResponse<I>, fidl::Error>>;
}

/// An IP extension providing functionality for `fuchsia_net_multicast_admin`.
pub trait FidlMulticastAdminIpExt: Ip {
    /// Protocol Marker for the multicast routing table controller.
    type TableControllerMarker: fidl::endpoints::DiscoverableProtocolMarker<
            RequestStream = Self::TableControllerRequestStream,
            Proxy = Self::TableControllerProxy,
        >;
    /// Request Stream for the multicast routing table controller.
    type TableControllerRequestStream: fidl::endpoints::RequestStream<
            Ok: Send + Into<TableControllerRequest<Self>>,
            ControlHandle: Send + TerminalEventControlHandle<TableControllerCloseReason>,
            Item = Result<
                <Self::TableControllerRequestStream as TryStream>::Ok,
                <Self::TableControllerRequestStream as TryStream>::Error,
            >,
        >;
    type TableControllerProxy: fidl::endpoints::Proxy + TableControllerProxy<Self>;
    /// The Unicast Source and Multicast Destination address tuple.
    type Addresses: Into<UnicastSourceAndMulticastDestination<Self>>;
    /// A [`FidlResponder`] for multicast routing table AddRoute requests.
    type AddRouteResponder: FidlResponder<Result<(), AddRouteError>>;
    /// A [`FidlResponder`] for multicast routing table DelRoute requests.
    type DelRouteResponder: FidlResponder<Result<(), DelRouteError>>;
    /// A [`FidlResponder`] for multicast routing table GetRouteStats requests.
    type GetRouteStatsResponder: for<'a> FidlResponder<
        Result<&'a fnet_multicast_admin::RouteStats, GetRouteStatsError>,
    >;
    /// A [`FidlResponder`] for multicast routing table WatchRoutingEvents
    /// requests.
    type WatchRoutingEventsResponder: FidlResponder<WatchRoutingEventsResponse<Self>>;
}

impl FidlMulticastAdminIpExt for Ipv4 {
    type TableControllerMarker = fnet_multicast_admin::Ipv4RoutingTableControllerMarker;
    type TableControllerRequestStream =
        fnet_multicast_admin::Ipv4RoutingTableControllerRequestStream;
    type TableControllerProxy = fnet_multicast_admin::Ipv4RoutingTableControllerProxy;
    type Addresses = fnet_multicast_admin::Ipv4UnicastSourceAndMulticastDestination;
    type AddRouteResponder = fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteResponder;
    type DelRouteResponder = fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteResponder;
    type GetRouteStatsResponder =
        fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsResponder;
    type WatchRoutingEventsResponder =
        fnet_multicast_admin::Ipv4RoutingTableControllerWatchRoutingEventsResponder;
}

impl FidlMulticastAdminIpExt for Ipv6 {
    type TableControllerMarker = fnet_multicast_admin::Ipv6RoutingTableControllerMarker;
    type TableControllerRequestStream =
        fnet_multicast_admin::Ipv6RoutingTableControllerRequestStream;
    type TableControllerProxy = fnet_multicast_admin::Ipv6RoutingTableControllerProxy;
    type Addresses = fnet_multicast_admin::Ipv6UnicastSourceAndMulticastDestination;
    type AddRouteResponder = fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteResponder;
    type DelRouteResponder = fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteResponder;
    type GetRouteStatsResponder =
        fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsResponder;
    type WatchRoutingEventsResponder =
        fnet_multicast_admin::Ipv6RoutingTableControllerWatchRoutingEventsResponder;
}

impl TerminalEventControlHandle<TableControllerCloseReason>
    for fnet_multicast_admin::Ipv4RoutingTableControllerControlHandle
{
    fn send_terminal_event(
        &self,
        terminal_event: TableControllerCloseReason,
    ) -> Result<(), fidl::Error> {
        self.send_on_close(terminal_event)
    }
}

impl TerminalEventControlHandle<TableControllerCloseReason>
    for fnet_multicast_admin::Ipv6RoutingTableControllerControlHandle
{
    fn send_terminal_event(
        &self,
        terminal_event: TableControllerCloseReason,
    ) -> Result<(), fidl::Error> {
        self.send_on_close(terminal_event)
    }
}

impl TableControllerProxy<Ipv4> for fnet_multicast_admin::Ipv4RoutingTableControllerProxy {
    fn take_event_stream(
        &self,
    ) -> impl Stream<Item = Result<TableControllerCloseReason, fidl::Error>> {
        self.take_event_stream().map(|e| {
            e.map(|e| match e {
                fnet_multicast_admin::Ipv4RoutingTableControllerEvent::OnClose { error } => error,
            })
        })
    }

    fn add_route(
        &self,
        addresses: UnicastSourceAndMulticastDestination<Ipv4>,
        route: Route,
    ) -> impl futures::Future<Output = Result<Result<(), AddRouteError>, fidl::Error>> {
        self.add_route(&addresses.into(), &route.into())
            .map_ok(|inner| inner.map_err(AddRouteError::from))
    }

    fn del_route(
        &self,
        addresses: UnicastSourceAndMulticastDestination<Ipv4>,
    ) -> impl futures::Future<Output = Result<Result<(), DelRouteError>, fidl::Error>> {
        self.del_route(&addresses.into()).map_ok(|inner| inner.map_err(DelRouteError::from))
    }

    fn get_route_stats(
        &self,
        addresses: UnicastSourceAndMulticastDestination<Ipv4>,
    ) -> impl futures::Future<
        Output = Result<Result<fnet_multicast_admin::RouteStats, GetRouteStatsError>, fidl::Error>,
    > {
        self.get_route_stats(&addresses.into())
            .map_ok(|inner| inner.map_err(GetRouteStatsError::from))
    }

    fn watch_routing_events(
        &self,
    ) -> impl futures::Future<Output = Result<WatchRoutingEventsResponse<Ipv4>, fidl::Error>> {
        self.watch_routing_events().map_ok(|(dropped_events, addresses, input_interface, event)| {
            WatchRoutingEventsResponse {
                dropped_events,
                addresses: addresses.into(),
                input_interface,
                event,
            }
        })
    }
}

impl TableControllerProxy<Ipv6> for fnet_multicast_admin::Ipv6RoutingTableControllerProxy {
    fn take_event_stream(
        &self,
    ) -> impl Stream<Item = Result<TableControllerCloseReason, fidl::Error>> {
        self.take_event_stream().map(|e| {
            e.map(|e| match e {
                fnet_multicast_admin::Ipv6RoutingTableControllerEvent::OnClose { error } => error,
            })
        })
    }

    fn add_route(
        &self,
        addresses: UnicastSourceAndMulticastDestination<Ipv6>,
        route: Route,
    ) -> impl futures::Future<Output = Result<Result<(), AddRouteError>, fidl::Error>> {
        self.add_route(&addresses.into(), &route.into())
            .map_ok(|inner| inner.map_err(AddRouteError::from))
    }

    fn del_route(
        &self,
        addresses: UnicastSourceAndMulticastDestination<Ipv6>,
    ) -> impl futures::Future<Output = Result<Result<(), DelRouteError>, fidl::Error>> {
        self.del_route(&addresses.into()).map_ok(|inner| inner.map_err(DelRouteError::from))
    }

    fn get_route_stats(
        &self,
        addresses: UnicastSourceAndMulticastDestination<Ipv6>,
    ) -> impl futures::Future<
        Output = Result<Result<fnet_multicast_admin::RouteStats, GetRouteStatsError>, fidl::Error>,
    > {
        self.get_route_stats(&addresses.into())
            .map_ok(|inner| inner.map_err(GetRouteStatsError::from))
    }

    fn watch_routing_events(
        &self,
    ) -> impl futures::Future<Output = Result<WatchRoutingEventsResponse<Ipv6>, fidl::Error>> {
        self.watch_routing_events().map_ok(|(dropped_events, addresses, input_interface, event)| {
            WatchRoutingEventsResponse {
                dropped_events,
                addresses: addresses.into(),
                input_interface,
                event,
            }
        })
    }
}

/// An IP generic version of
/// [`fnet_multicast_admin::Ipv4RoutingTableControllerRequest`] and
/// [`fnet_multicast_admin::Ipv6RoutingTableControllerRequest`].
#[derive(Debug)]
pub enum TableControllerRequest<I: FidlMulticastAdminIpExt> {
    AddRoute {
        addresses: UnicastSourceAndMulticastDestination<I>,
        route: fnet_multicast_admin::Route,
        responder: I::AddRouteResponder,
    },
    DelRoute {
        addresses: UnicastSourceAndMulticastDestination<I>,
        responder: I::DelRouteResponder,
    },
    GetRouteStats {
        addresses: UnicastSourceAndMulticastDestination<I>,
        responder: I::GetRouteStatsResponder,
    },
    WatchRoutingEvents {
        responder: I::WatchRoutingEventsResponder,
    },
}

impl From<fnet_multicast_admin::Ipv4RoutingTableControllerRequest>
    for TableControllerRequest<Ipv4>
{
    fn from(request: fnet_multicast_admin::Ipv4RoutingTableControllerRequest) -> Self {
        use fnet_multicast_admin::Ipv4RoutingTableControllerRequest as R;
        match request {
            R::AddRoute { addresses, route, responder } => {
                TableControllerRequest::AddRoute { addresses: addresses.into(), route, responder }
            }
            R::DelRoute { addresses, responder } => {
                TableControllerRequest::DelRoute { addresses: addresses.into(), responder }
            }
            R::GetRouteStats { addresses, responder } => {
                TableControllerRequest::GetRouteStats { addresses: addresses.into(), responder }
            }
            R::WatchRoutingEvents { responder } => {
                TableControllerRequest::WatchRoutingEvents { responder }
            }
        }
    }
}

impl From<fnet_multicast_admin::Ipv6RoutingTableControllerRequest>
    for TableControllerRequest<Ipv6>
{
    fn from(request: fnet_multicast_admin::Ipv6RoutingTableControllerRequest) -> Self {
        use fnet_multicast_admin::Ipv6RoutingTableControllerRequest as R;
        match request {
            R::AddRoute { addresses, route, responder } => {
                TableControllerRequest::AddRoute { addresses: addresses.into(), route, responder }
            }
            R::DelRoute { addresses, responder } => {
                TableControllerRequest::DelRoute { addresses: addresses.into(), responder }
            }
            R::GetRouteStats { addresses, responder } => {
                TableControllerRequest::GetRouteStats { addresses: addresses.into(), responder }
            }
            R::WatchRoutingEvents { responder } => {
                TableControllerRequest::WatchRoutingEvents { responder }
            }
        }
    }
}

/// An IP generic version of
/// [`fnet_multicast_admin::Ipv4UnicastSourceAndMulticastDestination`] and
/// [`fnet_multicast_admin::Ipv6UnicastSourceAndMulticastDestination`].
#[derive(Debug, Clone, GenericOverIp, PartialEq)]
#[generic_over_ip(I, Ip)]
pub struct UnicastSourceAndMulticastDestination<I: Ip> {
    pub unicast_source: I::Addr,
    pub multicast_destination: I::Addr,
}

impl From<fnet_multicast_admin::Ipv4UnicastSourceAndMulticastDestination>
    for UnicastSourceAndMulticastDestination<Ipv4>
{
    fn from(addresses: fnet_multicast_admin::Ipv4UnicastSourceAndMulticastDestination) -> Self {
        let fnet_multicast_admin::Ipv4UnicastSourceAndMulticastDestination {
            unicast_source,
            multicast_destination,
        } = addresses;
        UnicastSourceAndMulticastDestination {
            unicast_source: unicast_source.into_ext(),
            multicast_destination: multicast_destination.into_ext(),
        }
    }
}

impl From<fnet_multicast_admin::Ipv6UnicastSourceAndMulticastDestination>
    for UnicastSourceAndMulticastDestination<Ipv6>
{
    fn from(addresses: fnet_multicast_admin::Ipv6UnicastSourceAndMulticastDestination) -> Self {
        let fnet_multicast_admin::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source,
            multicast_destination,
        } = addresses;
        UnicastSourceAndMulticastDestination {
            unicast_source: unicast_source.into_ext(),
            multicast_destination: multicast_destination.into_ext(),
        }
    }
}

impl From<UnicastSourceAndMulticastDestination<Ipv4>>
    for fnet_multicast_admin::Ipv4UnicastSourceAndMulticastDestination
{
    fn from(addresses: UnicastSourceAndMulticastDestination<Ipv4>) -> Self {
        let UnicastSourceAndMulticastDestination { unicast_source, multicast_destination } =
            addresses;
        fnet_multicast_admin::Ipv4UnicastSourceAndMulticastDestination {
            unicast_source: unicast_source.into_ext(),
            multicast_destination: multicast_destination.into_ext(),
        }
    }
}

impl From<UnicastSourceAndMulticastDestination<Ipv6>>
    for fnet_multicast_admin::Ipv6UnicastSourceAndMulticastDestination
{
    fn from(addresses: UnicastSourceAndMulticastDestination<Ipv6>) -> Self {
        let UnicastSourceAndMulticastDestination { unicast_source, multicast_destination } =
            addresses;
        fnet_multicast_admin::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: unicast_source.into_ext(),
            multicast_destination: multicast_destination.into_ext(),
        }
    }
}

/// An IP generic version of
/// [`fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError`] and
/// [`fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError`].
#[derive(Debug, Clone, PartialEq)]
pub enum AddRouteError {
    InvalidAddress,
    RequiredRouteFieldsMissing,
    InterfaceNotFound,
    InputCannotBeOutput,
    DuplicateOutput,
    InvalidTtl,
}

impl From<fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError> for AddRouteError {
    fn from(error: fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError) -> Self {
        use fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError as E;
        match error {
            E::InvalidAddress => AddRouteError::InvalidAddress,
            E::RequiredRouteFieldsMissing => AddRouteError::RequiredRouteFieldsMissing,
            E::InterfaceNotFound => AddRouteError::InterfaceNotFound,
            E::InputCannotBeOutput => AddRouteError::InputCannotBeOutput,
            E::DuplicateOutput => AddRouteError::DuplicateOutput,
            E::InvalidTtl => AddRouteError::InvalidTtl,
        }
    }
}

impl From<fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError> for AddRouteError {
    fn from(error: fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError) -> Self {
        use fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError as E;
        match error {
            E::InvalidAddress => AddRouteError::InvalidAddress,
            E::RequiredRouteFieldsMissing => AddRouteError::RequiredRouteFieldsMissing,
            E::InterfaceNotFound => AddRouteError::InterfaceNotFound,
            E::InputCannotBeOutput => AddRouteError::InputCannotBeOutput,
            E::DuplicateOutput => AddRouteError::DuplicateOutput,
            E::InvalidTtl => AddRouteError::InvalidTtl,
        }
    }
}

impl From<AddRouteError> for fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError {
    fn from(error: AddRouteError) -> Self {
        use fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError as E;
        match error {
            AddRouteError::InvalidAddress => E::InvalidAddress,
            AddRouteError::RequiredRouteFieldsMissing => E::RequiredRouteFieldsMissing,
            AddRouteError::InterfaceNotFound => E::InterfaceNotFound,
            AddRouteError::InputCannotBeOutput => E::InputCannotBeOutput,
            AddRouteError::DuplicateOutput => E::DuplicateOutput,
            AddRouteError::InvalidTtl => E::InvalidTtl,
        }
    }
}

impl From<AddRouteError> for fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError {
    fn from(error: AddRouteError) -> Self {
        use fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError as E;
        match error {
            AddRouteError::InvalidAddress => E::InvalidAddress,
            AddRouteError::RequiredRouteFieldsMissing => E::RequiredRouteFieldsMissing,
            AddRouteError::InterfaceNotFound => E::InterfaceNotFound,
            AddRouteError::InputCannotBeOutput => E::InputCannotBeOutput,
            AddRouteError::DuplicateOutput => E::DuplicateOutput,
            AddRouteError::InvalidTtl => E::InvalidTtl,
        }
    }
}

impl FidlResponder<Result<(), AddRouteError>>
    for fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteResponder
{
    fn try_send(self, response: Result<(), AddRouteError>) -> Result<(), fidl::Error> {
        self.send(response.map_err(Into::into))
    }
}

impl FidlResponder<Result<(), AddRouteError>>
    for fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteResponder
{
    fn try_send(self, response: Result<(), AddRouteError>) -> Result<(), fidl::Error> {
        self.send(response.map_err(Into::into))
    }
}

/// An IP generic version of
/// [`fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError`] and
/// [`fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError`].
#[derive(Debug, Clone, PartialEq)]
pub enum DelRouteError {
    InvalidAddress,
    NotFound,
}

impl From<fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError> for DelRouteError {
    fn from(error: fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError) -> Self {
        use fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError as E;
        match error {
            E::InvalidAddress => DelRouteError::InvalidAddress,
            E::NotFound => DelRouteError::NotFound,
        }
    }
}

impl From<fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError> for DelRouteError {
    fn from(error: fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError) -> Self {
        use fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError as E;
        match error {
            E::InvalidAddress => DelRouteError::InvalidAddress,
            E::NotFound => DelRouteError::NotFound,
        }
    }
}

impl From<DelRouteError> for fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError {
    fn from(error: DelRouteError) -> Self {
        use fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError as E;
        match error {
            DelRouteError::InvalidAddress => E::InvalidAddress,
            DelRouteError::NotFound => E::NotFound,
        }
    }
}

impl From<DelRouteError> for fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError {
    fn from(error: DelRouteError) -> Self {
        use fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError as E;
        match error {
            DelRouteError::InvalidAddress => E::InvalidAddress,
            DelRouteError::NotFound => E::NotFound,
        }
    }
}

impl FidlResponder<Result<(), DelRouteError>>
    for fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteResponder
{
    fn try_send(self, response: Result<(), DelRouteError>) -> Result<(), fidl::Error> {
        self.send(response.map_err(Into::into))
    }
}

impl FidlResponder<Result<(), DelRouteError>>
    for fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteResponder
{
    fn try_send(self, response: Result<(), DelRouteError>) -> Result<(), fidl::Error> {
        self.send(response.map_err(Into::into))
    }
}

/// An IP generic version of
/// [`fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError`] and
/// [`fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError`].
#[derive(Debug, Clone, PartialEq)]
pub enum GetRouteStatsError {
    InvalidAddress,
    NotFound,
}

impl From<GetRouteStatsError>
    for fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError
{
    fn from(error: GetRouteStatsError) -> Self {
        use fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError as E;
        match error {
            GetRouteStatsError::InvalidAddress => E::InvalidAddress,
            GetRouteStatsError::NotFound => E::NotFound,
        }
    }
}

impl From<fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError>
    for GetRouteStatsError
{
    fn from(error: fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError) -> Self {
        use fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError as E;
        match error {
            E::InvalidAddress => GetRouteStatsError::InvalidAddress,
            E::NotFound => GetRouteStatsError::NotFound,
        }
    }
}

impl From<GetRouteStatsError>
    for fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError
{
    fn from(error: GetRouteStatsError) -> Self {
        use fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError as E;
        match error {
            GetRouteStatsError::InvalidAddress => E::InvalidAddress,
            GetRouteStatsError::NotFound => E::NotFound,
        }
    }
}

impl From<fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError>
    for GetRouteStatsError
{
    fn from(error: fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError) -> Self {
        use fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError as E;
        match error {
            E::InvalidAddress => GetRouteStatsError::InvalidAddress,
            E::NotFound => GetRouteStatsError::NotFound,
        }
    }
}

impl FidlResponder<Result<&fnet_multicast_admin::RouteStats, GetRouteStatsError>>
    for fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsResponder
{
    fn try_send(
        self,
        response: Result<&fnet_multicast_admin::RouteStats, GetRouteStatsError>,
    ) -> Result<(), fidl::Error> {
        self.send(response.map_err(Into::into))
    }
}

impl FidlResponder<Result<&fnet_multicast_admin::RouteStats, GetRouteStatsError>>
    for fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsResponder
{
    fn try_send(
        self,
        response: Result<&fnet_multicast_admin::RouteStats, GetRouteStatsError>,
    ) -> Result<(), fidl::Error> {
        self.send(response.map_err(Into::into))
    }
}

/// An IP generic version of the fields accepted by an
/// [`fnet_multicast_admin::Ipv4RoutingTableControllerWatchRoutingEventsResponder`] and
/// [`fnet_multicast_admin::Ipv6RoutingTableControllerWatchRoutingEventsResponder`].
#[derive(Debug)]
pub struct WatchRoutingEventsResponse<I: FidlMulticastAdminIpExt> {
    pub dropped_events: u64,
    pub addresses: UnicastSourceAndMulticastDestination<I>,
    pub input_interface: u64,
    pub event: fnet_multicast_admin::RoutingEvent,
}

impl FidlResponder<WatchRoutingEventsResponse<Ipv4>>
    for fnet_multicast_admin::Ipv4RoutingTableControllerWatchRoutingEventsResponder
{
    fn try_send(self, response: WatchRoutingEventsResponse<Ipv4>) -> Result<(), fidl::Error> {
        let WatchRoutingEventsResponse { dropped_events, addresses, input_interface, event } =
            response;
        let addresses = addresses.into();
        self.send(dropped_events, &addresses, input_interface, &event)
    }
}

impl FidlResponder<WatchRoutingEventsResponse<Ipv6>>
    for fnet_multicast_admin::Ipv6RoutingTableControllerWatchRoutingEventsResponder
{
    fn try_send(self, response: WatchRoutingEventsResponse<Ipv6>) -> Result<(), fidl::Error> {
        let WatchRoutingEventsResponse { dropped_events, addresses, input_interface, event } =
            response;
        let addresses = addresses.into();
        self.send(dropped_events, &addresses, input_interface, &event)
    }
}

/// The types of errors that may occur when creating [`Route`] from FIDL.
#[derive(Debug, PartialEq)]
pub enum RouteConversionError {
    RequiredFieldMissing,
}

impl From<RouteConversionError> for AddRouteError {
    fn from(route_conversion_error: RouteConversionError) -> AddRouteError {
        match route_conversion_error {
            RouteConversionError::RequiredFieldMissing => AddRouteError::RequiredRouteFieldsMissing,
        }
    }
}

/// A sanitized `fnet_multicast_admin::Route`.
///
/// All required fields are verified to be present.
#[derive(Debug, PartialEq)]
pub struct Route {
    pub expected_input_interface: u64,
    pub action: fnet_multicast_admin::Action,
}

impl TryFrom<fnet_multicast_admin::Route> for Route {
    type Error = RouteConversionError;
    fn try_from(fidl: fnet_multicast_admin::Route) -> Result<Self, Self::Error> {
        let fnet_multicast_admin::Route { expected_input_interface, action, __source_breaking } =
            fidl;
        if let (Some(expected_input_interface), Some(action)) = (expected_input_interface, action) {
            Ok(Route { expected_input_interface, action })
        } else {
            Err(RouteConversionError::RequiredFieldMissing)
        }
    }
}

impl From<Route> for fnet_multicast_admin::Route {
    fn from(route: Route) -> fnet_multicast_admin::Route {
        let Route { expected_input_interface, action } = route;
        fnet_multicast_admin::Route {
            expected_input_interface: Some(expected_input_interface),
            action: Some(action),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use net_declare::{fidl_ip_v4, fidl_ip_v6, net_ip_v4, net_ip_v6};
    use test_case::test_case;

    #[test]
    fn ipv4_unicast_source_and_multicast_destination() {
        let fidl_addrs = fnet_multicast_admin::Ipv4UnicastSourceAndMulticastDestination {
            unicast_source: fidl_ip_v4!("192.0.2.1"),
            multicast_destination: fidl_ip_v4!("224.0.0.1"),
        };
        let addrs = UnicastSourceAndMulticastDestination::<Ipv4> {
            unicast_source: net_ip_v4!("192.0.2.1"),
            multicast_destination: net_ip_v4!("224.0.0.1"),
        };
        assert_eq!(
            fnet_multicast_admin::Ipv4UnicastSourceAndMulticastDestination::from(addrs.clone()),
            fidl_addrs
        );
        assert_eq!(UnicastSourceAndMulticastDestination::<Ipv4>::from(fidl_addrs), addrs);
    }

    #[test]
    fn ipv6_unicast_source_and_multicast_destination() {
        let fidl_addrs = fnet_multicast_admin::Ipv6UnicastSourceAndMulticastDestination {
            unicast_source: fidl_ip_v6!("2001:db8::1"),
            multicast_destination: fidl_ip_v6!("ff00::1"),
        };
        let addrs = UnicastSourceAndMulticastDestination::<Ipv6> {
            unicast_source: net_ip_v6!("2001:db8::1"),
            multicast_destination: net_ip_v6!("ff00::1"),
        };
        assert_eq!(
            fnet_multicast_admin::Ipv6UnicastSourceAndMulticastDestination::from(addrs.clone()),
            fidl_addrs
        );
        assert_eq!(UnicastSourceAndMulticastDestination::<Ipv6>::from(fidl_addrs), addrs);
    }

    #[test_case(
        AddRouteError::InvalidAddress,
        fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError::InvalidAddress;
        "invalid_address"
    )]
    #[test_case(
        AddRouteError::RequiredRouteFieldsMissing,
        fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError::RequiredRouteFieldsMissing;
        "required_route_fields_missing"
    )]
    #[test_case(
        AddRouteError::InterfaceNotFound,
        fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError::InterfaceNotFound;
        "interface_not_found"
    )]
    #[test_case(
        AddRouteError::InputCannotBeOutput,
        fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError::InputCannotBeOutput;
        "input_cannot_be_output"
    )]
    #[test_case(
        AddRouteError::DuplicateOutput,
        fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError::DuplicateOutput;
        "duplicate_output"
    )]
    fn ipv4_add_route_error(
        err: AddRouteError,
        fidl: fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError,
    ) {
        assert_eq!(
            fnet_multicast_admin::Ipv4RoutingTableControllerAddRouteError::from(err.clone()),
            fidl
        );
        assert_eq!(AddRouteError::from(fidl), err);
    }

    #[test_case(
        AddRouteError::InvalidAddress,
        fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError::InvalidAddress;
        "invalid_address"
    )]
    #[test_case(
        AddRouteError::RequiredRouteFieldsMissing,
        fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError::RequiredRouteFieldsMissing;
        "required_route_fields_missing"
    )]
    #[test_case(
        AddRouteError::InterfaceNotFound,
        fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError::InterfaceNotFound;
        "interface_not_found"
    )]
    #[test_case(
        AddRouteError::InputCannotBeOutput,
        fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError::InputCannotBeOutput;
        "input_cannot_be_output"
    )]
    #[test_case(
        AddRouteError::DuplicateOutput,
        fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError::DuplicateOutput;
        "duplicate_output"
    )]
    fn ipv6_add_route_error(
        err: AddRouteError,
        fidl: fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError,
    ) {
        assert_eq!(
            fnet_multicast_admin::Ipv6RoutingTableControllerAddRouteError::from(err.clone()),
            fidl
        );
        assert_eq!(AddRouteError::from(fidl), err);
    }

    #[test_case(
        DelRouteError::InvalidAddress,
        fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError::InvalidAddress;
        "invalid_address"
    )]
    #[test_case(
        DelRouteError::NotFound,
        fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError::NotFound;
        "not_found"
    )]
    fn ipv4_del_route_error(
        err: DelRouteError,
        fidl: fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError,
    ) {
        assert_eq!(
            fnet_multicast_admin::Ipv4RoutingTableControllerDelRouteError::from(err.clone()),
            fidl
        );
        assert_eq!(DelRouteError::from(fidl), err);
    }

    #[test_case(
        DelRouteError::InvalidAddress,
        fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError::InvalidAddress;
        "invalid_address"
    )]
    #[test_case(
        DelRouteError::NotFound,
        fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError::NotFound;
        "not_found"
    )]
    fn ipv6_del_route_error(
        err: DelRouteError,
        fidl: fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError,
    ) {
        assert_eq!(
            fnet_multicast_admin::Ipv6RoutingTableControllerDelRouteError::from(err.clone()),
            fidl
        );
        assert_eq!(DelRouteError::from(fidl), err);
    }

    #[test_case(
        GetRouteStatsError::InvalidAddress =>
        fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError::InvalidAddress;
        "invalid_address"
    )]
    #[test_case(
        GetRouteStatsError::NotFound =>
        fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError::NotFound;
        "not_found"
    )]
    fn ipv4_get_route_stats_error(
        err: GetRouteStatsError,
    ) -> fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError {
        fnet_multicast_admin::Ipv4RoutingTableControllerGetRouteStatsError::from(err)
    }

    #[test_case(
        GetRouteStatsError::InvalidAddress =>
        fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError::InvalidAddress;
        "invalid_address"
    )]
    #[test_case(
        GetRouteStatsError::NotFound =>
        fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError::NotFound;
        "not_found"
    )]
    fn ipv6_get_route_stats_error(
        err: GetRouteStatsError,
    ) -> fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError {
        fnet_multicast_admin::Ipv6RoutingTableControllerGetRouteStatsError::from(err)
    }

    #[test_case(
        fnet_multicast_admin::Route {
            expected_input_interface: Some(1),
            action: Some(fnet_multicast_admin::Action::OutgoingInterfaces(vec![])),
            __source_breaking: fidl::marker::SourceBreaking,
        },
        Ok(Route {
            expected_input_interface: 1,
            action: fnet_multicast_admin::Action::OutgoingInterfaces(vec![]),
        });
        "success"
    )]
    #[test_case(
        fnet_multicast_admin::Route {
            expected_input_interface: None,
            action: Some(fnet_multicast_admin::Action::OutgoingInterfaces(vec![])),
            __source_breaking: fidl::marker::SourceBreaking,
        },
        Err(RouteConversionError::RequiredFieldMissing);
        "missing input interface"
    )]
    #[test_case(
        fnet_multicast_admin::Route {
            expected_input_interface: Some(1),
            action: None,
            __source_breaking: fidl::marker::SourceBreaking,
        },
        Err(RouteConversionError::RequiredFieldMissing);
        "missing action"
    )]
    fn route_conversion(
        fidl: fnet_multicast_admin::Route,
        expected_result: Result<Route, RouteConversionError>,
    ) {
        assert_eq!(Route::try_from(fidl.clone()), expected_result);
        if let Ok(route) = expected_result {
            assert_eq!(fnet_multicast_admin::Route::from(route), fidl);
        }
    }
}
