// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	"errors"
	"fmt"
	"syscall/zx/fidl"

	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/fidlconv"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routes"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routetypes"

	"fidl/fuchsia/net"
	"fidl/fuchsia/net/interfaces/admin"
	"fidl/fuchsia/net/stack"

	"gvisor.dev/gvisor/pkg/atomicbitops"
	"gvisor.dev/gvisor/pkg/tcpip"
)

const metricNotSet uint32 = 0

var _ stack.StackWithCtx = (*stackImpl)(nil)

type stackImpl struct {
	ns *Netstack
}

// validateSubnet returns true if the prefix length is valid and no
// address bits are set beyond the prefix length.
func validateSubnet(subnet net.Subnet) bool {
	var ipBytes []uint8
	switch typ := subnet.Addr.Which(); typ {
	case net.IpAddressIpv4:
		ipBytes = subnet.Addr.Ipv4.Addr[:]
	case net.IpAddressIpv6:
		ipBytes = subnet.Addr.Ipv6.Addr[:]
	default:
		panic(fmt.Sprintf("unknown IpAddress type %d", typ))
	}
	if int(subnet.PrefixLen) > len(ipBytes)*8 {
		return false
	}
	prefixBytes := subnet.PrefixLen / 8
	ipBytes = ipBytes[prefixBytes:]
	if prefixBits := subnet.PrefixLen - (prefixBytes * 8); prefixBits > 0 {
		// prefixBits is only greater than zero when ipBytes is non-empty.
		mask := uint8((1 << (8 - prefixBits)) - 1)
		ipBytes[0] &= mask
	}
	for _, byte := range ipBytes {
		if byte != 0 {
			return false
		}
	}
	return true
}

func (ns *Netstack) addForwardingEntry(entry stack.ForwardingEntry) stack.StackAddForwardingEntryResult {
	var result stack.StackAddForwardingEntryResult

	if !validateSubnet(entry.Subnet) {
		result.SetErr(stack.ErrorInvalidArgs)
		return result
	}

	metric := func() *routetypes.Metric {
		if entry.Metric != metricNotSet {
			metric := routetypes.Metric(entry.Metric)
			return &metric
		} else {
			return nil
		}
	}()

	route := fidlconv.ForwardingEntryToTCPIPRoute(entry)
	if _, err := ns.AddRoute(route, metric, false /* not dynamic */, true /* replaceMatchingGvisorRoutes */, routetypes.GlobalRouteSet()); err != nil {
		if errors.Is(err, routes.ErrNoSuchNIC) {
			result.SetErr(stack.ErrorInvalidArgs)
		} else {
			_ = syslog.Errorf("adding route %s to route table failed: %s", route, err)
			result.SetErr(stack.ErrorInternal)
		}
		return result
	}
	result.SetResponse(stack.StackAddForwardingEntryResponse{})
	return result
}

func (ns *Netstack) delForwardingEntry(entry stack.ForwardingEntry) stack.StackDelForwardingEntryResult {
	if !validateSubnet(entry.Subnet) {
		return stack.StackDelForwardingEntryResultWithErr(stack.ErrorInvalidArgs)
	}

	route := fidlconv.ForwardingEntryToTCPIPRoute(entry)
	if routesDeleted := ns.DelRoute(route, routetypes.GlobalRouteSet()); len(routesDeleted) == 0 {
		return stack.StackDelForwardingEntryResultWithErr(stack.ErrorNotFound)
	}
	return stack.StackDelForwardingEntryResultWithResponse(stack.StackDelForwardingEntryResponse{})
}

func (ni *stackImpl) AddForwardingEntry(_ fidl.Context, entry stack.ForwardingEntry) (stack.StackAddForwardingEntryResult, error) {
	return ni.ns.addForwardingEntry(entry), nil
}

func (ni *stackImpl) DelForwardingEntry(_ fidl.Context, entry stack.ForwardingEntry) (stack.StackDelForwardingEntryResult, error) {
	return ni.ns.delForwardingEntry(entry), nil
}

func (ni *stackImpl) SetDhcpClientEnabled(ctx_ fidl.Context, id uint64, enable bool) (stack.StackSetDhcpClientEnabledResult, error) {
	var r stack.StackSetDhcpClientEnabledResult

	nicInfo, ok := ni.ns.stack.NICInfo()[tcpip.NICID(id)]
	if !ok {
		return stack.StackSetDhcpClientEnabledResultWithErr(stack.ErrorNotFound), nil
	}

	ifState := nicInfo.Context.(*ifState)
	ifState.setDHCPStatus(nicInfo.Name, enable)

	r.SetResponse(stack.StackSetDhcpClientEnabledResponse{})
	return r, nil
}

func (ni *stackImpl) BridgeInterfaces(_ fidl.Context, interfaces []uint64, bridge admin.ControlWithCtxInterfaceRequest) error {
	_ = syslog.Infof("received request to bridge %v", interfaces)
	nics := make([]tcpip.NICID, len(interfaces))
	for i, n := range interfaces {
		nics[i] = tcpip.NICID(n)
	}
	ifs, err := ni.ns.Bridge(nics)
	if err != nil {
		_ = syslog.Warnf("failed to bridge interfaces %s", err)
		proxy := admin.ControlEventProxy{
			Channel: bridge.Channel,
		}
		if err := proxy.OnInterfaceRemoved(admin.InterfaceRemovedReasonBadPort); err != nil {
			_ = syslog.Warnf("failed to write terminal event: %s", err)
		}
		_ = bridge.Close()
		return nil
	}
	ifs.addAdminConnection(bridge, true /* strong */)
	return nil
}

var _ stack.LogWithCtx = (*logImpl)(nil)

type logImpl struct {
	logPackets *atomicbitops.Uint32
}

func (li *logImpl) SetLogPackets(_ fidl.Context, enabled bool) error {
	var val uint32
	if enabled {
		val = 1
	}
	li.logPackets.Store(val)
	syslog.VLogTf(syslog.DebugVerbosity, "fuchsia_net_stack", "SetLogPackets: %t", enabled)
	return nil
}
