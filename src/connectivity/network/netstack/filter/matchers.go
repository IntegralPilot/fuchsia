// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package filter

import (
	"fmt"

	"fidl/fuchsia/hardware/network"

	"gvisor.dev/gvisor/pkg/tcpip/header"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
)

var _ stack.Matcher = (*filterDisabledNICMatcher)(nil)

// filterDisabledNICMatcher is a stack.Matcher that keeps track of which
// NICs have filtering disabled.
//
// All NICs are considered to have filtering disabled until explicitly
// enabled.
type filterDisabledNICMatcher struct {
	enabledNics *enabledNicsCache
}

// Name implements stack.Matcher.
func (*filterDisabledNICMatcher) Name() string {
	return "filterDisabledNICMatcher"
}

// Match implements stack.Matcher.
func (d *filterDisabledNICMatcher) Match(hook stack.Hook, _ *stack.PacketBuffer, inNicName, outNicName string) (matches bool, hotdrop bool) {
	if inNicName != "" && d.enabledNics.nicDisabled(inNicName) {
		return true, false
	}
	if outNicName != "" && d.enabledNics.nicDisabled(outNicName) {
		return true, false
	}
	return false, false
}

var _ stack.Matcher = (*deviceClassNICMatcher)(nil)

type NICMatcherDirection uint32

const (
	_ NICMatcherDirection = iota
	NICMatcherDirectionInput
	NICMatcherDirectionOutput
)

// deviceClassNICMatcher is a stack.Matcher that matches on the device class
// of an incoming or outgoing interface.
type deviceClassNICMatcher struct {
	// enabledNics keeps track of the enabled NIC cache that provides the
	// matcher with the NIC's device class when available.
	enabledNics *enabledNicsCache
	// match is the port class the matcher matches on.
	match network.PortClass
	// direction controls if the input or output interface should be matched on.
	direction NICMatcherDirection
}

// Name implements stack.Matcher.
func (*deviceClassNICMatcher) Name() string {
	return "deviceClassNICMatcher"
}

// Match implements stack.Matcher.
func (d *deviceClassNICMatcher) Match(_ stack.Hook, _ *stack.PacketBuffer, inNicName, outNicName string) (matches bool, hotdrop bool) {
	var nicName string
	switch d := d.direction; d {
	case NICMatcherDirectionInput:
		nicName = inNicName
	case NICMatcherDirectionOutput:
		nicName = outNicName
	default:
		panic(fmt.Sprintf("invalid nic matcher direction %d", d))
	}
	if nicName != "" {
		PortClass := d.enabledNics.nicPortClass(nicName)
		return (PortClass != nil && *PortClass == d.match), false
	}
	return false, false
}

// portMatcher matches port to some range.
type portMatcher struct {
	start uint16
	end   uint16
}

func (m *portMatcher) match(port uint16) bool {
	return port >= m.start && port <= m.end
}

func NewTCPSourcePortMatcher(start, end uint16) *TCPSourcePortMatcher {
	return &TCPSourcePortMatcher{portMatcher: portMatcher{start, end}}
}

var _ stack.Matcher = (*TCPSourcePortMatcher)(nil)

// TCPSourcePortMatcher matches TCP packets and ports.
type TCPSourcePortMatcher struct {
	portMatcher
}

// Name implements stack.Matcher.
func (*TCPSourcePortMatcher) Name() string {
	return "TCPSourcePortMatcher"
}

// Match implements stack.Matcher.
func (m *TCPSourcePortMatcher) Match(_ stack.Hook, pkt *stack.PacketBuffer, _, _ string) (matches bool, hotdrop bool) {
	tcp := header.TCP(pkt.TransportHeader().Slice())
	if len(tcp) < header.TCPMinimumSize {
		// Drop immediately as the packet is invalid.
		return false, true
	}

	return m.portMatcher.match(tcp.SourcePort()), false
}

func NewTCPDestinationPortMatcher(start, end uint16) *TCPDestinationPortMatcher {
	return &TCPDestinationPortMatcher{portMatcher: portMatcher{start, end}}
}

var _ stack.Matcher = (*TCPDestinationPortMatcher)(nil)

// TCPDestinationPortMatcher matches TCP packets and ports.
type TCPDestinationPortMatcher struct {
	portMatcher
}

// Name implements stack.Matcher.
func (*TCPDestinationPortMatcher) Name() string {
	return "TCPDestinationPortMatcher"
}

// Match implements stack.Matcher.
func (m *TCPDestinationPortMatcher) Match(_ stack.Hook, pkt *stack.PacketBuffer, _, _ string) (matches bool, hotdrop bool) {
	tcp := header.TCP(pkt.TransportHeader().Slice())
	if len(tcp) < header.TCPMinimumSize {
		// Drop immediately as the packet is invalid.
		return false, true
	}

	return m.portMatcher.match(tcp.DestinationPort()), false
}

func NewUDPSourcePortMatcher(start, end uint16) *UDPSourcePortMatcher {
	return &UDPSourcePortMatcher{portMatcher: portMatcher{start, end}}
}

var _ stack.Matcher = (*UDPSourcePortMatcher)(nil)

// UDPSourcePortMatcher matches UDP packets and ports.
type UDPSourcePortMatcher struct {
	portMatcher
}

// Name implements stack.Matcher.
func (*UDPSourcePortMatcher) Name() string {
	return "UDPSourcePortMatcher"
}

// Match implements stack.Matcher.
func (m *UDPSourcePortMatcher) Match(_ stack.Hook, pkt *stack.PacketBuffer, _, _ string) (matches bool, hotdrop bool) {
	udp := header.UDP(pkt.TransportHeader().Slice())
	if len(udp) < header.UDPMinimumSize {
		// Drop immediately as the packet is invalid.
		return false, true
	}

	return m.portMatcher.match(udp.SourcePort()), false
}

func NewUDPDestinationPortMatcher(start, end uint16) *UDPDestinationPortMatcher {
	return &UDPDestinationPortMatcher{portMatcher: portMatcher{start, end}}
}

var _ stack.Matcher = (*UDPDestinationPortMatcher)(nil)

// UDPDestinationPortMatcher matches UDP packets and ports.
type UDPDestinationPortMatcher struct {
	portMatcher
}

// Name implements stack.Matcher.
func (*UDPDestinationPortMatcher) Name() string {
	return "UDPDestinationPortMatcher"
}

// Match implements stack.Matcher.
func (m *UDPDestinationPortMatcher) Match(_ stack.Hook, pkt *stack.PacketBuffer, _, _ string) (matches bool, hotdrop bool) {
	udp := header.UDP(pkt.TransportHeader().Slice())
	if len(udp) < header.UDPMinimumSize {
		// Drop immediately as the packet is invalid.
		return false, true
	}

	return m.portMatcher.match(udp.DestinationPort()), false
}
