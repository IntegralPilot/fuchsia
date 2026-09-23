// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	"context"
	"testing"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/fidlconv"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/util"

	"fidl/fuchsia/net"
	"fidl/fuchsia/net/stack"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/header"
)

func TestValidateIPAddressMask(t *testing.T) {
	var (
		addr1 = util.Parse("10.11.224.0")
		addr2 = util.Parse("10.11.255.0")
	)

	for _, tc := range []struct {
		addr      tcpip.Address
		prefixLen uint8
		want      bool
	}{
		{addr: addr1, prefixLen: 32, want: true},
		{addr: addr1, prefixLen: 20, want: true},
		{addr: addr1, prefixLen: 19, want: true},
		{addr: addr1, prefixLen: 18, want: false},

		{addr: addr2, prefixLen: 25, want: true},
		{addr: addr2, prefixLen: 24, want: true},
		{addr: addr2, prefixLen: 23, want: false},

		{addr: header.IPv4Any, prefixLen: 0, want: true},
		{addr: header.IPv4Any, prefixLen: 32, want: true},
		{addr: header.IPv4Any, prefixLen: 33, want: false},
	} {
		addr := fidlconv.ToNetIpAddress(tc.addr)
		if got := validateSubnet(net.Subnet{Addr: addr, PrefixLen: tc.prefixLen}); got != tc.want {
			t.Errorf("got validateSubnet(%v) = %t, want = %t", addr, got, tc.want)
		}
	}
}

func AssertNoError(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Errorf("Received unexpected error:\n%+v", err)
	}
}

func TestFuchsiaNetStack(t *testing.T) {
	t.Run("Add and Delete Forwarding Entries", func(t *testing.T) {
		ns, _ := newNetstack(t, netstackTestOptions{})
		ifs := addNoopEndpoint(t, ns, "")
		device := uint64(ifs.nicid)
		t.Cleanup(ifs.RemoveByUser)
		ni := stackImpl{ns: ns}

		getTable := func(ns *Netstack) []stack.ForwardingEntry {
			ert := ns.GetExtendedRouteTable()
			entries := make([]stack.ForwardingEntry, 0, len(ert))
			for _, er := range ert {
				entry := fidlconv.TCPIPRouteToForwardingEntry(er.Route)
				entry.Metric = uint32(er.Metric)
				entries = append(entries, entry)
			}
			return entries
		}

		// The multicast subnet routes that are implicitly installed on all devices.
		ipv4MulticastSubnetRoute := stack.ForwardingEntry{
			Subnet:   fidlconv.ToNetSubnet(ipv4MulticastSubnet()),
			DeviceId: device,
			Metric:   uint32(defaultInterfaceMetric),
		}
		ipv6MulticastSubnetRoute := stack.ForwardingEntry{
			Subnet:   fidlconv.ToNetSubnet(ipv6MulticastSubnet()),
			DeviceId: device,
			Metric:   uint32(defaultInterfaceMetric),
		}

		nonexistentSubnet := net.Subnet{
			Addr:      fidlconv.ToNetIpAddress(util.Parse("170.11.224.0")),
			PrefixLen: 19,
		}
		badMaskSubnet := net.Subnet{
			Addr:      fidlconv.ToNetIpAddress(util.Parse("192.168.17.1")),
			PrefixLen: 19,
		}
		localSubnet := net.Subnet{
			Addr:      fidlconv.ToNetIpAddress(util.Parse("192.168.32.0")),
			PrefixLen: 19,
		}
		defaultSubnet := net.Subnet{
			Addr:      fidlconv.ToNetIpAddress(header.IPv4Any),
			PrefixLen: 0,
		}

		invalidRoute := stack.ForwardingEntry{
			Subnet:   localSubnet,
			DeviceId: device + 1,
		}
		localRoute := stack.ForwardingEntry{
			Subnet:   localSubnet,
			DeviceId: device,
		}
		wantLocalRoute := localRoute
		wantLocalRoute.Metric = uint32(defaultInterfaceMetric)
		nextHop := fidlconv.ToNetIpAddress(util.Parse("192.168.32.1"))
		defaultRoute := stack.ForwardingEntry{
			Subnet:   defaultSubnet,
			NextHop:  &nextHop,
			DeviceId: device,
		}
		wantDefaultRoute := defaultRoute
		wantDefaultRoute.Metric = uint32(defaultInterfaceMetric)

		// Add an invalid entry.
		addResult, err := ni.AddForwardingEntry(context.Background(), invalidRoute)
		AssertNoError(t, err)
		if addResult != stack.StackAddForwardingEntryResultWithErr(stack.ErrorInvalidArgs) {
			t.Errorf("got ni.AddForwardingEntry(%#v) = %#v, want = Err(ErrorInvalidArgs)", invalidRoute, addResult)
		}

		// Add a local subnet route.
		addResult, err = ni.AddForwardingEntry(context.Background(), localRoute)
		AssertNoError(t, err)
		if addResult != stack.StackAddForwardingEntryResultWithResponse(stack.StackAddForwardingEntryResponse{}) {
			t.Fatalf("got ni.AddForwardingEntry(%#v) = %#v, want = Response()", localRoute, addResult)
		}
		table := getTable(ns)
		expectedTable := []stack.ForwardingEntry{
			wantLocalRoute,
			ipv4MulticastSubnetRoute,
			ipv6MulticastSubnetRoute,
		}
		if diff := cmp.Diff(expectedTable, table, cmpopts.IgnoreTypes(struct{}{})); diff != "" {
			t.Fatalf("forwarding table mismatch (-want +got):\n%s", diff)
		}

		// Add the same entry again.
		addResult, err = ni.AddForwardingEntry(context.Background(), localRoute)
		AssertNoError(t, err)
		if addResult != stack.StackAddForwardingEntryResultWithResponse(stack.StackAddForwardingEntryResponse{}) {
			t.Errorf("got ni.AddForwardingEntry(%#v) = %#v, want = Response()", localRoute, addResult)
		}
		table = getTable(ns)
		AssertNoError(t, err)
		if diff := cmp.Diff(expectedTable, table, cmpopts.IgnoreTypes(struct{}{})); diff != "" {
			t.Fatalf("forwarding table mismatch (-want +got):\n%s", diff)
		}

		// Add default route.
		addResult, err = ni.AddForwardingEntry(context.Background(), defaultRoute)
		AssertNoError(t, err)
		if addResult != stack.StackAddForwardingEntryResultWithResponse(stack.StackAddForwardingEntryResponse{}) {
			t.Errorf("got ni.AddForwardingEntry(%#v) = %#v, want = Response()", defaultRoute, addResult)
		}
		table = getTable(ns)
		AssertNoError(t, err)
		expectedTable = []stack.ForwardingEntry{
			wantLocalRoute,
			ipv4MulticastSubnetRoute,
			ipv6MulticastSubnetRoute,
			wantDefaultRoute,
		}
		if diff := cmp.Diff(expectedTable, table, cmpopts.IgnoreTypes(struct{}{})); diff != "" {
			t.Fatalf("forwarding table mismatch (-want +got):\n%s", diff)
		}

		// Remove nonexistent subnet.
		delResult, err := ni.DelForwardingEntry(context.Background(), stack.ForwardingEntry{Subnet: nonexistentSubnet})
		AssertNoError(t, err)
		if delResult != stack.StackDelForwardingEntryResultWithErr(stack.ErrorNotFound) {
			t.Errorf("got ni.DelForwardingEntry(%#v) = %#v, want = Err(ErrorNotFound)", nonexistentSubnet, delResult)
		}

		// Remove subnet with bad subnet mask.
		delResult, err = ni.DelForwardingEntry(context.Background(), stack.ForwardingEntry{Subnet: badMaskSubnet})
		AssertNoError(t, err)
		if delResult != stack.StackDelForwardingEntryResultWithErr(stack.ErrorInvalidArgs) {
			t.Errorf("got ni.DelForwardingEntry(%#v) = %#v, want = Err(ErrorInvalidArgs)", badMaskSubnet, delResult)
		}

		// Remove local route.
		delResult, err = ni.DelForwardingEntry(context.Background(), stack.ForwardingEntry{Subnet: localSubnet})
		AssertNoError(t, err)
		if delResult != stack.StackDelForwardingEntryResultWithResponse(stack.StackDelForwardingEntryResponse{}) {
			t.Fatalf("got ni.DelForwardingEntry(%#v) = Err(%s), want = Response()", localRoute, delResult.Err)
		}
		table = getTable(ns)
		AssertNoError(t, err)
		expectedTable = []stack.ForwardingEntry{
			ipv4MulticastSubnetRoute,
			ipv6MulticastSubnetRoute,
			wantDefaultRoute,
		}
		if diff := cmp.Diff(expectedTable, table, cmpopts.IgnoreTypes(struct{}{})); diff != "" {
			t.Fatalf("forwarding table mismatch (-want +got):\n%s", diff)
		}

		// Remove default route.
		delResult, err = ni.DelForwardingEntry(context.Background(), stack.ForwardingEntry{Subnet: defaultSubnet})
		AssertNoError(t, err)
		if delResult != stack.StackDelForwardingEntryResultWithResponse(stack.StackDelForwardingEntryResponse{}) {
			t.Fatalf("got ni.DelForwardingEntry(%#v) = Err(%s), want = Response()", localRoute, delResult.Err)
		}
		table = getTable(ns)
		AssertNoError(t, err)
		expectedTable = []stack.ForwardingEntry{
			ipv4MulticastSubnetRoute,
			ipv6MulticastSubnetRoute,
		}
		if diff := cmp.Diff(expectedTable, table, cmpopts.IgnoreTypes(struct{}{})); diff != "" {
			t.Fatalf("forwarding table mismatch (-want +got):\n%s", diff)
		}
	})
}

func TestSetDhcpClientEnabled(t *testing.T) {
	t.Run("bad NIC", func(t *testing.T) {
		ns, _ := newNetstack(t, netstackTestOptions{})
		stackServiceImpl := stackImpl{ns: ns}

		result, err := stackServiceImpl.SetDhcpClientEnabled(context.Background(), 1234, true)
		if err != nil {
			t.Fatalf("stackServiceImpl.StartDhcpClient(...) = %s", err)
		}
		if got, want := result.Which(), stack.I_stackSetDhcpClientEnabledResultTag(stack.StackSetDhcpClientEnabledResultErr); got != want {
			t.Fatalf("got result.Which() = %d, want = %d", got, want)
		}
		if got, want := result.Err, stack.ErrorNotFound; got != want {
			t.Fatalf("got result.Err = %s, want = %s", got, want)
		}
	})

	t.Run("good NIC", func(t *testing.T) {
		ns, _ := newNetstack(t, netstackTestOptions{})
		ifs1 := addNoopEndpoint(t, ns, "")
		t.Cleanup(ifs1.RemoveByUser)

		stackServiceImpl := stackImpl{ns: ns}

		result, err := stackServiceImpl.SetDhcpClientEnabled(context.Background(), uint64(ifs1.nicid), true)
		if err != nil {
			t.Fatalf("stackServiceImpl.StartDhcpClient(...) = %s", err)
		}
		if got, want := result.Which(), stack.I_stackSetDhcpClientEnabledResultTag(stack.StackSetDhcpClientEnabledResultResponse); got != want {
			t.Fatalf("got result.Which() = %d, want = %d", got, want)
		}
	})
}
