// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlconv

import (
	"fmt"
	stdnet "net"
	"net/netip"
	"syscall/zx"
	"time"

	"fidl/fuchsia/net"
	"fidl/fuchsia/net/interfaces"
	interfacesadmin "fidl/fuchsia/net/interfaces/admin"
	"fidl/fuchsia/net/multicast/admin"
	fnetRoutes "fidl/fuchsia/net/routes"
	"fidl/fuchsia/net/stack"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routetypes"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/header"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv6"
	tcpipstack "gvisor.dev/gvisor/pkg/tcpip/stack"
)

func ToTCPIPNetProto(v net.IpVersion) tcpip.NetworkProtocolNumber {
	switch v {
	case net.IpVersionV4:
		return header.IPv4ProtocolNumber
	case net.IpVersionV6:
		return header.IPv6ProtocolNumber
	default:
		panic(fmt.Sprintf("invalid fuchsia.net/IpVersion %s (%d)", v, v))
	}
}

func ToTCPIPAddressAndProtocolNumber(addr net.IpAddress) (tcpip.Address, tcpip.NetworkProtocolNumber) {
	switch tag := addr.Which(); tag {
	case net.IpAddressIpv4:
		return tcpip.AddrFrom4(addr.Ipv4.Addr), ipv4.ProtocolNumber
	case net.IpAddressIpv6:
		return tcpip.AddrFrom16(addr.Ipv6.Addr), ipv6.ProtocolNumber
	default:
		panic(fmt.Sprintf("invalid fuchsia.net/IpAddress tag %d", tag))
	}
}

func ToTCPIPAddress(addr net.IpAddress) tcpip.Address {
	a, _ := ToTCPIPAddressAndProtocolNumber(addr)
	return a
}

func ToStdAddr(addr net.IpAddress) netip.Addr {
	switch tag := addr.Which(); tag {
	case net.IpAddressIpv4:
		return netip.AddrFrom4(addr.Ipv4.Addr)
	case net.IpAddressIpv6:
		return netip.AddrFrom16(addr.Ipv6.Addr)
	default:
		panic(fmt.Sprintf("invalid fuchsia.net/IpAddress tag %d", tag))
	}
}

func ToNetIpAddress(addr tcpip.Address) net.IpAddress {
	switch l := addr.Len(); l {
	case header.IPv4AddressSize:
		var v4 net.Ipv4Address
		copy(v4.Addr[:], addr.AsSlice())
		return net.IpAddressWithIpv4(v4)
	case header.IPv6AddressSize:
		var v6 net.Ipv6Address
		copy(v6.Addr[:], addr.AsSlice())
		return net.IpAddressWithIpv6(v6)
	default:
		panic(fmt.Sprintf("invalid IP address length = %d: %x", l, addr))
	}
}

func ToNetSubnet(addressWithPrefix tcpip.AddressWithPrefix) net.Subnet {
	return net.Subnet{
		Addr:      ToNetIpAddress(addressWithPrefix.Address),
		PrefixLen: uint8(addressWithPrefix.PrefixLen),
	}
}

func ToNetMacAddress(addr tcpip.LinkAddress) net.MacAddress {
	if len(addr) != header.EthernetAddressSize {
		panic(fmt.Sprintf("invalid link address length = %d: %x", len(addr), addr))
	}
	var mac net.MacAddress
	copy(mac.Octets[:], addr)
	return mac
}

func ToTCPIPLinkAddress(mac net.MacAddress) tcpip.LinkAddress {
	return tcpip.LinkAddress(mac.Octets[:])
}

func ToNetSocketAddress(addr tcpip.FullAddress) net.SocketAddress {
	var out net.SocketAddress
	switch l := addr.Addr.Len(); l {
	case header.IPv4AddressSize:
		var v4 net.Ipv4Address
		copy(v4.Addr[:], addr.Addr.AsSlice())
		out.SetIpv4(net.Ipv4SocketAddress{
			Address: v4,
			Port:    addr.Port,
		})
	case header.IPv6AddressSize:
		var v6 net.Ipv6Address
		copy(v6.Addr[:], addr.Addr.AsSlice())

		// Zone information should only be included for non-global addresses as the same
		// address may be used across different zones. Note, there is only a single globally
		// scoped zone where global addresses may only be used once so zone information is not
		// needed for global addresses. See RFC 4007 section 6 for more details.
		var zoneIdx uint64
		if header.IsV6LinkLocalUnicastAddress(addr.Addr) || header.IsV6LinkLocalMulticastAddress(addr.Addr) {
			zoneIdx = uint64(addr.NIC)
		}
		out.SetIpv6(net.Ipv6SocketAddress{
			Address:   v6,
			Port:      addr.Port,
			ZoneIndex: zoneIdx,
		})
	default:
		panic(fmt.Sprintf("invalid IP address length = %d: %x", l, addr.Addr))
	}
	return out
}

// isLinkLocal determines if the given IPv6 address is link-local. This is the
// case when it has the fe80::/10 prefix. This check is used to determine when
// the NICID is relevant for a given IPv6 address.
func isLinkLocal(addr net.Ipv6Address) bool {
	return addr.Addr[0] == 0xfe && addr.Addr[1]&0xc0 == 0x80
}

// ToNetSocketAddress converts a tcpip.FullAddress into a fnet.SocketAddress
// taking the protocol into consideration. If addr is unspecified, the
// unspecified address for the provided protocol is returned.
//
// Panics if protocol is neither IPv4 nor IPv6.
func ToNetSocketAddressWithProto(protocol tcpip.NetworkProtocolNumber, addr tcpip.FullAddress) net.SocketAddress {
	switch protocol {
	case ipv4.ProtocolNumber:
		out := net.Ipv4SocketAddress{
			Port: addr.Port,
		}
		copy(out.Address.Addr[:], addr.Addr.AsSlice())
		return net.SocketAddressWithIpv4(out)
	case ipv6.ProtocolNumber:
		out := net.Ipv6SocketAddress{
			Port: addr.Port,
		}
		if addr.Addr.Len() == header.IPv4AddressSize {
			// Copy address in v4-mapped format.
			copy(out.Address.Addr[header.IPv6AddressSize-header.IPv4AddressSize:], addr.Addr.AsSlice())
			out.Address.Addr[header.IPv6AddressSize-header.IPv4AddressSize-1] = 0xff
			out.Address.Addr[header.IPv6AddressSize-header.IPv4AddressSize-2] = 0xff
		} else {
			copy(out.Address.Addr[:], addr.Addr.AsSlice())
			if isLinkLocal(out.Address) {
				out.ZoneIndex = uint64(addr.NIC)
			}
		}
		return net.SocketAddressWithIpv6(out)
	default:
		panic(fmt.Sprintf("invalid protocol for conversion: %d", protocol))
	}
}

func ToTCPIPAddressWithPrefix(sn net.Subnet) tcpip.AddressWithPrefix {
	return tcpip.AddressWithPrefix{
		Address:   ToTCPIPAddress(sn.Addr),
		PrefixLen: int(sn.PrefixLen),
	}
}

func ToTCPIPSubnet(sn net.Subnet) tcpip.Subnet {
	return ToTCPIPAddressWithPrefix(sn).Subnet()
}

// ToTCPIPSubnetChecked converts the fuchsia.net.Subnet into a tcpip.Subnet,
// returning an error if the subnet prefix length is invalid or if the subnet
// address has host bits set.
func ToTCPIPSubnetChecked(sn net.Subnet) (tcpip.Subnet, error) {
	ones := sn.PrefixLen
	addr := ToTCPIPAddress(sn.Addr)
	return tcpip.NewSubnet(addr, tcpip.MaskFromBytes(stdnet.CIDRMask(int(ones), addr.BitLen())))
}

func ToTCPIPProtocolAddress(sn net.Subnet) tcpip.ProtocolAddress {
	protocolAddr := tcpip.ProtocolAddress{
		AddressWithPrefix: ToTCPIPAddressWithPrefix(sn),
	}

	switch typ := sn.Addr.Which(); typ {
	case net.IpAddressIpv4:
		protocolAddr.Protocol = ipv4.ProtocolNumber
	case net.IpAddressIpv6:
		protocolAddr.Protocol = ipv6.ProtocolNumber
	default:
		panic(fmt.Sprintf("unknown IpAddress type %d", typ))
	}
	return protocolAddr
}

func TCPIPRouteToForwardingEntry(route tcpip.Route) stack.ForwardingEntry {
	forwardingEntry := stack.ForwardingEntry{
		Subnet: net.Subnet{
			Addr:      ToNetIpAddress(route.Destination.ID()),
			PrefixLen: uint8(route.Destination.Prefix()),
		},
		DeviceId: uint64(route.NIC),
	}
	if route.Gateway.Len() != 0 {
		nextHop := ToNetIpAddress(route.Gateway)
		forwardingEntry.NextHop = &nextHop
	}
	return forwardingEntry
}

func ForwardingEntryToTCPIPRoute(forwardingEntry stack.ForwardingEntry) tcpip.Route {
	route := tcpip.Route{
		Destination: ToTCPIPSubnet(forwardingEntry.Subnet),
		NIC:         tcpip.NICID(forwardingEntry.DeviceId),
	}
	if nextHop := forwardingEntry.NextHop; nextHop != nil {
		route.Gateway = ToTCPIPAddress(*nextHop)
	}
	return route
}

// ToStackMulticastRoute converts the provided route to a stack multicast
// route.
//
// Returns true if the provided route contains all required fields and the
// conversion was successful. Otherwise, returns false.
func ToStackMulticastRoute(route admin.Route) (tcpipstack.MulticastRoute, bool) {
	if !route.HasExpectedInputInterface() || !route.HasAction() {
		return tcpipstack.MulticastRoute{}, false
	}

	outgoingInterfaces := make([]tcpipstack.MulticastRouteOutgoingInterface, 0, len(route.Action.OutgoingInterfaces))

	for _, outgoingInterface := range route.Action.OutgoingInterfaces {
		outgoingInterfaces = append(outgoingInterfaces, tcpipstack.MulticastRouteOutgoingInterface{
			ID:     tcpip.NICID(outgoingInterface.Id),
			MinTTL: outgoingInterface.MinTtl,
		})
	}

	return tcpipstack.MulticastRoute{
		ExpectedInputInterface: tcpip.NICID(route.ExpectedInputInterface),
		OutgoingInterfaces:     outgoingInterfaces,
	}, true
}

// TODO(https://fxbug.dev/42181878): Replace usages with `Clock.Elapsed` when available.
func ToZxTime(t tcpip.MonotonicTime) zx.Time {
	return zx.Time(t.Sub(tcpip.MonotonicTime{}))
}

func ToTCPIPMonotonicTime(zxtime zx.Time) tcpip.MonotonicTime {
	return tcpip.MonotonicTime{}.Add(time.Duration(zxtime))
}

func BytesToAddressDroppingUnspecified(b []uint8) tcpip.Address {
	for _, e := range b {
		if e != 0 {
			return tcpip.AddrFromSlice(b)
		}
	}
	return tcpip.Address{}
}

func ToTCPIPFullAddress(addr net.SocketAddress) tcpip.FullAddress {
	switch w := addr.Which(); w {
	case net.SocketAddressIpv4:
		return tcpip.FullAddress{
			NIC:  0,
			Addr: BytesToAddressDroppingUnspecified(addr.Ipv4.Address.Addr[:]),
			Port: addr.Ipv4.Port,
		}
	case net.SocketAddressIpv6:
		return tcpip.FullAddress{
			NIC:  tcpip.NICID(addr.Ipv6.ZoneIndex),
			Addr: BytesToAddressDroppingUnspecified(addr.Ipv6.Address.Addr[:]),
			Port: addr.Ipv6.Port,
		}
	default:
		panic(fmt.Sprintf("invalid fuchsia.net/SocketAddress variant: %d", w))
	}
}

func ToTcpIpAddressDroppingUnspecifiedv4(fidl net.Ipv4Address) tcpip.Address {
	return BytesToAddressDroppingUnspecified(fidl.Addr[:])
}

func ToTcpIpAddressDroppingUnspecifiedv6(fidl net.Ipv6Address) tcpip.Address {
	return BytesToAddressDroppingUnspecified(fidl.Addr[:])
}

func ToAddressAssignmentState(state tcpipstack.AddressAssignmentState) interfaces.AddressAssignmentState {
	switch state {
	case tcpipstack.AddressDisabled:
		return interfaces.AddressAssignmentStateUnavailable
	case tcpipstack.AddressAssigned:
		return interfaces.AddressAssignmentStateAssigned
	case tcpipstack.AddressTentative:
		return interfaces.AddressAssignmentStateTentative
	default:
		panic(fmt.Errorf("unknown address assignment state: %d", state))
	}
}

func ToAddressRemovalReason(reason tcpipstack.AddressRemovalReason) interfacesadmin.AddressRemovalReason {
	switch reason {
	case tcpipstack.AddressRemovalDADFailed:
		return interfacesadmin.AddressRemovalReasonDadFailed
	case tcpipstack.AddressRemovalInterfaceRemoved:
		return interfacesadmin.AddressRemovalReasonInterfaceRemoved
	case tcpipstack.AddressRemovalManualAction:
		return interfacesadmin.AddressRemovalReasonUserRemoved
	// TODO(https://fxbug.dev/42065170): When invalidation for all addresses are
	// handled in-stack, change this to return an appropriate FIDL variant instead
	// of panicking.
	// The invalidated removal reason is only returned for addresses auto-generated
	// within the stack, and cannot be returned for addresses added via
	// fuchsia.net.interfaces.admin/Control.AddAddress.
	case tcpipstack.AddressRemovalInvalidated:
		panic("unexpected address removal due to invalidation")
	default:
		panic(fmt.Errorf("unknown address removal reason: %d", reason))
	}
}

// A union type, abstracting over `InstalledRouteV4` and `InstalledRouteV6`.
type InstalledRoute struct {
	Version routetypes.IpProtoTag
	V4      fnetRoutes.InstalledRouteV4
	V6      fnetRoutes.InstalledRouteV6
}

// Converts the given `ExtendedRoute` into an `InstalledRoute`.
// Panics if the given route is neither IPv4 nor IPv6, or if the route's
// destination & gateway are different IP versions.
func ToInstalledRoute(route routetypes.ExtendedRoute) InstalledRoute {
	var specifiedMetric fnetRoutes.SpecifiedMetric
	if route.MetricTracksInterface {
		specifiedMetric.SetInheritedFromInterface(fnetRoutes.Empty{})
	} else {
		specifiedMetric.SetExplicitMetric(uint32(route.Metric))
	}
	var specifiedProperties fnetRoutes.SpecifiedRouteProperties
	specifiedProperties.SetMetric(specifiedMetric)
	var effectiveProperties fnetRoutes.EffectiveRouteProperties
	effectiveProperties.SetMetric(uint32(route.Metric))

	dst := ToNetIpAddress(route.Route.Destination.ID())

	switch dst.Which() {
	case net.IpAddressIpv4:
		destination := net.Ipv4AddressWithPrefix{
			Addr:      dst.Ipv4,
			PrefixLen: uint8(route.Route.Destination.Prefix()),
		}
		target := fnetRoutes.RouteTargetV4{
			OutboundInterface: uint64(route.Route.NIC),
		}
		if route.Route.Gateway.Len() != 0 {
			gateway := ToNetIpAddress(route.Route.Gateway)
			if gateway.Which() != net.IpAddressIpv4 {
				panic(fmt.Sprintf(
					"Route with IPv4 Destination and non-IPv4 Gateway: %s",
					route.Route,
				))
			}
			target.NextHop = &gateway.Ipv4
		}

		var properties fnetRoutes.RoutePropertiesV4
		properties.SetSpecifiedProperties(specifiedProperties)
		innerRoute := fnetRoutes.RouteV4{
			Destination: destination,
			Action:      fnetRoutes.RouteActionV4WithForward(target),
			Properties:  properties,
		}
		var installedRoute fnetRoutes.InstalledRouteV4
		installedRoute.SetRoute(innerRoute)
		installedRoute.SetEffectiveProperties(effectiveProperties)
		installedRoute.SetTableId(V4MainRouteTableId)
		return InstalledRoute{
			Version: routetypes.IPv4,
			V4:      installedRoute,
		}
	case net.IpAddressIpv6:
		destination := net.Ipv6AddressWithPrefix{
			Addr:      dst.Ipv6,
			PrefixLen: uint8(route.Route.Destination.Prefix()),
		}
		target := fnetRoutes.RouteTargetV6{
			OutboundInterface: uint64(route.Route.NIC),
		}
		if route.Route.Gateway.Len() != 0 {
			gateway := ToNetIpAddress(route.Route.Gateway)
			if gateway.Which() != net.IpAddressIpv6 {
				panic(fmt.Sprintf(
					"Route with IPv6 Destination and non-IPv6 Gateway: %s",
					route.Route,
				))
			}
			target.NextHop = &gateway.Ipv6
		}
		var properties fnetRoutes.RoutePropertiesV6
		properties.SetSpecifiedProperties(specifiedProperties)
		innerRoute := fnetRoutes.RouteV6{
			Destination: destination,
			Action:      fnetRoutes.RouteActionV6WithForward(target),
			Properties:  properties,
		}
		var installedRoute fnetRoutes.InstalledRouteV6
		installedRoute.SetRoute(innerRoute)
		installedRoute.SetEffectiveProperties(effectiveProperties)
		installedRoute.SetTableId(V6MainRouteTableId)
		return InstalledRoute{
			Version: routetypes.IPv6,
			V6:      installedRoute,
		}
	default:
		panic(fmt.Sprintf("invalid IP protocol for address: I_ipAddressTag=%d", dst.I_ipAddressTag))
	}
}

type RouteComparisonKey struct {
	V4 Route[net.Ipv4Address]
	V6 Route[net.Ipv6Address]
}

// Converts the given `InstalledRoute` to a `RouteComparisonKey` such that if
// this route's `RouteComparisonKey` is equal to another route's
// `RouteComparisonKey`, the two routes should be considered identical for the
// purposes of adding or removing a matching route in the routing table.
func (installedRoute InstalledRoute) ToRouteComparisonKey() (RouteComparisonKey, error) {
	var key RouteComparisonKey
	switch installedRoute.Version {
	case routetypes.IPv4:
		route, validationResult := FromFidlRouteV4(installedRoute.V4.GetRoute())
		if err := validationResult.ToError(); err != nil {
			return key, err
		}
		key.V4 = route
		return key, nil
	case routetypes.IPv6:
		route, validationResult := FromFidlRouteV6(installedRoute.V6.GetRoute())
		if err := validationResult.ToError(); err != nil {
			return key, err
		}
		key.V6 = route
		return key, nil
	default:
		panic("unknown IpProtoTag")
	}
}

// Converts the given `ExtendedRoute` to a `RouteComparisonKey` such that if
// this route's `RouteComparisonKey` is equal to another route's
// `RouteComparisonKey`, the two routes should be considered identical for the
// purposes of adding or removing a matching route in the routing table.
func ToRouteComparisonKey(route routetypes.ExtendedRoute) (RouteComparisonKey, error) {
	return ToInstalledRoute(route).ToRouteComparisonKey()
}
