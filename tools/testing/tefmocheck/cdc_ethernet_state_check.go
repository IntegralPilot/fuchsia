// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"bytes"
	"regexp"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

// CdcEthernetStateCheck checks whether the device failed to initialize the CDC ethernet link correctly via serial logs.
type CdcEthernetStateCheck struct {
	baseCheck
	// RequiredTags specifies tags that must be present on the swarming task for this check to run.
	RequiredTags  map[string]string
	failureReason string
}

type cdcEthState int

const (
	cdcStateNone cdcEthState = iota
	cdcStateStarted
	cdcStateCreated
	cdcStateOnline
	cdcStateDHCP
	cdcStateIP
)

type ethernetState struct {
	state  cdcEthState
	prefix string
}

func (s *ethernetState) reset() {
	s.state = cdcStateNone
	s.prefix = ""
}

func (s *ethernetState) check() (bool, string) {
	switch s.state {
	case cdcStateNone, cdcStateIP:
		return false, ""
	case cdcStateStarted:
		return true, "cdc ethernet interface never created"
	case cdcStateCreated:
		return true, "cdc ethernet interface never came online"
	case cdcStateOnline:
		return true, "cdc ethernet DHCP client never started"
	case cdcStateDHCP:
		return true, "cdc ethernet never acquired an IP address"
	}
	return false, ""
}

const (
	// LINT.IfChange(cdc_driver_component_name_tefmo)
	cdcComponentStr = "usb-cdc-function#meta/usb-cdc-function.cm"
	// LINT.ThenChange(//src/connectivity/ethernet/drivers/usb-cdc-function/BUILD.gn:cdc_driver_component_name_tefmo)

	// LINT.IfChange(netcfg_naming_policy_tefmo)
	namingPolicyStr = "using naming policy"
	// LINT.ThenChange(//src/connectivity/policy/netcfg/src/lib.rs:netcfg_naming_policy_tefmo)

	// LINT.IfChange(netstack_created_interface_tefmo)
	createdInterfaceStr = "created interface Ethernet"
	// LINT.ThenChange(//src/connectivity/network/netstack3/src/bindings/netdevice_worker.rs:netstack_created_interface_tefmo)

	// LINT.IfChange(netstack_ip_enabled_tefmo)
	ipEnabledStr = "ipv4_enabled=true, ipv6_enabled=true on Ethernet"
	// LINT.ThenChange(//src/connectivity/network/netstack3/src/bindings/interfaces_admin.rs:netstack_ip_enabled_tefmo)

	// LINT.IfChange(netcfg_dhcp_start_tefmo)
	dhcpStartStr = "starting DHCPv4 client for"
	// LINT.ThenChange(//src/connectivity/policy/netcfg/src/dhcpv4.rs:netcfg_dhcp_start_tefmo)

	// LINT.IfChange(netstack_ip_added_tefmo)
	ipAddedStr    = "adding addr AddrSubnet"
	ipv4ConfigStr = "Ipv4AddrConfig"
	// LINT.ThenChange(//src/connectivity/network/netstack3/core/ip/src/device.rs:netstack_ip_added_tefmo)
)

var (
	// LINT.IfChange(device_class_enum_tefmo)
	ethernetStaticRe = regexp.MustCompile(`DeviceClasses\(\[Ethernet\]\).*?naming_scheme:\s*\[Static\s*\{\s*value:\s*"([^"]+)"`)
	// LINT.ThenChange(//src/connectivity/policy/netcfg/src/lib.rs:device_class_enum_tefmo)
)

func (s *ethernetState) setPrefix(lineBytes []byte) {
	m := ethernetStaticRe.FindSubmatch(lineBytes)
	if len(m) > 1 {
		s.prefix = string(m[1])
	}
	// Always append "eth" as a fallback or suffix.
	// If the naming policy yielded no prefix, this ensures we still match "eth"
	// (avoiding "wlan") and distinguish from "not seen" (where prefix is still
	// empty).
	s.prefix += "eth"
}

func (c *CdcEthernetStateCheck) shouldRun(to *TestingOutputs) bool {
	if to.TestSummary != nil && len(to.TestSummary.Tests) > 0 {
		allPassed := true
		for _, test := range to.TestSummary.Tests {
			if test.Status != runtests.TestSuccess {
				allPassed = false
				break
			}
		}
		if allPassed {
			return false
		}
	}

	if len(c.RequiredTags) == 0 {
		return true
	}
	if to.SwarmingSummary == nil || to.SwarmingSummary.Results == nil {
		return false
	}
	// Extract tags into a map for easy lookup
	tags := make(map[string]string)
	for _, t := range to.SwarmingSummary.Results.Tags {
		parts := strings.SplitN(t, ":", 2)
		if len(parts) == 2 {
			tags[parts[0]] = parts[1]
		}
	}
	for k, v := range c.RequiredTags {
		if tags[k] != v {
			return false
		}
	}
	return true
}

func (c *CdcEthernetStateCheck) Check(to *TestingOutputs) bool {
	if !c.shouldRun(to) {
		return false
	}

	for _, serialLog := range to.SerialLogs {
		var state ethernetState

		// Split on newline to process log by line
		lines := bytes.Split(serialLog, []byte("\n"))
		for _, lineBytes := range lines {
			if bytes.Contains(lineBytes, []byte("physboot: Finding kernel package zircon...")) {
				// We've hit a reboot.
				if failed, reason := state.check(); failed {
					c.failureReason = reason
					return true
				}
				state.reset()
				continue
			}
			if bytes.Contains(lineBytes, []byte(cdcComponentStr)) {
				if state.state < cdcStateStarted {
					state.state = cdcStateStarted
				}
				continue
			}

			if bytes.Contains(lineBytes, []byte(namingPolicyStr)) {
				state.setPrefix(lineBytes)
				continue
			}

			// All subsequent checks require a prefix.
			if state.prefix == "" {
				continue
			}

			if bytes.Contains(lineBytes, []byte(createdInterfaceStr)) {
				if bytes.Contains(lineBytes, []byte("=>"+state.prefix)) {
					if state.state < cdcStateCreated {
						state.state = cdcStateCreated
					}
				}
				continue
			}

			// All subsequent checks require the interface to have been created.
			if state.state < cdcStateCreated {
				continue
			}

			if bytes.Contains(lineBytes, []byte(ipEnabledStr)) {
				if bytes.Contains(lineBytes, []byte("=>"+state.prefix)) {
					if state.state < cdcStateOnline {
						state.state = cdcStateOnline
					}
				}
				continue
			}

			if bytes.Contains(lineBytes, []byte(dhcpStartStr)) {
				if bytes.Contains(lineBytes, []byte(state.prefix)) {
					if state.state < cdcStateDHCP {
						state.state = cdcStateDHCP
					}
				}
				continue
			}
			if bytes.Contains(lineBytes, []byte(ipAddedStr)) && bytes.Contains(lineBytes, []byte(ipv4ConfigStr)) {
				if state.state < cdcStateIP {
					state.state = cdcStateIP
				}
				continue
			}
		}
		if failed, reason := state.check(); failed {
			c.failureReason = reason
			return true
		}
	}
	return false
}

func (c *CdcEthernetStateCheck) Name() string {
	return "cdc_ethernet_state"
}

func (c *CdcEthernetStateCheck) DebugText() string {
	return "The CDC Ethernet interface did not reach a healthy state with DHCP and an IP address. Check the serial logs for more details."
}

func (c *CdcEthernetStateCheck) FailureReason() string {
	return c.failureReason
}

func (c *CdcEthernetStateCheck) EmitSyntheticTestCase() bool {
	return true
}
