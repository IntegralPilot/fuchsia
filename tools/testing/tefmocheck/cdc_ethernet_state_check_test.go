// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"testing"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

func TestCdcEthernetStateCheck(t *testing.T) {
	tests := []struct {
		name          string
		log           []byte
		expectFailure bool
		expectedErr   string
		requiredTags  map[string]string
		tags          []string
		testSummary   *runtests.TestSummary
	}{
		{
			name: "success",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: [NamingRule { matchers: {DeviceClasses([Ethernet])}, naming_scheme: [Static { value: "fnp" }, Default] }]
2026-03-30 16:37:21.732324 [00010.288] 28117:40089> [netstack] INFO: created interface Ethernet(3=>fnpethx43)
2026-03-30 16:37:22.472457 [00010.579] 28117:40108> [netstack] INFO: updated core state to ipv4_enabled=true, ipv6_enabled=true on Ethernet(3=>fnpethx43), prev v4=false,v6=false
2026-03-30 16:37:22.478807 [00010.581] 21353:21355> [netcfg] INFO: starting DHCPv4 client for fnpethx43 (id=3)
2026-03-30 16:37:23.713379 [00015.535] 28117:40096> [netstack] INFO: adding addr AddrSubnet { addr: 192.168.236.10, subnet: 192.168.236.0/24 } config Ipv4AddrConfig
`),
			expectFailure: false,
		},
		{
			name: "fails to create ethernet",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: [NamingRule { matchers: {DeviceClasses([Ethernet])}, naming_scheme: [Static { value: "fnp" }, Default] }]
`),
			expectFailure: true,
			expectedErr:   "cdc ethernet interface never created",
		},
		{
			name: "fails to online ethernet",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: [NamingRule { matchers: {DeviceClasses([Ethernet])}, naming_scheme: [Static { value: "fnp" }, Default] }]
2026-03-30 16:37:21.732324 [00010.288] 28117:40089> [netstack] INFO: created interface Ethernet(3=>fnpethx43)
`),
			expectFailure: true,
			expectedErr:   "cdc ethernet interface never came online",
		},
		{
			name: "fails to start dhcp",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: [NamingRule { matchers: {DeviceClasses([Ethernet])}, naming_scheme: [Static { value: "fnp" }, Default] }]
2026-03-30 16:37:21.732324 [00010.288] 28117:40089> [netstack] INFO: created interface Ethernet(3=>fnpethx43)
2026-03-30 16:37:22.472457 [00010.579] 28117:40108> [netstack] INFO: updated core state to ipv4_enabled=true, ipv6_enabled=true on Ethernet(3=>fnpethx43), prev v4=false,v6=false
`),
			expectFailure: true,
			expectedErr:   "cdc ethernet DHCP client never started",
		},
		{
			name: "fails to acquire IP",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: [NamingRule { matchers: {DeviceClasses([Ethernet])}, naming_scheme: [Static { value: "fnp" }, Default] }]
2026-03-30 16:37:21.732324 [00010.288] 28117:40089> [netstack] INFO: created interface Ethernet(3=>fnpethx43)
2026-03-30 16:37:22.472457 [00010.579] 28117:40108> [netstack] INFO: updated core state to ipv4_enabled=true, ipv6_enabled=true on Ethernet(3=>fnpethx43), prev v4=false,v6=false
2026-03-30 16:37:22.478807 [00010.581] 21353:21355> [netcfg] INFO: starting DHCPv4 client for fnpethx43 (id=3)
`),
			expectFailure: true,
			expectedErr:   "cdc ethernet never acquired an IP address",
		},
		{
			name: "fails if prefix is empty and driver found",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: []
2026-03-30 16:37:21.732324 [00010.288] 28117:40089> [netstack] INFO: created interface Ethernet(3=>enx00)
`),
			expectFailure: true,
			expectedErr:   "cdc ethernet interface never created",
		},
		{
			name: "success with empty naming policy and fallback",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: []
2026-03-30 16:37:21.732324 [00010.288] 28117:40089> [netstack] INFO: created interface Ethernet(3=>ethx43)
2026-03-30 16:37:22.472457 [00010.579] 28117:40108> [netstack] INFO: updated core state to ipv4_enabled=true, ipv6_enabled=true on Ethernet(3=>ethx43), prev v4=false,v6=false
2026-03-30 16:37:22.478807 [00010.581] 21353:21355> [netcfg] INFO: starting DHCPv4 client for ethx43 (id=3)
2026-03-30 16:37:23.713379 [00015.535] 28117:40096> [netstack] INFO: adding addr AddrSubnet { addr: 192.168.236.10, subnet: 192.168.236.0/24 } config Ipv4AddrConfig
`),
			expectFailure: false,
		},
		{
			name: "passes if driver not found and no interface created",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: [NamingRule { matchers: {DeviceClasses([Ethernet])}, naming_scheme: [Static { value: "fnp" }, Default] }]
`),
			expectFailure: false,
		},
		{
			name: "passes if driver not found but interface completes successfully",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: [NamingRule { matchers: {DeviceClasses([Ethernet])}, naming_scheme: [Static { value: "fnp" }, Default] }]
2026-03-30 16:37:21.732324 [00010.288] 28117:40089> [netstack] INFO: created interface Ethernet(3=>fnpethx43)
2026-03-30 16:37:22.472457 [00010.579] 28117:40108> [netstack] INFO: updated core state to ipv4_enabled=true, ipv6_enabled=true on Ethernet(3=>fnpethx43), prev v4=false,v6=false
2026-03-30 16:37:22.478807 [00010.581] 21353:21355> [netcfg] INFO: starting DHCPv4 client for fnpethx43 (id=3)
2026-03-30 16:37:23.713379 [00015.535] 28117:40096> [netstack] INFO: adding addr AddrSubnet { addr: 192.168.236.10, subnet: 192.168.236.0/24 } config Ipv4AddrConfig
`),
			expectFailure: false,
		},
		{
			name: "fails on reboot if first boot failed",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
2026-03-30 16:37:18.315839 [00007.375] 21353:21355> [netcfg] INFO: using naming policy: [NamingRule { matchers: {DeviceClasses([Ethernet])}, naming_scheme: [Static { value: "fnp" }, Default] }]
2026-03-30 16:37:21.732324 [00010.288] 28117:40089> [netstack] INFO: created interface Ethernet(3=>fnpethx43)
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
`),
			expectFailure: true,
			expectedErr:   "cdc ethernet interface never came online",
		},
		{
			name: "skip when tags don't match",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
`),
			expectFailure: false,
			requiredTags: map[string]string{
				"device_type": "Sorrel",
				"product":     "vendor/google/products/fuchsia_internal.gni",
			},
			tags: []string{"device_type:Sorrel", "product:vendor/google/products/other.gni"},
		},
		{
			name: "run when tags match and fails",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
`),
			expectFailure: true,
			expectedErr:   "cdc ethernet interface never created",
			requiredTags: map[string]string{
				"device_type": "Sorrel",
				"product":     "vendor/google/products/fuchsia_internal.gni",
			},
			tags: []string{"device_type:Sorrel", "product:vendor/google/products/fuchsia_internal.gni"},
		},
		{
			name: "pass on truncated prefix output",
			log: []byte(`
[00001.000] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
[00010.296] 21413:21415> [netcfg] INFO: using naming policy: [NamingRule { matchers: {DeviceClasses([Ethernet])}, naming_scheme: [Static { value: "fnp" }, Default] }, NamingRule { matchers: {DeviceClasses([WlanClient])}, naming_scheme: [Static { value: "w
lan" }] }]
[netstack] INFO: created interface Ethernet(3=>fnpethx69)
[netstack] INFO: updated core state to ipv4_enabled=true, ipv6_enabled=true on Ethernet(3=>fnpethx69), prev v4=false,v6=false
[netcfg] INFO: starting DHCPv4 client for fnpethx69 (id=3)
[netstack] INFO: adding addr AddrSubnet { addr: 192.168.234.10, subnet: 192.168.234.0/24 } config Ipv4AddrConfig { config: CommonAddressConfig
`),
			expectFailure: false,
		},
		{
			name: "skip when all tests passed",
			log: []byte(`
2026-03-30 16:37:05.323213 physboot: Finding kernel package zircon...
2026-03-30 16:37:16.544647 [00006.605] 03977:03979> [driver] INFO: Found driver: fuchsia-pkg://fuchsia.com/usb-cdc-function#meta/usb-cdc-function.cm
`),
			expectFailure: false,
			testSummary: &runtests.TestSummary{
				Tests: []runtests.TestDetails{
					{
						Name:   "test1",
						Status: runtests.TestSuccess,
					},
				},
			},
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			check := &CdcEthernetStateCheck{
				RequiredTags: tc.requiredTags,
			}
			to := &TestingOutputs{
				SerialLogs:  [][]byte{tc.log},
				TestSummary: tc.testSummary,
			}
			if len(tc.tags) > 0 {
				to.SwarmingSummary = &SwarmingTaskSummary{
					Results: &SwarmingRpcsTaskResult{
						Tags: tc.tags,
					},
				}
			}
			failed := check.Check(to)
			if failed != tc.expectFailure {
				t.Fatalf("expected failure = %v, got = %v", tc.expectFailure, failed)
			}
			if tc.expectFailure && check.FailureReason() != tc.expectedErr {
				t.Fatalf("expected check error %q, got %q", tc.expectedErr, check.FailureReason())
			}
		})
	}
}
