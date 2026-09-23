// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package filter

import (
	"context"
	"testing"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/fidlconv"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/util"

	"fidl/fuchsia/net"
	filter "fidl/fuchsia/net/filter/deprecated"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"

	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
	"gvisor.dev/gvisor/pkg/tcpip/transport/udp"
)

func TestGetAndUpdateRules(t *testing.T) {
	s := stack.New(stack.Options{
		NetworkProtocols: []stack.NetworkProtocolFactory{
			ipv4.NewProtocol,
		},
		TransportProtocols: []stack.TransportProtocolFactory{
			udp.NewProtocol,
		},
	})
	fi := &filterImpl{filter: newForTest(s)}

	// 0. Prepare test rules.
	trs1 := []filter.Rule{
		{
			Action:       filter.ActionDrop,
			Direction:    filter.DirectionIncoming,
			Proto:        filter.SocketProtocolTcp,
			SrcSubnet:    &net.Subnet{Addr: fidlconv.ToNetIpAddress(util.Parse("10.0.0.0")), PrefixLen: 8},
			SrcPortRange: filter.PortRange{Start: 100, End: 100},
			Log:          true,
			DeviceClass:  filter.DeviceClassWithAny(filter.Empty{}),
		},
	}
	trs2 := []filter.Rule{
		{
			Action:       filter.ActionPass,
			Direction:    filter.DirectionIncoming,
			Proto:        filter.SocketProtocolTcp,
			SrcSubnet:    &net.Subnet{Addr: fidlconv.ToNetIpAddress(util.Parse("11.0.0.0")), PrefixLen: 8},
			SrcPortRange: filter.PortRange{Start: 100, End: 100},
			Log:          true,
			DeviceClass:  filter.DeviceClassWithAny(filter.Empty{}),
		},
	}

	validateGetRulesResult := func(t *testing.T, resultRules []filter.Rule, rules []filter.Rule) {
		t.Helper()
		if diff := cmp.Diff(resultRules, rules, cmpopts.IgnoreTypes(struct{}{})); diff != "" {
			t.Errorf("result.Response.Rules: (-want +got)\n%s", diff)
		}
	}
	validateUpdateRulesResult := func(t *testing.T, result filter.FilterUpdateRulesResult) {
		t.Helper()
		switch result.Which() {
		case filter.FilterUpdateRulesResultResponse:
			if got, want := result, filter.FilterUpdateRulesResultWithResponse(filter.FilterUpdateRulesResponse{}); got != want {
				t.Errorf("got result = %#v, want = %#v", got, want)
			}
		case filter.FilterUpdateRulesResultErr:
			t.Errorf("result.Err = %s", result.Err)
		}
	}
	validateUpdateRulesResultErr := func(t *testing.T, result filter.FilterUpdateRulesResult, err filter.FilterUpdateRulesError) {
		t.Helper()
		switch result.Which() {
		case filter.FilterUpdateRulesResultResponse:
			t.Errorf("result.Response = %#v", result.Response)
		case filter.FilterUpdateRulesResultErr:
			if got, want := result.Err, err; got != want {
				t.Errorf("got result.Err = %s, want = %s", result.Err, want)
			}
		}
	}

	var lastGeneration uint32

	// Get the current rules (should be empty).
	{
		rules, generation, err := fi.GetRules(context.Background())
		if err != nil {
			t.Errorf("GetRules error: %s", err)
		}
		validateGetRulesResult(t, rules, nil)
		lastGeneration = generation
	}
	// Update the current rules with trs1.
	{
		result, err := fi.UpdateRules(context.Background(), trs1, lastGeneration)
		if err != nil {
			t.Errorf("UpdateRules error: %s", err)
		}
		validateUpdateRulesResult(t, result)
	}
	// Get the current rules (should be trs1).
	{
		rules, generation, err := fi.GetRules(context.Background())
		if err != nil {
			t.Errorf("GetRules error: %s", err)
		}
		validateGetRulesResult(t, rules, trs1)
		if got, notWant := generation, lastGeneration; got == notWant {
			t.Errorf("got result.Response.Generation = %d (want = not %d)", got, notWant)
		}
		lastGeneration = generation
	}
	// Try to update the current rules with trs2 using a wrong generation number (should fail).
	{
		result, err := fi.UpdateRules(context.Background(), trs2, lastGeneration-1)
		if err != nil {
			t.Errorf("UpdateRules error: %s", err)
		}
		validateUpdateRulesResultErr(t, result, filter.FilterUpdateRulesErrorGenerationMismatch)
	}
	// Update the current rules with trs2 using a correct generation number.
	{
		result, err := fi.UpdateRules(context.Background(), trs2, lastGeneration)
		if err != nil {
			t.Errorf("UpdateRules error: %s", err)
		}
		validateUpdateRulesResult(t, result)
	}
	// Get the current rules (should be trs2).
	{
		rules, generation, err := fi.GetRules(context.Background())
		if err != nil {
			t.Errorf("GetRules error: %s", err)
		}
		validateGetRulesResult(t, rules, trs2)
		if got, notWant := generation, lastGeneration; got == notWant {
			t.Errorf("got result.Response.Generation = %d (want = not %d)", got, notWant)
		}
	}
}
