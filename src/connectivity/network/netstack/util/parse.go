// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package util

import (
	"net"

	"gvisor.dev/gvisor/pkg/tcpip"
)

func IsAny(a tcpip.Address) bool {
	// An empty address is not the same as ANY.
	if a.Len() == 0 {
		return false
	}
	for _, n := range a.AsSlice() {
		if n != 0 {
			return false
		}
	}
	return true
}

// Parse parses the string representation of an IPv4 or IPv6 address.
func Parse(src string) tcpip.Address {
	ip := net.ParseIP(src)
	if ip == nil {
		return tcpip.Address{}
	}
	if v4 := ip.To4(); v4 != nil {
		return tcpip.AddrFrom4Slice(v4)
	}
	return tcpip.AddrFrom16Slice(ip)
}

// ParseMask parses the string representation of an IPv4 or IPv6 mask address.
func ParseMask(src string) tcpip.AddressMask {
	ip := net.ParseIP(src)
	if ip == nil {
		return tcpip.AddressMask{}
	}
	if v4 := ip.To4(); v4 != nil {
		return tcpip.MaskFromBytes(v4)
	}
	return tcpip.MaskFromBytes(ip)
}

// PointSubnet creates a subnet which contains only the passed address.
func PointSubnet(a tcpip.Address) tcpip.Subnet {
	l := a.BitLen()
	subnet, err := tcpip.NewSubnet(a, tcpip.MaskFromBytes(net.CIDRMask(l, l)))
	if err != nil {
		panic(err)
	}
	return subnet
}
