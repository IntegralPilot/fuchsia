// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	"context"
	"errors"
	"fmt"
	"syscall/zx"
	"syscall/zx/fidl"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/fidlconv"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routes"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routetypes"
	"go.fuchsia.dev/fuchsia/src/lib/component"
	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"
	"gvisor.dev/gvisor/pkg/tcpip"

	fuchsianet "fidl/fuchsia/net"
	"fidl/fuchsia/net/resources"
	fnetRoutes "fidl/fuchsia/net/routes"
	routesAdmin "fidl/fuchsia/net/routes/admin"
)

const (
	routeSetV4Name  = "fuchsia.net.routes.admin/RouteSetV4"
	routeSetV6Name  = "fuchsia.net.routes.admin/RouteSetV6"
	routesAdminName = "fuchsia.net.routes.admin"
)

type UnauthenticatedError struct {
	ID tcpip.NICID
}

func (e *UnauthenticatedError) Error() string {
	return fmt.Sprintf("Interface %v was not authenticated", e.ID)
}

func (*UnauthenticatedError) Is(other error) bool {
	_, ok := other.(*UnauthenticatedError)
	return ok
}

type routeSet[A fidlconv.IpAddress] struct {
	ns                      *Netstack
	id                      *routetypes.RouteSetId
	authenticatedInterfaces map[tcpip.NICID]struct{}
}

func makeUserRouteSet[A fidlconv.IpAddress](ns *Netstack) routeSet[A] {
	return routeSet[A]{
		ns:                      ns,
		id:                      &routetypes.RouteSetId{},
		authenticatedInterfaces: make(map[tcpip.NICID]struct{}),
	}
}

func makeGlobalRouteSet[A fidlconv.IpAddress](ns *Netstack) routeSet[A] {
	return routeSet[A]{
		ns:                      ns,
		id:                      routetypes.GlobalRouteSet(),
		authenticatedInterfaces: make(map[tcpip.NICID]struct{}),
	}
}

// addRoute adds a route to the routeSet and returns whether the route is new
// to the routeSet.
func (r *routeSet[A]) addRoute(route fidlconv.Route[A]) (bool, error) {
	nicID := tcpip.NICID(route.Action.Forward.OutboundInterface)
	if _, ok := r.authenticatedInterfaces[nicID]; !ok {
		return false, &UnauthenticatedError{nicID}
	}

	gvisorRoute, err := route.GVisorRoute()
	if err != nil {
		return false, err
	}

	metric, err := func() (*routetypes.Metric, error) {
		switch route.Properties.Metric.Which() {
		case fnetRoutes.SpecifiedMetricExplicitMetric:
			metric := routetypes.Metric(route.Properties.Metric.ExplicitMetric)
			return &metric, nil
		case fnetRoutes.SpecifiedMetricInheritedFromInterface:
			return nil, nil
		default:
			return nil, fmt.Errorf("unknown metric type: %+v", route.Properties.Metric)
		}
	}()

	if err != nil {
		return false, err
	}

	addResult, err := r.ns.AddRoute(gvisorRoute, metric, false /* dynamic */, false /* replaceMatchingGvisorRoutes */, r.id)
	return addResult.NewlyAddedToSet, err
}

// removesRoute removes a route from the routeSet and returns whether the route
// was present in the routeSet.
func (r *routeSet[A]) removeRoute(route fidlconv.Route[A]) (bool, error) {
	nicID := tcpip.NICID(route.Action.Forward.OutboundInterface)
	if _, ok := r.authenticatedInterfaces[nicID]; !ok {
		return false, &UnauthenticatedError{nicID}
	}

	gvisorRoute, err := route.GVisorRoute()
	if err != nil {
		return false, err
	}

	metric, err := func() (*routetypes.Metric, error) {
		switch route.Properties.Metric.Which() {
		case fnetRoutes.SpecifiedMetricExplicitMetric:
			metric := routetypes.Metric(route.Properties.Metric.ExplicitMetric)
			return &metric, nil
		case fnetRoutes.SpecifiedMetricInheritedFromInterface:
			return nil, nil
		default:
			return nil, fmt.Errorf("unknown metric type: %+v", route.Properties.Metric)
		}
	}()

	if err != nil {
		return false, err
	}

	removeResult, err := r.ns.DelRouteExactMatch(gvisorRoute, metric, r.id)
	return removeResult.NewlyRemovedFromSet, err
}

// close deletes the routeSet in the Netstack.
func (r *routeSet[A]) close() {
	r.ns.DelRouteSet(r.id)
}

type routesAdminMainRouteTable struct {
	tableId uint32
	token   zx.Event
}

func (impl *routesAdminMainRouteTable) GetTableId(ctx_ fidl.Context) (uint32, error) {
	return impl.tableId, nil
}

func (impl *routesAdminMainRouteTable) Detach(ctx_ fidl.Context) error {
	return nil
}

func (impl *routesAdminMainRouteTable) Remove(ctx_ fidl.Context) (routesAdmin.BaseRouteTableRemoveResult, error) {
	return routesAdmin.BaseRouteTableRemoveResultWithErr(routesAdmin.BaseRouteTableRemoveErrorInvalidOp), nil
}

func (impl *routesAdminMainRouteTable) GetAuthorizationForRouteTable(ctx_ fidl.Context) (routesAdmin.GrantForRouteTableAuthorization, error) {
	token, err := impl.token.Duplicate(zx.RightTransfer | zx.RightDuplicate)
	if err != nil {
		return routesAdmin.GrantForRouteTableAuthorization{}, err
	}
	return routesAdmin.GrantForRouteTableAuthorization{
		TableId: impl.tableId,
		Token:   token,
	}, nil
}

var _ routesAdmin.BaseRouteTableWithCtx = (*routesAdminMainRouteTable)(nil)

var _ routesAdmin.RouteTableV4WithCtx = (*routesAdminRouteTableV4Impl)(nil)

type routesAdminRouteTableV4Impl struct {
	routesAdminMainRouteTable
	ns *Netstack
}

func (impl *routesAdminRouteTableV4Impl) NewRouteSet(ctx_ fidl.Context, request routesAdmin.RouteSetV4WithCtxInterfaceRequest) error {
	return bindV4RouteSet(request.Channel, makeUserRouteSet[fuchsianet.Ipv4Address](impl.ns))
}

func bindV4RouteSet(ch zx.Channel, rs routeSet[fuchsianet.Ipv4Address]) error {
	ctx, cancel := context.WithCancel(context.Background())

	go func() {
		defer cancel()

		setImpl := routeSetV4Impl{
			routeSet: rs,
		}
		defer setImpl.routeSet.close()

		component.Serve(
			ctx,
			&routesAdmin.RouteSetV4WithCtxStub{Impl: &setImpl},
			ch,
			component.ServeOptions{
				Concurrent: true,
				OnError: func(err error) {
					_ = syslog.ErrorTf(routeSetV4Name, "%s", err)
				},
			},
		)
	}()

	return nil
}

var _ routesAdmin.RouteTableV6WithCtx = (*routesAdminRouteTableV6Impl)(nil)

type routesAdminRouteTableV6Impl struct {
	routesAdminMainRouteTable
	ns *Netstack
}

func (impl *routesAdminRouteTableV6Impl) NewRouteSet(ctx_ fidl.Context, request routesAdmin.RouteSetV6WithCtxInterfaceRequest) error {
	return bindV6RouteSet(request.Channel, makeUserRouteSet[fuchsianet.Ipv6Address](impl.ns))
}

func bindV6RouteSet(ch zx.Channel, rs routeSet[fuchsianet.Ipv6Address]) error {
	ctx, cancel := context.WithCancel(context.Background())

	go func() {
		defer cancel()

		setImpl := routeSetV6Impl{
			routeSet: rs,
		}
		defer setImpl.routeSet.close()

		component.Serve(
			ctx,
			&routesAdmin.RouteSetV6WithCtxStub{Impl: &setImpl},
			ch,
			component.ServeOptions{
				Concurrent: true,
				OnError: func(err error) {
					_ = syslog.ErrorTf(routeSetV6Name, "%s", err)
				},
			},
		)
	}()

	return nil
}

func validateInterfaceCredential(ns *Netstack, clientCredential resources.ProofOfInterfaceAuthorization) bool {
	nsIfInfo, ok := ns.stack.NICInfo()[tcpip.NICID(clientCredential.InterfaceId)]
	if !ok {
		// A NIC not existing should be transformed into INVALID_AUTHENTICATION,
		// just like an invalid credential.
		return false
	}

	nsTokenInfo, err := nsIfInfo.Context.(*ifState).authorizationToken.Handle().GetInfoHandleBasic()
	if err != nil {
		panic(fmt.Sprintf("Failed to get basic info for Netstack-generated token: %s", err))
	}

	clientTokenInfo, err := clientCredential.Token.Handle().GetInfoHandleBasic()
	if err != nil {
		_ = syslog.ErrorTf(routesAdminName, "while getting handle info for credential object: %s", err)
		return false
	}

	return nsTokenInfo.Koid == clientTokenInfo.Koid
}

type routeSetV4Impl struct {
	routeSet routeSet[fuchsianet.Ipv4Address]
}

var _ routesAdmin.RouteSetV4WithCtx = (*routeSetV4Impl)(nil)

func (r *routeSetV4Impl) AddRoute(ctx_ fidl.Context, fidlRoute fnetRoutes.RouteV4) (routesAdmin.RouteSetV4AddRouteResult, error) {
	var result routesAdmin.RouteSetV4AddRouteResult
	route, validationResult := fidlconv.FromFidlRouteV4(fidlRoute)

	switch validationResult {
	case fidlconv.RouteOk:
		didAdd, err := r.routeSet.addRoute(route)
		if err != nil {
			if errors.Is(err, routes.ErrNoSuchNIC) {
				result.SetErr(routesAdmin.RouteSetErrorPreviouslyAuthenticatedInterfaceNoLongerExists)
				return result, nil
			} else if errors.Is(err, &UnauthenticatedError{}) {
				return routesAdmin.RouteSetV4AddRouteResultWithErr(routesAdmin.RouteSetErrorUnauthenticated), nil
			} else {
				_ = syslog.ErrorTf(routeSetV4Name, "error while adding route: %s", err)
			}
		}
		result.SetResponse(routesAdmin.RouteSetV4AddRouteResponse{
			DidAdd: didAdd,
		})
	case fidlconv.RouteInvalidDestinationSubnet:
		result.SetErr(routesAdmin.RouteSetErrorInvalidDestinationSubnet)
	case fidlconv.RouteInvalidNextHop:
		result.SetErr(routesAdmin.RouteSetErrorInvalidNextHop)
	case fidlconv.RouteInvalidUnknownAction:
		result.SetErr(routesAdmin.RouteSetErrorUnsupportedAction)
	case fidlconv.RouteInvalidMissingRouteProperties:
		result.SetErr(routesAdmin.RouteSetErrorMissingRouteProperties)
	case fidlconv.RouteInvalidMissingMetric:
		result.SetErr(routesAdmin.RouteSetErrorMissingMetric)
	default:
		panic(fmt.Sprintf("unrecognized RouteValidationResult: %d", validationResult))
	}

	return result, nil
}

func (r *routeSetV4Impl) AuthenticateForInterface(ctx_ fidl.Context, credential resources.ProofOfInterfaceAuthorization) (routesAdmin.RouteSetV4AuthenticateForInterfaceResult, error) {
	defer func() {
		// Close credential token to prevent resource leaks.
		if err := credential.Token.Close(); err != nil {
			_ = syslog.WarnTf(routeSetV6Name, "failed to close credential: %s", err)
		}
	}()

	var result routesAdmin.RouteSetV4AuthenticateForInterfaceResult
	if validateInterfaceCredential(r.routeSet.ns, credential) {
		r.routeSet.authenticatedInterfaces[tcpip.NICID(credential.InterfaceId)] = struct{}{}
		result.SetResponse(routesAdmin.RouteSetV4AuthenticateForInterfaceResponse{})
	} else {
		result.SetErr(routesAdmin.AuthenticateForInterfaceErrorInvalidAuthentication)
	}

	return result, nil
}

func (r *routeSetV4Impl) RemoveRoute(ctx_ fidl.Context, fidlRoute fnetRoutes.RouteV4) (routesAdmin.RouteSetV4RemoveRouteResult, error) {
	var result routesAdmin.RouteSetV4RemoveRouteResult
	route, validationResult := fidlconv.FromFidlRouteV4(fidlRoute)

	switch validationResult {
	case fidlconv.RouteOk:
		didRemove, err := r.routeSet.removeRoute(route)
		if err != nil {
			if errors.Is(err, routes.ErrNoSuchNIC) {
				result.SetErr(routesAdmin.RouteSetErrorPreviouslyAuthenticatedInterfaceNoLongerExists)
				return result, nil
			} else if errors.Is(err, &UnauthenticatedError{}) {
				return routesAdmin.RouteSetV4RemoveRouteResultWithErr(routesAdmin.RouteSetErrorUnauthenticated), nil
			} else {
				_ = syslog.ErrorTf(routeSetV4Name, "error while removing route: %s", err)
			}
		}
		result.SetResponse(routesAdmin.RouteSetV4RemoveRouteResponse{
			DidRemove: didRemove,
		})
	case fidlconv.RouteInvalidDestinationSubnet:
		result.SetErr(routesAdmin.RouteSetErrorInvalidDestinationSubnet)
	case fidlconv.RouteInvalidNextHop:
		result.SetErr(routesAdmin.RouteSetErrorInvalidNextHop)
	case fidlconv.RouteInvalidUnknownAction:
		result.SetErr(routesAdmin.RouteSetErrorUnsupportedAction)
	case fidlconv.RouteInvalidMissingRouteProperties:
		result.SetErr(routesAdmin.RouteSetErrorMissingRouteProperties)
	case fidlconv.RouteInvalidMissingMetric:
		result.SetErr(routesAdmin.RouteSetErrorMissingMetric)
	default:
		panic(fmt.Sprintf("unrecognized RouteValidationResult: %d", validationResult))
	}

	return result, nil
}

type routeSetV6Impl struct {
	routeSet routeSet[fuchsianet.Ipv6Address]
}

var _ routesAdmin.RouteSetV6WithCtx = (*routeSetV6Impl)(nil)

func (r *routeSetV6Impl) AddRoute(ctx_ fidl.Context, fidlRoute fnetRoutes.RouteV6) (routesAdmin.RouteSetV6AddRouteResult, error) {
	var result routesAdmin.RouteSetV6AddRouteResult
	route, validationResult := fidlconv.FromFidlRouteV6(fidlRoute)

	switch validationResult {
	case fidlconv.RouteOk:
		didAdd, err := r.routeSet.addRoute(route)
		if err != nil {
			if errors.Is(err, routes.ErrNoSuchNIC) {
				result.SetErr(routesAdmin.RouteSetErrorUnauthenticated)
				return result, nil
			} else if errors.Is(err, &UnauthenticatedError{}) {
				return routesAdmin.RouteSetV6AddRouteResultWithErr(routesAdmin.RouteSetErrorUnauthenticated), nil
			} else {
				_ = syslog.ErrorTf(routeSetV6Name, "error while adding route: %s", err)
			}
		}
		result.SetResponse(routesAdmin.RouteSetV6AddRouteResponse{
			DidAdd: didAdd,
		})

	case fidlconv.RouteInvalidDestinationSubnet:
		result.SetErr(routesAdmin.RouteSetErrorInvalidDestinationSubnet)
	case fidlconv.RouteInvalidNextHop:
		_ = syslog.ErrorTf(routeSetV6Name, "rejecting route %#v due to invalid next hop", route)
		result.SetErr(routesAdmin.RouteSetErrorInvalidNextHop)
	case fidlconv.RouteInvalidUnknownAction:
		result.SetErr(routesAdmin.RouteSetErrorUnsupportedAction)
	case fidlconv.RouteInvalidMissingRouteProperties:
		result.SetErr(routesAdmin.RouteSetErrorMissingRouteProperties)
	case fidlconv.RouteInvalidMissingMetric:
		result.SetErr(routesAdmin.RouteSetErrorMissingMetric)
	default:
		panic(fmt.Sprintf("unrecognized RouteValidationResult: %d", validationResult))
	}

	return result, nil
}

func (r *routeSetV6Impl) AuthenticateForInterface(ctx_ fidl.Context, credential resources.ProofOfInterfaceAuthorization) (routesAdmin.RouteSetV6AuthenticateForInterfaceResult, error) {
	defer func() {
		// Close credential token to prevent resource leaks.
		if err := credential.Token.Close(); err != nil {
			_ = syslog.WarnTf(routeSetV6Name, "failed to close credential: %s", err)
		}
	}()

	var result routesAdmin.RouteSetV6AuthenticateForInterfaceResult
	if validateInterfaceCredential(r.routeSet.ns, credential) {
		r.routeSet.authenticatedInterfaces[tcpip.NICID(credential.InterfaceId)] = struct{}{}
		result.SetResponse(routesAdmin.RouteSetV6AuthenticateForInterfaceResponse{})
	} else {
		result.SetErr(routesAdmin.AuthenticateForInterfaceErrorInvalidAuthentication)
	}

	return result, nil
}

func (r *routeSetV6Impl) RemoveRoute(ctx_ fidl.Context, fidlRoute fnetRoutes.RouteV6) (routesAdmin.RouteSetV6RemoveRouteResult, error) {
	var result routesAdmin.RouteSetV6RemoveRouteResult
	route, validationResult := fidlconv.FromFidlRouteV6(fidlRoute)

	switch validationResult {
	case fidlconv.RouteOk:
		didRemove, err := r.routeSet.removeRoute(route)
		if err != nil {
			if errors.Is(err, routes.ErrNoSuchNIC) {
				result.SetErr(routesAdmin.RouteSetErrorUnauthenticated)
				return result, nil
			} else if errors.Is(err, &UnauthenticatedError{}) {
				return routesAdmin.RouteSetV6RemoveRouteResultWithErr(routesAdmin.RouteSetErrorUnauthenticated), nil
			} else {
				_ = syslog.ErrorTf(routeSetV6Name, "error while removing route: %s", err)
			}
		}
		result.SetResponse(routesAdmin.RouteSetV6RemoveRouteResponse{
			DidRemove: didRemove,
		})
	case fidlconv.RouteInvalidDestinationSubnet:
		result.SetErr(routesAdmin.RouteSetErrorInvalidDestinationSubnet)
	case fidlconv.RouteInvalidNextHop:
		result.SetErr(routesAdmin.RouteSetErrorInvalidNextHop)
	case fidlconv.RouteInvalidUnknownAction:
		result.SetErr(routesAdmin.RouteSetErrorUnsupportedAction)
	case fidlconv.RouteInvalidMissingRouteProperties:
		result.SetErr(routesAdmin.RouteSetErrorMissingRouteProperties)
	case fidlconv.RouteInvalidMissingMetric:
		result.SetErr(routesAdmin.RouteSetErrorMissingMetric)
	default:
		panic(fmt.Sprintf("unrecognized RouteValidationResult: %d", validationResult))
	}

	return result, nil
}
