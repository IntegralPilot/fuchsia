// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlconv

import (
	"errors"
	"fmt"

	fuchsianet "fidl/fuchsia/net"
	fidlRoutes "fidl/fuchsia/net/routes"

	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/header"
)

const (
	V4MainRouteTableId = 0
	V6MainRouteTableId = 1
)

// Route must have exclusively value types in order to be used as a map key with
// the expected semantics (no pointers).
type Route[A IpAddress] struct {
	Destination IpAddressWithPrefix[A]
	Action      RouteAction[A]
	Properties  SpecifiedRouteProperties
}

func (route *Route[A]) GVisorRoute() (tcpip.Route, error) {
	switch route.Action.RouteActionType {
	case RouteActionTypeForward:
	case RouteActionTypeUnknown:
		fallthrough
	default:
		return tcpip.Route{}, fmt.Errorf("unknown RouteActionType: %d", route.Action.RouteActionType)
	}

	destFidlSubnet := route.Destination.ToSubnet()
	dest, err := ToTCPIPSubnetChecked(destFidlSubnet)
	if err != nil {
		return tcpip.Route{}, fmt.Errorf("ToTCPIPSubnetChecked(%#v) = %w", destFidlSubnet, err)
	}

	return tcpip.Route{
		Destination: dest,
		Gateway: func() tcpip.Address {
			if route.Action.Forward.NextHopPresent {
				return ToTCPIPAddress(toFidlIpAddress(route.Action.Forward.NextHop))
			}
			return tcpip.Address{}
		}(),
		NIC: tcpip.NICID(route.Action.Forward.OutboundInterface),
	}, nil
}

func (route *Route[A]) Validate() RouteValidationResult {
	if !validateSubnet(route.Destination.ToSubnet()) {
		return RouteInvalidDestinationSubnet
	}

	switch route.Action.RouteActionType {
	case RouteActionTypeForward:
	case RouteActionTypeUnknown:
		fallthrough
	default:
		return RouteInvalidUnknownAction
	}

	if route.Action.Forward.NextHopPresent {
		addr := ToTCPIPAddress(toFidlIpAddress(route.Action.Forward.NextHop))

		switch addr.Len() {
		case header.IPv4AddressSize:
			if header.IsV4MulticastAddress(addr) || addr == header.IPv4Broadcast || addr.Unspecified() {
				return RouteInvalidNextHop
			}
		case header.IPv6AddressSize:
			if !header.IsV6UnicastAddress(addr) {
				return RouteInvalidNextHop
			}
		default:
			// NextHop is neither an IPv6 nor IPv4 address: it is empty, and should be
			// rejected (because NextHopPresent is set)
			return RouteInvalidNextHop
		}
	}

	return RouteOk
}

type RouteValidationResult int

const (
	RouteOk RouteValidationResult = iota
	RouteInvalidDestinationSubnet
	RouteInvalidNextHop
	RouteInvalidUnknownAction
	RouteInvalidMissingRouteProperties
	RouteInvalidMissingMetric
)

func (result *RouteValidationResult) ToError() error {
	switch *result {
	case RouteOk:
		return nil
	case RouteInvalidDestinationSubnet:
		return errors.New("invalid destination subnet")
	case RouteInvalidNextHop:
		return errors.New("invalid next hop")
	case RouteInvalidUnknownAction:
		return errors.New("unknown route action")
	case RouteInvalidMissingRouteProperties:
		return errors.New("missing route properties")
	case RouteInvalidMissingMetric:
		return errors.New("missing metric")
	default:
		panic(fmt.Sprintf("Unknown validation result variant: %d", *result))
	}
}

// validateSubnet returns true if the prefix length is valid and no
// address bits are set beyond the prefix length.
func validateSubnet(subnet fuchsianet.Subnet) bool {
	_, err := ToTCPIPSubnetChecked(subnet)

	return err == nil
}

func FromFidlRouteV4(route fidlRoutes.RouteV4) (Route[fuchsianet.Ipv4Address], RouteValidationResult) {
	switch route.Action.Which() {
	case fidlRoutes.RouteActionV4Forward:
	default:
		return Route[fuchsianet.Ipv4Address]{}, RouteInvalidUnknownAction
	}

	if !route.Properties.SpecifiedPropertiesPresent {
		return Route[fuchsianet.Ipv4Address]{}, RouteInvalidMissingRouteProperties
	}

	if !route.Properties.SpecifiedProperties.MetricPresent {
		return Route[fuchsianet.Ipv4Address]{}, RouteInvalidMissingMetric
	}

	r := Route[fuchsianet.Ipv4Address]{
		Destination: fromFidlIpAddressWithPrefixV4(route.Destination),
		Action: RouteAction[fuchsianet.Ipv4Address]{
			RouteActionType: RouteActionTypeForward,
			Forward: RouteTarget[fuchsianet.Ipv4Address]{
				OutboundInterface: route.Action.Forward.OutboundInterface,
				NextHop: func() fuchsianet.Ipv4Address {
					if route.Action.Forward.NextHop != nil {
						return *route.Action.Forward.NextHop
					}
					return fuchsianet.Ipv4Address{}
				}(),
				NextHopPresent: route.Action.Forward.NextHop != nil,
			},
		},
		Properties: fromFidlSpecifiedRouteProperties(route.Properties.SpecifiedProperties),
	}
	return r, r.Validate()
}

func ToFidlRouteV4(route Route[fuchsianet.Ipv4Address]) (fidlRoutes.RouteV4, error) {
	var result fidlRoutes.RouteV4

	switch route.Action.RouteActionType {
	case RouteActionTypeForward:
		result.Action.SetForward(func() fidlRoutes.RouteTargetV4 {
			var target fidlRoutes.RouteTargetV4
			target.OutboundInterface = route.Action.Forward.OutboundInterface
			if route.Action.Forward.NextHopPresent {
				nextHop := route.Action.Forward.NextHop
				target.NextHop = &nextHop
			}
			return target
		}())
	case RouteActionTypeUnknown:
		return result, fmt.Errorf("unknown RouteActionType in %#v", route)
	}

	result.Destination = fuchsianet.Ipv4AddressWithPrefix{
		Addr:      route.Destination.Addr,
		PrefixLen: route.Destination.PrefixLen,
	}

	result.Properties.SetSpecifiedProperties(
		fidlRoutes.SpecifiedRouteProperties{
			Metric:        route.Properties.Metric,
			MetricPresent: true,
		},
	)
	return result, nil
}

func ToFidlRouteV6(route Route[fuchsianet.Ipv6Address]) (fidlRoutes.RouteV6, error) {
	var result fidlRoutes.RouteV6

	switch route.Action.RouteActionType {
	case RouteActionTypeForward:
		result.Action.SetForward(func() fidlRoutes.RouteTargetV6 {
			var target fidlRoutes.RouteTargetV6
			target.OutboundInterface = route.Action.Forward.OutboundInterface
			if route.Action.Forward.NextHopPresent {
				nextHop := route.Action.Forward.NextHop
				target.NextHop = &nextHop
			}
			return target
		}())
	case RouteActionTypeUnknown:
		return result, fmt.Errorf("unknown RouteActionType in %#v", route)
	}

	result.Destination = fuchsianet.Ipv6AddressWithPrefix{
		Addr:      route.Destination.Addr,
		PrefixLen: route.Destination.PrefixLen,
	}

	result.Properties.SetSpecifiedProperties(
		fidlRoutes.SpecifiedRouteProperties{
			Metric:        route.Properties.Metric,
			MetricPresent: true,
		},
	)
	return result, nil
}

func FromFidlRouteV6(route fidlRoutes.RouteV6) (Route[fuchsianet.Ipv6Address], RouteValidationResult) {
	switch route.Action.Which() {
	case fidlRoutes.RouteActionV4Forward:
	default:
		return Route[fuchsianet.Ipv6Address]{}, RouteInvalidUnknownAction
	}

	if !route.Properties.SpecifiedPropertiesPresent {
		return Route[fuchsianet.Ipv6Address]{}, RouteInvalidMissingRouteProperties
	}

	if !route.Properties.SpecifiedProperties.MetricPresent {
		return Route[fuchsianet.Ipv6Address]{}, RouteInvalidMissingMetric
	}

	r := Route[fuchsianet.Ipv6Address]{
		Destination: fromFidlIpAddressWithPrefixV6(route.Destination),
		Action: RouteAction[fuchsianet.Ipv6Address]{
			RouteActionType: RouteActionTypeForward,
			Forward: RouteTarget[fuchsianet.Ipv6Address]{
				OutboundInterface: route.Action.Forward.OutboundInterface,
				NextHop: func() fuchsianet.Ipv6Address {
					if route.Action.Forward.NextHop != nil {
						return *route.Action.Forward.NextHop
					}
					return fuchsianet.Ipv6Address{}
				}(),
				NextHopPresent: route.Action.Forward.NextHop != nil,
			},
		},
		Properties: fromFidlSpecifiedRouteProperties(route.Properties.SpecifiedProperties),
	}
	return r, r.Validate()
}

type IpAddressWithPrefix[A IpAddress] struct {
	Addr      A
	PrefixLen uint8
}

func (a *IpAddressWithPrefix[A]) ToSubnet() fuchsianet.Subnet {
	return fuchsianet.Subnet{
		PrefixLen: a.PrefixLen,
		Addr:      toFidlIpAddress[A](a.Addr),
	}
}

func fromFidlIpAddressWithPrefixV4(addr fuchsianet.Ipv4AddressWithPrefix) IpAddressWithPrefix[fuchsianet.Ipv4Address] {
	return IpAddressWithPrefix[fuchsianet.Ipv4Address]{
		Addr:      addr.Addr,
		PrefixLen: addr.PrefixLen,
	}
}

func fromFidlIpAddressWithPrefixV6(addr fuchsianet.Ipv6AddressWithPrefix) IpAddressWithPrefix[fuchsianet.Ipv6Address] {
	return IpAddressWithPrefix[fuchsianet.Ipv6Address]{
		Addr:      addr.Addr,
		PrefixLen: addr.PrefixLen,
	}
}

type IpAddress interface {
	fuchsianet.Ipv4Address | fuchsianet.Ipv6Address
}

func toFidlIpAddress[A IpAddress](addr A) fuchsianet.IpAddress {
	var fidlIpAddr fuchsianet.IpAddress

	switch any(addr).(type) {
	case fuchsianet.Ipv4Address:
		fidlIpAddr.SetIpv4(any(addr).(fuchsianet.Ipv4Address))
	case fuchsianet.Ipv6Address:
		fidlIpAddr.SetIpv6(any(addr).(fuchsianet.Ipv6Address))
	default:
		panic(fmt.Sprintf("unknown IP address type: %T", addr))
	}

	return fidlIpAddr
}

type RouteActionType int

const (
	RouteActionTypeUnknown RouteActionType = iota
	RouteActionTypeForward
)

type RouteAction[A IpAddress] struct {
	RouteActionType
	Forward RouteTarget[A]
}

// RouteTarget must have exclusively value types so that structs containing it
// (such as Route) can be used as map keys with the expected semantics
// (no pointers).
type RouteTarget[A IpAddress] struct {
	OutboundInterface uint64
	NextHop           A
	NextHopPresent    bool
}

// SpecifiedRouteProperties must have exclusively value types so that structs
// containing it (such as Route) can be used as map keys with the expected
// semantics (no pointers).
type SpecifiedRouteProperties struct {
	Metric fidlRoutes.SpecifiedMetric
}

func fromFidlSpecifiedRouteProperties(props fidlRoutes.SpecifiedRouteProperties) SpecifiedRouteProperties {
	return SpecifiedRouteProperties{
		Metric: props.Metric,
	}
}
