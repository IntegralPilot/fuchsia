// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extensions for fuchsia.net.routes.admin.

use std::fmt::Debug;

use flex_client::ProxyHasDomain as _;
use flex_client::fidl::{DiscoverableProtocolMarker, ProtocolMarker};
use flex_fuchsia_net_resources as fnet_resources;
use flex_fuchsia_net_root as fnet_root;
use flex_fuchsia_net_routes_admin as fnet_routes_admin;
use futures::future::Either;
use net_types::ip::{GenericOverIp, Ip, IpInvariant, Ipv4, Ipv6};
use thiserror::Error;

use crate::{FidlRouteIpExt, Responder, TableId, impl_responder};

/// Route set creation errors.
#[derive(Clone, Debug, Error)]
pub enum RouteSetCreationError {
    /// Proxy creation failed.
    #[error("failed to create proxy: {0}")]
    CreateProxy(fidl::Error),
    /// Route set creation failed.
    #[error("failed to create route set: {0}")]
    RouteSet(fidl::Error),
}

/// Route table creation errors.
#[derive(Clone, Debug, Error)]
pub enum RouteTableCreationError {
    /// Proxy creation failed.
    #[error("failed to create proxy: {0}")]
    CreateProxy(fidl::Error),
    /// Route table creation failed.
    #[error("failed to create route set: {0}")]
    RouteTable(fidl::Error),
}

/// Admin extension for the `fuchsia.net.routes.admin` FIDL API.
pub trait FidlRouteAdminIpExt: Ip {
    /// The "route table" protocol to use for this IP version.
    type RouteTableMarker: DiscoverableProtocolMarker<
            RequestStream = Self::RouteTableRequestStream,
            Proxy: Clone + Debug,
        > + Unpin
        + std::fmt::Debug;
    /// The "root set" protocol to use for this IP version.
    type GlobalRouteTableMarker: DiscoverableProtocolMarker;
    /// The "route set" protocol to use for this IP version.
    type RouteSetMarker: ProtocolMarker<RequestStream = Self::RouteSetRequestStream>
        + std::fmt::Debug;
    /// The "route table provider" protocol to use for this IP version.
    type RouteTableProviderMarker: DiscoverableProtocolMarker<Proxy: Clone>;
    /// The request stream for the route set protocol.
    type RouteSetRequestStream: flex_client::fidl::RequestStream<Ok: Send, ControlHandle: Send>;
    /// The request stream for the route table protocol.
    type RouteTableRequestStream: flex_client::fidl::RequestStream<Ok: Send, ControlHandle: Send>;
    /// The responder for AddRoute requests.
    type AddRouteResponder: Responder<Payload = Result<bool, fnet_routes_admin::RouteSetError>>;
    /// The responder for RemoveRoute requests.
    type RemoveRouteResponder: Responder<Payload = Result<bool, fnet_routes_admin::RouteSetError>>;
    /// The responder for AuthenticateForInterface requests.
    type RouteSetAuthenticateForInterfaceResponder: Responder<
        Payload = Result<(), fnet_routes_admin::AuthenticateForInterfaceError>,
    >;
    /// The responder for GetTableId requests.
    type RouteTableGetTableIdResponder: Responder<Payload = u32>;
    /// The responder for RemoveRequests.
    type RouteTableRemoveResponder: Responder<
        Payload = Result<(), fnet_routes_admin::BaseRouteTableRemoveError>,
    >;
    /// The responder for GetAuthorizationForRouteTable requests.
    type RouteTableGetAuthorizationResponder: Responder<
        Payload = fnet_routes_admin::GrantForRouteTableAuthorization,
    >;
    /// The control handle for RouteTable protocols.
    type RouteTableControlHandle: flex_client::fidl::ControlHandle + Debug;

    /// The control handle for RouteTableProvider protocols.
    type RouteTableProviderControlHandle: flex_client::fidl::ControlHandle + Debug;

    /// The responder for the GetInterfaceLocalTable method.
    type GetInterfaceLocalTableResponder: Responder<
        Payload = Result<
            flex_client::fidl::ClientEnd<Self::RouteTableMarker>,
            fnet_routes_admin::GetInterfaceLocalTableError,
        >,
    >;

    /// Turns a FIDL route table provider request into the extension type.
    fn into_route_table_provider_request(
        request: flex_client::fidl::Request<Self::RouteTableProviderMarker>,
    ) -> RouteTableProviderRequest<Self>;

    /// Turns a FIDL route set request into the extension type.
    fn into_route_set_request(
        request: flex_client::fidl::Request<Self::RouteSetMarker>,
    ) -> RouteSetRequest<Self>;

    /// Turns a FIDL route table request into the extension type.
    fn into_route_table_request(
        request: flex_client::fidl::Request<Self::RouteTableMarker>,
    ) -> RouteTableRequest<Self>;

    /// Turns a FIDL route set request stream item into a Result of the extension type.
    fn into_route_set_request_result(
        request: <Self::RouteSetRequestStream as futures::Stream>::Item,
    ) -> Result<RouteSetRequest<Self>, fidl::Error>;

    /// Turns a FIDL route table request stream item into a Result of the extension type.
    fn into_route_table_request_result(
        request: <Self::RouteTableRequestStream as futures::Stream>::Item,
    ) -> Result<RouteTableRequest<Self>, fidl::Error>;
}

impl FidlRouteAdminIpExt for Ipv4 {
    type RouteTableMarker = fnet_routes_admin::RouteTableV4Marker;
    type GlobalRouteTableMarker = fnet_root::RoutesV4Marker;
    type RouteSetMarker = fnet_routes_admin::RouteSetV4Marker;
    type RouteTableProviderMarker = fnet_routes_admin::RouteTableProviderV4Marker;
    type RouteSetRequestStream = fnet_routes_admin::RouteSetV4RequestStream;
    type RouteTableRequestStream = fnet_routes_admin::RouteTableV4RequestStream;
    type AddRouteResponder = fnet_routes_admin::RouteSetV4AddRouteResponder;
    type RemoveRouteResponder = fnet_routes_admin::RouteSetV4RemoveRouteResponder;
    type RouteSetAuthenticateForInterfaceResponder =
        fnet_routes_admin::RouteSetV4AuthenticateForInterfaceResponder;
    type RouteTableGetTableIdResponder = fnet_routes_admin::RouteTableV4GetTableIdResponder;
    type RouteTableRemoveResponder = fnet_routes_admin::RouteTableV4RemoveResponder;
    type RouteTableGetAuthorizationResponder =
        fnet_routes_admin::RouteTableV4GetAuthorizationForRouteTableResponder;
    type RouteTableControlHandle = fnet_routes_admin::RouteTableV4ControlHandle;
    type RouteTableProviderControlHandle = fnet_routes_admin::RouteTableProviderV4ControlHandle;
    type GetInterfaceLocalTableResponder =
        fnet_routes_admin::RouteTableProviderV4GetInterfaceLocalTableResponder;

    fn into_route_table_provider_request(
        request: flex_client::fidl::Request<Self::RouteTableProviderMarker>,
    ) -> RouteTableProviderRequest<Ipv4> {
        RouteTableProviderRequest::from(request)
    }

    fn into_route_set_request(
        request: flex_client::fidl::Request<Self::RouteSetMarker>,
    ) -> RouteSetRequest<Self> {
        RouteSetRequest::from(request)
    }

    fn into_route_table_request(
        request: flex_client::fidl::Request<Self::RouteTableMarker>,
    ) -> RouteTableRequest<Self> {
        RouteTableRequest::from(request)
    }

    fn into_route_set_request_result(
        request: <Self::RouteSetRequestStream as futures::Stream>::Item,
    ) -> Result<RouteSetRequest<Self>, fidl::Error> {
        request.map(RouteSetRequest::from)
    }

    fn into_route_table_request_result(
        request: <Self::RouteTableRequestStream as futures::Stream>::Item,
    ) -> Result<RouteTableRequest<Self>, fidl::Error> {
        request.map(RouteTableRequest::from)
    }
}

impl FidlRouteAdminIpExt for Ipv6 {
    type RouteTableMarker = fnet_routes_admin::RouteTableV6Marker;
    type GlobalRouteTableMarker = fnet_root::RoutesV6Marker;
    type RouteSetMarker = fnet_routes_admin::RouteSetV6Marker;
    type RouteTableProviderMarker = fnet_routes_admin::RouteTableProviderV6Marker;
    type RouteSetRequestStream = fnet_routes_admin::RouteSetV6RequestStream;
    type RouteTableRequestStream = fnet_routes_admin::RouteTableV6RequestStream;
    type AddRouteResponder = fnet_routes_admin::RouteSetV6AddRouteResponder;
    type RemoveRouteResponder = fnet_routes_admin::RouteSetV6RemoveRouteResponder;
    type RouteSetAuthenticateForInterfaceResponder =
        fnet_routes_admin::RouteSetV6AuthenticateForInterfaceResponder;
    type RouteTableGetTableIdResponder = fnet_routes_admin::RouteTableV6GetTableIdResponder;
    type RouteTableRemoveResponder = fnet_routes_admin::RouteTableV6RemoveResponder;
    type RouteTableGetAuthorizationResponder =
        fnet_routes_admin::RouteTableV6GetAuthorizationForRouteTableResponder;
    type RouteTableControlHandle = fnet_routes_admin::RouteTableV6ControlHandle;
    type RouteTableProviderControlHandle = fnet_routes_admin::RouteTableProviderV6ControlHandle;
    type GetInterfaceLocalTableResponder =
        fnet_routes_admin::RouteTableProviderV6GetInterfaceLocalTableResponder;

    fn into_route_table_provider_request(
        request: flex_client::fidl::Request<Self::RouteTableProviderMarker>,
    ) -> RouteTableProviderRequest<Ipv6> {
        RouteTableProviderRequest::from(request)
    }

    fn into_route_set_request(
        request: flex_client::fidl::Request<Self::RouteSetMarker>,
    ) -> RouteSetRequest<Self> {
        RouteSetRequest::from(request)
    }

    fn into_route_table_request(
        request: flex_client::fidl::Request<Self::RouteTableMarker>,
    ) -> RouteTableRequest<Self> {
        RouteTableRequest::from(request)
    }

    fn into_route_set_request_result(
        request: <Self::RouteSetRequestStream as futures::Stream>::Item,
    ) -> Result<RouteSetRequest<Self>, fidl::Error> {
        request.map(RouteSetRequest::from)
    }

    fn into_route_table_request_result(
        request: <Self::RouteTableRequestStream as futures::Stream>::Item,
    ) -> Result<RouteTableRequest<Self>, fidl::Error> {
        request.map(RouteTableRequest::from)
    }
}

impl_responder!(
    fnet_routes_admin::RouteSetV4AddRouteResponder,
    Result<bool, fnet_routes_admin::RouteSetError>,
);
impl_responder!(
    fnet_routes_admin::RouteSetV4RemoveRouteResponder,
    Result<bool, fnet_routes_admin::RouteSetError>,
);
impl_responder!(
    fnet_routes_admin::RouteSetV6AddRouteResponder,
    Result<bool, fnet_routes_admin::RouteSetError>,
);
impl_responder!(
    fnet_routes_admin::RouteSetV6RemoveRouteResponder,
    Result<bool, fnet_routes_admin::RouteSetError>,
);
impl_responder!(
    fnet_routes_admin::RouteSetV4AuthenticateForInterfaceResponder,
    Result<(), fnet_routes_admin::AuthenticateForInterfaceError>,
);
impl_responder!(
    fnet_routes_admin::RouteSetV6AuthenticateForInterfaceResponder,
    Result<(), fnet_routes_admin::AuthenticateForInterfaceError>,
);
impl_responder!(fnet_routes_admin::RouteTableV4GetTableIdResponder, u32,);
impl_responder!(fnet_routes_admin::RouteTableV6GetTableIdResponder, u32,);
impl_responder!(
    fnet_routes_admin::RouteTableV4RemoveResponder,
    Result<(), fnet_routes_admin::BaseRouteTableRemoveError>,
);
impl_responder!(
    fnet_routes_admin::RouteTableV6RemoveResponder,
    Result<(), fnet_routes_admin::BaseRouteTableRemoveError>,
);
impl_responder!(
    fnet_routes_admin::RouteTableV4GetAuthorizationForRouteTableResponder,
    fnet_routes_admin::GrantForRouteTableAuthorization,
);
impl_responder!(
    fnet_routes_admin::RouteTableV6GetAuthorizationForRouteTableResponder,
    fnet_routes_admin::GrantForRouteTableAuthorization,
);
impl_responder!(
    fnet_routes_admin::RouteTableProviderV4GetInterfaceLocalTableResponder,
    Result<
        flex_client::fidl::ClientEnd<fnet_routes_admin::RouteTableV4Marker>,
        fnet_routes_admin::GetInterfaceLocalTableError,
    >,
);
impl_responder!(
    fnet_routes_admin::RouteTableProviderV6GetInterfaceLocalTableResponder,
    Result<
        flex_client::fidl::ClientEnd<fnet_routes_admin::RouteTableV6Marker>,
        fnet_routes_admin::GetInterfaceLocalTableError,
    >,
);

/// Options for creating route tables.
#[derive(Clone, Debug, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct RouteTableOptions<I: Ip> {
    /// Optional name for the table.
    pub name: Option<String>,
    /// Marker for the IP version.
    pub _marker: std::marker::PhantomData<I>,
}

impl From<RouteTableOptions<Ipv4>> for fnet_routes_admin::RouteTableOptionsV4 {
    fn from(val: RouteTableOptions<Ipv4>) -> Self {
        let RouteTableOptions { name, _marker } = val;
        Self { name, __source_breaking: fidl::marker::SourceBreaking }
    }
}

impl From<RouteTableOptions<Ipv6>> for fnet_routes_admin::RouteTableOptionsV6 {
    fn from(val: RouteTableOptions<Ipv6>) -> Self {
        let RouteTableOptions { name, _marker } = val;
        Self { name, __source_breaking: fidl::marker::SourceBreaking }
    }
}

impl From<fnet_routes_admin::RouteTableOptionsV4> for RouteTableOptions<Ipv4> {
    fn from(val: fnet_routes_admin::RouteTableOptionsV4) -> Self {
        let fnet_routes_admin::RouteTableOptionsV4 { name, __source_breaking: _ } = val;
        Self { name, _marker: std::marker::PhantomData }
    }
}

impl From<fnet_routes_admin::RouteTableOptionsV6> for RouteTableOptions<Ipv6> {
    fn from(val: fnet_routes_admin::RouteTableOptionsV6) -> Self {
        let fnet_routes_admin::RouteTableOptionsV6 { name, __source_breaking: _ } = val;
        Self { name, _marker: std::marker::PhantomData }
    }
}

/// GenericOverIp version of RouteTableProviderV{4, 6}Request.
#[derive(derivative::Derivative, GenericOverIp)]
#[derivative(Debug(bound = ""))]
#[generic_over_ip(I, Ip)]
pub enum RouteTableProviderRequest<I: Ip + FidlRouteAdminIpExt> {
    /// Request to create a new route table.
    NewRouteTable {
        /// The server end of the RouteTable request
        provider: flex_client::fidl::ServerEnd<I::RouteTableMarker>,
        /// The creation options.
        options: RouteTableOptions<I>,
        /// The control handle for the protocol.
        control_handle: I::RouteTableProviderControlHandle,
    },
    /// Request to get the interface-local route table.
    GetInterfaceLocalTable {
        /// The credentials for the interface.
        credential: fnet_resources::ProofOfInterfaceAuthorization,
        /// Responder to return the local table.
        responder: I::GetInterfaceLocalTableResponder,
    },
}

impl From<fnet_routes_admin::RouteTableProviderV4Request> for RouteTableProviderRequest<Ipv4> {
    fn from(val: fnet_routes_admin::RouteTableProviderV4Request) -> Self {
        match val {
            fnet_routes_admin::RouteTableProviderV4Request::NewRouteTable {
                provider,
                options,
                control_handle,
            } => Self::NewRouteTable { provider, options: options.into(), control_handle },
            fnet_routes_admin::RouteTableProviderV4Request::GetInterfaceLocalTable {
                credential,
                responder,
            } => Self::GetInterfaceLocalTable { credential, responder },
        }
    }
}

impl From<fnet_routes_admin::RouteTableProviderV6Request> for RouteTableProviderRequest<Ipv6> {
    fn from(val: fnet_routes_admin::RouteTableProviderV6Request) -> Self {
        match val {
            fnet_routes_admin::RouteTableProviderV6Request::NewRouteTable {
                provider,
                options,
                control_handle,
            } => Self::NewRouteTable { provider, options: options.into(), control_handle },
            fnet_routes_admin::RouteTableProviderV6Request::GetInterfaceLocalTable {
                credential,
                responder,
            } => Self::GetInterfaceLocalTable { credential, responder },
        }
    }
}

/// Dispatches `new_route_table` on either the `RouteTableProviderV4`
/// or `RouteTableProviderV6` proxy.
pub fn new_route_table<I: Ip + FidlRouteAdminIpExt>(
    route_table_provider_proxy: &<I::RouteTableProviderMarker as ProtocolMarker>::Proxy,
    name: Option<String>,
) -> Result<<I::RouteTableMarker as ProtocolMarker>::Proxy, RouteTableCreationError> {
    let (route_table_proxy, route_table_server_end) =
        route_table_provider_proxy.domain().create_proxy::<I::RouteTableMarker>();

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct NewRouteTableInput<'a, I: FidlRouteAdminIpExt> {
        route_table_server_end: flex_client::fidl::ServerEnd<I::RouteTableMarker>,
        route_table_provider_proxy: &'a <I::RouteTableProviderMarker as ProtocolMarker>::Proxy,
        name: Option<String>,
    }

    let result = I::map_ip_in(
        NewRouteTableInput::<'_, I> { route_table_server_end, route_table_provider_proxy, name },
        |NewRouteTableInput { route_table_server_end, route_table_provider_proxy, name }| {
            route_table_provider_proxy.new_route_table(
                route_table_server_end,
                &fnet_routes_admin::RouteTableOptionsV4 {
                    name,
                    ..fnet_routes_admin::RouteTableOptionsV4::default()
                },
            )
        },
        |NewRouteTableInput { route_table_server_end, route_table_provider_proxy, name }| {
            route_table_provider_proxy.new_route_table(
                route_table_server_end,
                &fnet_routes_admin::RouteTableOptionsV6 {
                    name,
                    ..fnet_routes_admin::RouteTableOptionsV6::default()
                },
            )
        },
    );

    result.map_err(RouteTableCreationError::RouteTable)?;
    Ok(route_table_proxy)
}

/// Dispatches `get_interface_local_table` on either the `RouteTableProviderV4`
/// or `RouteTableProviderV6` proxy.
pub async fn get_interface_local_table<I: Ip + FidlRouteAdminIpExt>(
    route_table_provider_proxy: &<I::RouteTableProviderMarker as ProtocolMarker>::Proxy,
    credential: fnet_resources::ProofOfInterfaceAuthorization,
) -> Result<
    Result<
        <I::RouteTableMarker as ProtocolMarker>::Proxy,
        fnet_routes_admin::GetInterfaceLocalTableError,
    >,
    fidl::Error,
> {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct Input<'a, I: FidlRouteAdminIpExt> {
        route_table_provider_proxy: &'a <I::RouteTableProviderMarker as ProtocolMarker>::Proxy,
        credential: fnet_resources::ProofOfInterfaceAuthorization,
    }

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct Output<I: FidlRouteAdminIpExt>(
        fidl::client::QueryResponseFut<
            Result<
                flex_client::fidl::ClientEnd<I::RouteTableMarker>,
                fnet_routes_admin::GetInterfaceLocalTableError,
            >,
            flex_client::Dialect,
        >,
    );

    let Output(fut) = I::map_ip::<Input<'_, I>, Output<I>>(
        Input::<'_, I> { route_table_provider_proxy, credential },
        |Input { route_table_provider_proxy, credential }| {
            Output::<Ipv4>(route_table_provider_proxy.get_interface_local_table(credential))
        },
        |Input { route_table_provider_proxy, credential }| {
            Output::<Ipv6>(route_table_provider_proxy.get_interface_local_table(credential))
        },
    );

    fut.await.map(|r| r.map(|client_end| client_end.into_proxy()))
}

/// Dispatches `new_route_set` on either the `RouteTableV4`
/// or `RouteTableV6` proxy.
pub fn new_route_set<I: Ip + FidlRouteAdminIpExt>(
    route_table_proxy: &<I::RouteTableMarker as ProtocolMarker>::Proxy,
) -> Result<<I::RouteSetMarker as ProtocolMarker>::Proxy, RouteSetCreationError> {
    let (route_set_proxy, route_set_server_end) =
        route_table_proxy.domain().create_proxy::<I::RouteSetMarker>();

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct NewRouteSetInput<'a, I: FidlRouteAdminIpExt> {
        route_set_server_end: flex_client::fidl::ServerEnd<I::RouteSetMarker>,
        route_table_proxy: &'a <I::RouteTableMarker as ProtocolMarker>::Proxy,
    }
    let result = I::map_ip_in(
        NewRouteSetInput::<'_, I> { route_set_server_end, route_table_proxy },
        |NewRouteSetInput { route_set_server_end, route_table_proxy }| {
            route_table_proxy.new_route_set(route_set_server_end)
        },
        |NewRouteSetInput { route_set_server_end, route_table_proxy }| {
            route_table_proxy.new_route_set(route_set_server_end)
        },
    );

    result.map_err(RouteSetCreationError::RouteSet)?;
    Ok(route_set_proxy)
}

/// Dispatches `global_route_set` on either the `RoutesV4` or `RoutesV6` in
/// fuchsia.net.root.
pub fn new_global_route_set<I: Ip + FidlRouteAdminIpExt>(
    route_table_proxy: &<I::GlobalRouteTableMarker as ProtocolMarker>::Proxy,
) -> Result<<I::RouteSetMarker as ProtocolMarker>::Proxy, RouteSetCreationError> {
    let (route_set_proxy, route_set_server_end) =
        route_table_proxy.domain().create_proxy::<I::RouteSetMarker>();

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct NewRouteSetInput<'a, I: FidlRouteAdminIpExt> {
        route_set_server_end: flex_client::fidl::ServerEnd<I::RouteSetMarker>,
        route_table_proxy: &'a <I::GlobalRouteTableMarker as ProtocolMarker>::Proxy,
    }
    let result = I::map_ip_in(
        NewRouteSetInput::<'_, I> { route_set_server_end, route_table_proxy },
        |NewRouteSetInput { route_set_server_end, route_table_proxy }| {
            route_table_proxy.global_route_set(route_set_server_end)
        },
        |NewRouteSetInput { route_set_server_end, route_table_proxy }| {
            route_table_proxy.global_route_set(route_set_server_end)
        },
    );

    result.map_err(RouteSetCreationError::RouteSet)?;
    Ok(route_set_proxy)
}

/// Dispatches `add_route` on either the `RouteSetV4` or `RouteSetV6` proxy.
pub async fn add_route<I: Ip + FidlRouteAdminIpExt + FidlRouteIpExt>(
    route_set: &<I::RouteSetMarker as ProtocolMarker>::Proxy,
    route: &I::Route,
) -> Result<Result<bool, fnet_routes_admin::RouteSetError>, fidl::Error> {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct AddRouteInput<'a, I: FidlRouteAdminIpExt + FidlRouteIpExt> {
        route_set: &'a <I::RouteSetMarker as ProtocolMarker>::Proxy,
        route: &'a I::Route,
    }

    I::map_ip_in(
        AddRouteInput { route_set, route },
        |AddRouteInput { route_set, route }| Either::Left(route_set.add_route(route)),
        |AddRouteInput { route_set, route }| Either::Right(route_set.add_route(route)),
    )
    .await
}

/// Dispatches `remove_route` on either the `RouteSetV4` or `RouteSetV6` proxy.
pub async fn remove_route<I: Ip + FidlRouteAdminIpExt + FidlRouteIpExt>(
    route_set: &<I::RouteSetMarker as ProtocolMarker>::Proxy,
    route: &I::Route,
) -> Result<Result<bool, fnet_routes_admin::RouteSetError>, fidl::Error> {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct RemoveRouteInput<'a, I: FidlRouteAdminIpExt + FidlRouteIpExt> {
        route_set: &'a <I::RouteSetMarker as ProtocolMarker>::Proxy,
        route: &'a I::Route,
    }

    I::map_ip_in(
        RemoveRouteInput { route_set, route },
        |RemoveRouteInput { route_set, route }| Either::Left(route_set.remove_route(route)),
        |RemoveRouteInput { route_set, route }| Either::Right(route_set.remove_route(route)),
    )
    .await
}

/// Dispatches `authenticate_for_interface` on either the `RouteSetV4` or
/// `RouteSetV6` proxy.
pub async fn authenticate_for_interface<I: Ip + FidlRouteAdminIpExt + FidlRouteIpExt>(
    route_set: &<I::RouteSetMarker as ProtocolMarker>::Proxy,
    credential: fnet_resources::ProofOfInterfaceAuthorization,
) -> Result<Result<(), fnet_routes_admin::AuthenticateForInterfaceError>, fidl::Error> {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct AuthenticateForInterfaceInput<'a, I: FidlRouteAdminIpExt + FidlRouteIpExt> {
        route_set: &'a <I::RouteSetMarker as ProtocolMarker>::Proxy,
        credential: fnet_resources::ProofOfInterfaceAuthorization,
    }

    I::map_ip_in(
        AuthenticateForInterfaceInput { route_set, credential },
        |AuthenticateForInterfaceInput { route_set, credential }| {
            Either::Left(route_set.authenticate_for_interface(credential))
        },
        |AuthenticateForInterfaceInput { route_set, credential }| {
            Either::Right(route_set.authenticate_for_interface(credential))
        },
    )
    .await
}

#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
struct RouteTableProxy<'a, I: FidlRouteAdminIpExt + FidlRouteIpExt> {
    route_table: &'a <I::RouteTableMarker as ProtocolMarker>::Proxy,
}

/// Dispatches `detach` on either the `RouteTableV4` or `RouteTableV6` proxy.
pub async fn detach_route_table<I: Ip + FidlRouteAdminIpExt + FidlRouteIpExt>(
    route_table: &<I::RouteTableMarker as ProtocolMarker>::Proxy,
) -> Result<(), fidl::Error> {
    let IpInvariant(result) = net_types::map_ip_twice!(
        I,
        RouteTableProxy { route_table },
        |RouteTableProxy { route_table }| { IpInvariant(route_table.detach()) }
    );

    result
}

/// Dispatches `remove` on either the `RouteTableV4` or `RouteTableV6` proxy.
pub async fn remove_route_table<I: Ip + FidlRouteAdminIpExt + FidlRouteIpExt>(
    route_table: &<I::RouteTableMarker as ProtocolMarker>::Proxy,
) -> Result<Result<(), fnet_routes_admin::BaseRouteTableRemoveError>, fidl::Error> {
    let IpInvariant(result_fut) = net_types::map_ip_twice!(
        I,
        RouteTableProxy { route_table },
        |RouteTableProxy { route_table }| { IpInvariant(route_table.remove()) }
    );

    result_fut.await
}

/// Dispatches `get_table_id` on either the `RouteTableV4` or `RouteTableV6`
/// proxy.
pub async fn get_table_id<I: Ip + FidlRouteAdminIpExt + FidlRouteIpExt>(
    route_table: &<I::RouteTableMarker as ProtocolMarker>::Proxy,
) -> Result<TableId, fidl::Error> {
    let IpInvariant(result_fut) = net_types::map_ip_twice!(
        I,
        RouteTableProxy { route_table },
        |RouteTableProxy { route_table }| IpInvariant(route_table.get_table_id()),
    );

    result_fut.await.map(TableId::new)
}

/// Dispatches `get_authorization_for_route_table` on either the `RouteTableV4`
/// or `RouteTableV6` proxy.
pub async fn get_authorization_for_route_table<I: Ip + FidlRouteAdminIpExt + FidlRouteIpExt>(
    route_table: &<I::RouteTableMarker as ProtocolMarker>::Proxy,
) -> Result<fnet_routes_admin::GrantForRouteTableAuthorization, fidl::Error> {
    let IpInvariant(result_fut) = net_types::map_ip_twice!(
        I,
        RouteTableProxy { route_table },
        |RouteTableProxy { route_table }| IpInvariant(
            route_table.get_authorization_for_route_table()
        ),
    );

    result_fut.await
}

/// GenericOverIp version of RouteSetV{4, 6}Request.
#[derive(GenericOverIp, Debug)]
#[generic_over_ip(I, Ip)]
pub enum RouteSetRequest<I: FidlRouteAdminIpExt> {
    /// Adds a route to the route set.
    AddRoute {
        /// The route to add.
        route: Result<
            crate::Route<I>,
            crate::FidlConversionError<crate::RoutePropertiesRequiredFields>,
        >,
        /// The responder for this request.
        responder: I::AddRouteResponder,
    },
    /// Removes a route from the route set.
    RemoveRoute {
        /// The route to add.
        route: Result<
            crate::Route<I>,
            crate::FidlConversionError<crate::RoutePropertiesRequiredFields>,
        >,
        /// The responder for this request.
        responder: I::RemoveRouteResponder,
    },
    /// Authenticates the route set for managing routes on an interface.
    AuthenticateForInterface {
        /// The credential proving authorization for this interface.
        credential: fnet_resources::ProofOfInterfaceAuthorization,
        /// The responder for this request.
        responder: I::RouteSetAuthenticateForInterfaceResponder,
    },
}

impl From<fnet_routes_admin::RouteSetV4Request> for RouteSetRequest<Ipv4> {
    fn from(value: fnet_routes_admin::RouteSetV4Request) -> Self {
        match value {
            fnet_routes_admin::RouteSetV4Request::AddRoute { route, responder } => {
                RouteSetRequest::AddRoute { route: route.try_into(), responder }
            }
            fnet_routes_admin::RouteSetV4Request::RemoveRoute { route, responder } => {
                RouteSetRequest::RemoveRoute { route: route.try_into(), responder }
            }
            fnet_routes_admin::RouteSetV4Request::AuthenticateForInterface {
                credential,
                responder,
            } => RouteSetRequest::AuthenticateForInterface { credential, responder },
        }
    }
}

impl From<fnet_routes_admin::RouteSetV6Request> for RouteSetRequest<Ipv6> {
    fn from(value: fnet_routes_admin::RouteSetV6Request) -> Self {
        match value {
            fnet_routes_admin::RouteSetV6Request::AddRoute { route, responder } => {
                RouteSetRequest::AddRoute { route: route.try_into(), responder }
            }
            fnet_routes_admin::RouteSetV6Request::RemoveRoute { route, responder } => {
                RouteSetRequest::RemoveRoute { route: route.try_into(), responder }
            }
            fnet_routes_admin::RouteSetV6Request::AuthenticateForInterface {
                credential,
                responder,
            } => RouteSetRequest::AuthenticateForInterface { credential, responder },
        }
    }
}

/// GenericOverIp version of RouteTableV{4, 6}Request.
#[derive(GenericOverIp, derivative::Derivative)]
#[derivative(Debug(bound = ""))]
#[generic_over_ip(I, Ip)]
pub enum RouteTableRequest<I: FidlRouteAdminIpExt> {
    /// Gets the table ID for the table
    GetTableId {
        /// Responder for the request.
        responder: I::RouteTableGetTableIdResponder,
    },
    /// Detaches the table lifetime from the channel.
    Detach {
        /// Control handle to the protocol.
        control_handle: I::RouteTableControlHandle,
    },
    /// Removes the table.
    Remove {
        /// Responder to the request.
        responder: I::RouteTableRemoveResponder,
    },
    /// Gets the authorization for the route table.
    GetAuthorizationForRouteTable {
        /// Responder to the request.
        responder: I::RouteTableGetAuthorizationResponder,
    },
    /// Creates a new route set for the table.
    NewRouteSet {
        /// The server end of the route set protocol.
        route_set: flex_client::fidl::ServerEnd<I::RouteSetMarker>,
        /// Control handle to the protocol.
        control_handle: I::RouteTableControlHandle,
    },
}

impl From<fnet_routes_admin::RouteTableV4Request> for RouteTableRequest<Ipv4> {
    fn from(value: fnet_routes_admin::RouteTableV4Request) -> Self {
        match value {
            fnet_routes_admin::RouteTableV4Request::NewRouteSet { route_set, control_handle } => {
                RouteTableRequest::NewRouteSet { route_set, control_handle }
            }
            fnet_routes_admin::RouteTableV4Request::GetTableId { responder } => {
                RouteTableRequest::GetTableId { responder }
            }

            fnet_routes_admin::RouteTableV4Request::Detach { control_handle } => {
                RouteTableRequest::Detach { control_handle }
            }

            fnet_routes_admin::RouteTableV4Request::Remove { responder } => {
                RouteTableRequest::Remove { responder }
            }
            fnet_routes_admin::RouteTableV4Request::GetAuthorizationForRouteTable { responder } => {
                RouteTableRequest::GetAuthorizationForRouteTable { responder }
            }
        }
    }
}

impl From<fnet_routes_admin::RouteTableV6Request> for RouteTableRequest<Ipv6> {
    fn from(value: fnet_routes_admin::RouteTableV6Request) -> Self {
        match value {
            fnet_routes_admin::RouteTableV6Request::NewRouteSet { route_set, control_handle } => {
                RouteTableRequest::NewRouteSet { route_set, control_handle }
            }
            fnet_routes_admin::RouteTableV6Request::GetTableId { responder } => {
                RouteTableRequest::GetTableId { responder }
            }

            fnet_routes_admin::RouteTableV6Request::Detach { control_handle } => {
                RouteTableRequest::Detach { control_handle }
            }

            fnet_routes_admin::RouteTableV6Request::Remove { responder } => {
                RouteTableRequest::Remove { responder }
            }
            fnet_routes_admin::RouteTableV6Request::GetAuthorizationForRouteTable { responder } => {
                RouteTableRequest::GetAuthorizationForRouteTable { responder }
            }
        }
    }
}
