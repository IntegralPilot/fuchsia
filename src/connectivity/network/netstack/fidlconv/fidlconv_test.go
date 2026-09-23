// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlconv

import (
	"fmt"
	"net"
	"testing"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routetypes"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/util"

	fnet "fidl/fuchsia/net"
	fnetRoutes "fidl/fuchsia/net/routes"
	"fidl/fuchsia/net/stack"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/header"
)

// TODO(tkilbourn): Consider moving more of these tests to "table-driven" tests.
// This is challenging because of the way FIDL unions are constructed in Go.

func TestNetIPtoTCPIPAddressIPv4(t *testing.T) {
	from := fnet.IpAddress{}
	from.SetIpv4(fnet.Ipv4Address{Addr: [4]uint8{127, 0, 0, 1}})
	to := ToTCPIPAddress(from)
	expected := util.Parse("127.0.0.1")
	if to != expected {
		t.Fatalf("Expected:\n %v\nActual:\n %v", expected, to)
	}
}

func TestNetIPtoTCPIPAddressIPv6(t *testing.T) {
	from := fnet.IpAddress{}
	from.SetIpv6(fnet.Ipv6Address{Addr: [16]uint8{0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1}})
	to := ToTCPIPAddress(from)
	expected := util.Parse("fe80::1")
	if to != expected {
		t.Fatalf("Expected:\n %v\nActual:\n %v", expected, to)
	}
}

func TestToFIDLIPAddressIPv4(t *testing.T) {
	from := util.Parse("127.0.0.1")
	to := ToNetIpAddress(from)
	expected := fnet.IpAddress{}
	expected.SetIpv4(fnet.Ipv4Address{Addr: [4]uint8{127, 0, 0, 1}})
	if to != expected {
		t.Fatalf("Expected:\n %v\nActual:\n %v", expected, to)
	}
}

func TestToFIDLIPAddressIPv6(t *testing.T) {
	from := util.Parse("fe80::1")
	to := ToNetIpAddress(from)
	expected := fnet.IpAddress{}
	expected.SetIpv6(fnet.Ipv6Address{Addr: [16]uint8{0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1}})
	if to != expected {
		t.Fatalf("Expected:\n %v\nActual:\n %v", expected, to)
	}
}

func TestToFIDLIPAddressEmptyInvalid(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Fatalf("Expected to fail on invalid address length")
		}
	}()

	var from tcpip.Address
	ToNetIpAddress(from)
	t.Errorf("Expected to fail on invalid address length")
}

func TestToTCPIPSubnet(t *testing.T) {
	cases := []struct {
		addr     [4]uint8
		prefix   uint8
		expected string
	}{
		{[4]uint8{255, 255, 255, 255}, 32, "255.255.255.255/32"},
		{[4]uint8{255, 255, 255, 254}, 31, "255.255.255.254/31"},
		{[4]uint8{255, 255, 255, 0}, 24, "255.255.255.0/24"},
		{[4]uint8{255, 0, 0, 0}, 8, "255.0.0.0/8"},
		{[4]uint8{128, 0, 0, 0}, 1, "128.0.0.0/1"},
		{[4]uint8{0, 0, 0, 0}, 0, "0.0.0.0/0"},
	}
	for _, testCase := range cases {
		netSubnet := fnet.Subnet{
			PrefixLen: testCase.prefix,
		}
		netSubnet.Addr.SetIpv4(fnet.Ipv4Address{Addr: testCase.addr})
		to := ToTCPIPSubnet(netSubnet)
		_, ipNet, err := net.ParseCIDR(testCase.expected)
		if err != nil {
			t.Fatalf("Error creating tcpip.Subnet: %v", err)
		}
		expected, err := tcpip.NewSubnet(tcpip.AddrFromSlice(ipNet.IP), tcpip.MaskFromBytes(ipNet.Mask))
		if err != nil {
			t.Fatal(err)
		}
		if to != expected {
			t.Errorf("got = %s, want = %s", to, expected)
		}
	}
}

func TestForwardingEntryAndTcpipRouteConversions(t *testing.T) {
	var (
		gateway = tcpip.AddrFromSlice([]byte("efghijklmnopqrst"))
		nextHop = ToNetIpAddress(gateway)
	)

	destination, err := tcpip.NewSubnet(util.Parse("171.205.0.0"), util.ParseMask("255.255.224.0"))
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		dest func(*stack.ForwardingEntry)
		want tcpip.Route
	}{
		{
			dest: func(fe *stack.ForwardingEntry) {
				fe.DeviceId = 789
			},
			want: tcpip.Route{
				Destination: destination,
				NIC:         789,
			},
		},
		{
			dest: func(fe *stack.ForwardingEntry) {
				nextHop := nextHop
				fe.NextHop = &nextHop
			},
			want: tcpip.Route{
				Destination: destination,
				Gateway:     gateway,
			},
		},
		{
			dest: func(fe *stack.ForwardingEntry) {
				fe.DeviceId = 789
				nextHop := nextHop
				fe.NextHop = &nextHop
			},
			want: tcpip.Route{
				Destination: destination,
				Gateway:     gateway,
				NIC:         789,
			},
		},
	} {
		fe := stack.ForwardingEntry{
			Subnet: fnet.Subnet{
				Addr:      ToNetIpAddress(destination.ID()),
				PrefixLen: 19,
			},
		}
		tc.dest(&fe)
		got := ForwardingEntryToTCPIPRoute(fe)
		if got != tc.want {
			t.Errorf("got ForwardingEntryToTCPIPRoute(%v) = %v, want = %v", fe, got, tc.want)
		}
		roundtripFe := TCPIPRouteToForwardingEntry(got)
		if diff := cmp.Diff(roundtripFe, fe, cmpopts.IgnoreTypes(struct{}{})); diff != "" {
			t.Fatalf("forwarding entry mismatch (-want +got):\n%s", diff)
		}
	}
}

func TestToNetSocketAddress(t *testing.T) {
	tests := []struct {
		name     string
		fullAddr tcpip.FullAddress
		sockAddr fnet.SocketAddress
	}{
		{
			name: "IPv4",
			fullAddr: tcpip.FullAddress{
				Addr: util.Parse("192.168.0.1"),
				Port: 8080,
			},
			sockAddr: func() fnet.SocketAddress {
				var a fnet.SocketAddress
				a.SetIpv4(fnet.Ipv4SocketAddress{
					Address: fnet.Ipv4Address{
						Addr: [4]uint8{192, 168, 0, 1},
					},
					Port: 8080,
				})
				return a
			}(),
		},
		{
			name: "IPv6 Global without NIC",
			fullAddr: tcpip.FullAddress{
				Addr: util.Parse("2001:4860:4860::8888"),
				Port: 8080,
			},
			sockAddr: func() fnet.SocketAddress {
				var a fnet.SocketAddress
				a.SetIpv6(fnet.Ipv6SocketAddress{
					Address: fnet.Ipv6Address{
						Addr: [16]uint8{0x20, 0x01, 0x48, 0x60, 0x48, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x88, 0x88},
					},
					Port: 8080,
				})
				return a
			}(),
		},
		{
			name: "IPv6 Global with NIC",
			fullAddr: tcpip.FullAddress{
				Addr: util.Parse("2001:4860:4860::8888"),
				Port: 8080,
				NIC:  2,
			},
			sockAddr: func() fnet.SocketAddress {
				var a fnet.SocketAddress
				a.SetIpv6(fnet.Ipv6SocketAddress{
					Address: fnet.Ipv6Address{
						Addr: [16]uint8{0x20, 0x01, 0x48, 0x60, 0x48, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x88, 0x88},
					},
					Port: 8080,
				})
				return a
			}(),
		},
		{
			name: "IPv6 LinkLocal Unicast with NIC",
			fullAddr: tcpip.FullAddress{
				Addr: util.Parse("fe80:4860:4860::8888"),
				Port: 8080,
				NIC:  2,
			},
			sockAddr: func() fnet.SocketAddress {
				var a fnet.SocketAddress
				a.SetIpv6(fnet.Ipv6SocketAddress{
					Address: fnet.Ipv6Address{
						Addr: [16]uint8{0xfe, 0x80, 0x48, 0x60, 0x48, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x88, 0x88},
					},
					Port:      8080,
					ZoneIndex: 2,
				})
				return a
			}(),
		},
		{
			name: "IPv6 LinkLocal Multicast with NIC",
			fullAddr: tcpip.FullAddress{
				Addr: util.Parse("ff02:4860:4860::8888"),
				Port: 8080,
				NIC:  2,
			},
			sockAddr: func() fnet.SocketAddress {
				var a fnet.SocketAddress
				a.SetIpv6(fnet.Ipv6SocketAddress{
					Address: fnet.Ipv6Address{
						Addr: [16]uint8{0xff, 0x02, 0x48, 0x60, 0x48, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x88, 0x88},
					},
					Port:      8080,
					ZoneIndex: 2,
				})
				return a
			}(),
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if converted := ToNetSocketAddress(test.fullAddr); test.sockAddr != converted {
				t.Errorf("got ToNetSocketAddress(%+v) = %+v, want = %+v", test.fullAddr, converted, test.sockAddr)
			}
		})
	}
}

func TestBytesToAddressDroppingUnspecified(t *testing.T) {
	tests := []struct {
		name  string
		bytes []uint8
		addr  tcpip.Address
	}{
		{
			name:  "IPv4",
			bytes: []uint8{192, 0, 2, 1},
			addr:  util.Parse("192.0.2.1"),
		},
		{
			name:  "IPv4 Unspecified",
			bytes: []uint8{0, 0, 0, 0},
			addr:  tcpip.Address{},
		},
		{
			name:  "IPv6",
			bytes: []uint8{0x20, 0x01, 0xdb, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01},
			addr:  util.Parse("2001:db80::1"),
		},
		{
			name:  "IPv6 Unspecified",
			bytes: []uint8{0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
			addr:  tcpip.Address{},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := BytesToAddressDroppingUnspecified(test.bytes); got != test.addr {
				t.Errorf("got BytesToAddressDroppingUnspecified(%+v) = %+v, want %+v", test.bytes, got, test.addr)
			}
		})
	}
}

func TestToInstalledRoute(t *testing.T) {
	makeSubnet := func(address tcpip.Address, mask tcpip.AddressMask) tcpip.Subnet {
		subnet, err := tcpip.NewSubnet(address, mask)
		if err != nil {
			t.Errorf("failed to create subnet from address %+v with mask %+v", address, mask)
		}
		return subnet
	}

	const (
		interfaceId  = 1
		metric       = 100
		subnetV4Hex  = "\xC0\xA8\x00\x00"
		maskV4Hex    = "\xFF\xFF\xFF\x00"
		prefixLenV4  = 24
		gatewayV4Hex = "\xC0\xA8\x00\x01"
		subnetV6Hex  = "\xFE\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00"
		maskV6Hex    = "\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x00\x00\x00\x00\x00\x00\x00\x00"
		prefixLenV6  = 64
		gatewayV6Hex = "\xFE\x80\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01"
	)
	// Note Golang slices can't be const.
	subnetV4Bytes := [4]uint8{192, 168, 0, 0}
	gatewayV4Bytes := [4]uint8{192, 168, 0, 1}
	subnetV6Bytes := [16]uint8{254, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0}
	gatewayV6Bytes := [16]uint8{254, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1}

	tests := []struct {
		name           string
		extendedRoute  routetypes.ExtendedRoute
		installedRoute InstalledRoute
	}{
		{
			name: "IPv4",
			extendedRoute: routetypes.ExtendedRoute{
				Route: tcpip.Route{
					Destination: makeSubnet(
						tcpip.AddrFromSlice([]byte(subnetV4Hex)),
						tcpip.MaskFromBytes([]byte(maskV4Hex))),
					Gateway: tcpip.AddrFromSlice([]byte(gatewayV4Hex)),
					NIC:     interfaceId,
				},
				Metric:                metric,
				MetricTracksInterface: false,
			},
			installedRoute: InstalledRoute{
				Version: routetypes.IPv4,
				V4: fnetRoutes.InstalledRouteV4{
					RoutePresent: true,
					Route: fnetRoutes.RouteV4{
						Destination: fnet.Ipv4AddressWithPrefix{
							Addr: fnet.Ipv4Address{
								Addr: subnetV4Bytes,
							},
							PrefixLen: prefixLenV4,
						},
						Action: fnetRoutes.RouteActionV4WithForward(fnetRoutes.RouteTargetV4{
							OutboundInterface: interfaceId,
							NextHop: &fnet.Ipv4Address{
								Addr: gatewayV4Bytes,
							},
						}),
						Properties: fnetRoutes.RoutePropertiesV4{
							SpecifiedPropertiesPresent: true,
							SpecifiedProperties: fnetRoutes.SpecifiedRouteProperties{
								MetricPresent: true,
								Metric:        fnetRoutes.SpecifiedMetricWithExplicitMetric(metric),
							},
						},
					},
					EffectivePropertiesPresent: true,
					EffectiveProperties: fnetRoutes.EffectiveRouteProperties{
						MetricPresent: true,
						Metric:        metric,
					},
					TableIdPresent: true,
					TableId:        V4MainRouteTableId,
				},
			},
		},
		{
			name: "IPv4 NoGateway",
			extendedRoute: routetypes.ExtendedRoute{
				Route: tcpip.Route{
					Destination: makeSubnet(
						tcpip.AddrFromSlice([]byte(subnetV4Hex)),
						tcpip.MaskFromBytes([]byte(maskV4Hex))),
					NIC: interfaceId,
				},
				Metric:                metric,
				MetricTracksInterface: false,
			},
			installedRoute: InstalledRoute{
				Version: routetypes.IPv4,
				V4: fnetRoutes.InstalledRouteV4{
					RoutePresent: true,
					Route: fnetRoutes.RouteV4{
						Destination: fnet.Ipv4AddressWithPrefix{
							Addr: fnet.Ipv4Address{
								Addr: subnetV4Bytes,
							},
							PrefixLen: prefixLenV4,
						},
						Action: fnetRoutes.RouteActionV4WithForward(fnetRoutes.RouteTargetV4{
							OutboundInterface: interfaceId,
							NextHop:           nil,
						}),
						Properties: fnetRoutes.RoutePropertiesV4{
							SpecifiedPropertiesPresent: true,
							SpecifiedProperties: fnetRoutes.SpecifiedRouteProperties{
								MetricPresent: true,
								Metric:        fnetRoutes.SpecifiedMetricWithExplicitMetric(metric),
							},
						},
					},
					EffectivePropertiesPresent: true,
					EffectiveProperties: fnetRoutes.EffectiveRouteProperties{
						MetricPresent: true,
						Metric:        metric,
					},
					TableIdPresent: true,
					TableId:        V4MainRouteTableId,
				},
			},
		},
		{
			name: "IPv4 Metric Tracks Interface",
			extendedRoute: routetypes.ExtendedRoute{
				Route: tcpip.Route{
					Destination: makeSubnet(
						tcpip.AddrFromSlice([]byte(subnetV4Hex)),
						tcpip.MaskFromBytes([]byte(maskV4Hex))),
					Gateway: tcpip.AddrFromSlice([]byte(gatewayV4Hex)),
					NIC:     interfaceId,
				},
				Metric:                metric,
				MetricTracksInterface: true,
			},
			installedRoute: InstalledRoute{
				Version: routetypes.IPv4,
				V4: fnetRoutes.InstalledRouteV4{
					RoutePresent: true,
					Route: fnetRoutes.RouteV4{
						Destination: fnet.Ipv4AddressWithPrefix{
							Addr: fnet.Ipv4Address{
								Addr: subnetV4Bytes,
							},
							PrefixLen: prefixLenV4,
						},
						Action: fnetRoutes.RouteActionV4WithForward(fnetRoutes.RouteTargetV4{
							OutboundInterface: interfaceId,
							NextHop: &fnet.Ipv4Address{
								Addr: gatewayV4Bytes,
							},
						}),
						Properties: fnetRoutes.RoutePropertiesV4{
							SpecifiedPropertiesPresent: true,
							SpecifiedProperties: fnetRoutes.SpecifiedRouteProperties{
								MetricPresent: true,
								Metric: fnetRoutes.SpecifiedMetricWithInheritedFromInterface(
									fnetRoutes.Empty{},
								),
							},
						},
					},
					EffectivePropertiesPresent: true,
					EffectiveProperties: fnetRoutes.EffectiveRouteProperties{
						MetricPresent: true,
						Metric:        metric,
					},
					TableIdPresent: true,
					TableId:        V4MainRouteTableId,
				},
			},
		},
		{
			name: "IPv6",
			extendedRoute: routetypes.ExtendedRoute{
				Route: tcpip.Route{
					Destination: makeSubnet(
						tcpip.AddrFromSlice([]byte(subnetV6Hex)),
						tcpip.MaskFromBytes([]byte(maskV6Hex))),
					Gateway: tcpip.AddrFromSlice([]byte(gatewayV6Hex)),
					NIC:     interfaceId,
				},
				Metric:                metric,
				MetricTracksInterface: false,
			},
			installedRoute: InstalledRoute{
				Version: routetypes.IPv6,
				V6: fnetRoutes.InstalledRouteV6{
					RoutePresent: true,
					Route: fnetRoutes.RouteV6{
						Destination: fnet.Ipv6AddressWithPrefix{
							Addr: fnet.Ipv6Address{
								Addr: subnetV6Bytes,
							},
							PrefixLen: prefixLenV6,
						},
						Action: fnetRoutes.RouteActionV6WithForward(fnetRoutes.RouteTargetV6{
							OutboundInterface: interfaceId,
							NextHop: &fnet.Ipv6Address{
								Addr: gatewayV6Bytes,
							},
						}),
						Properties: fnetRoutes.RoutePropertiesV6{
							SpecifiedPropertiesPresent: true,
							SpecifiedProperties: fnetRoutes.SpecifiedRouteProperties{
								MetricPresent: true,
								Metric:        fnetRoutes.SpecifiedMetricWithExplicitMetric(metric),
							},
						},
					},
					EffectivePropertiesPresent: true,
					EffectiveProperties: fnetRoutes.EffectiveRouteProperties{
						MetricPresent: true,
						Metric:        metric,
					},
					TableIdPresent: true,
					TableId:        V6MainRouteTableId,
				},
			},
		},
		{
			name: "IPv6 No Gateway",
			extendedRoute: routetypes.ExtendedRoute{
				Route: tcpip.Route{
					Destination: makeSubnet(
						tcpip.AddrFromSlice([]byte(subnetV6Hex)),
						tcpip.MaskFromBytes([]byte(maskV6Hex))),
					NIC: interfaceId,
				},
				Metric:                metric,
				MetricTracksInterface: false,
			},
			installedRoute: InstalledRoute{
				Version: routetypes.IPv6,
				V6: fnetRoutes.InstalledRouteV6{
					RoutePresent: true,
					Route: fnetRoutes.RouteV6{
						Destination: fnet.Ipv6AddressWithPrefix{
							Addr: fnet.Ipv6Address{
								Addr: subnetV6Bytes,
							},
							PrefixLen: prefixLenV6,
						},
						Action: fnetRoutes.RouteActionV6WithForward(fnetRoutes.RouteTargetV6{
							OutboundInterface: interfaceId,
							NextHop:           nil,
						}),
						Properties: fnetRoutes.RoutePropertiesV6{
							SpecifiedPropertiesPresent: true,
							SpecifiedProperties: fnetRoutes.SpecifiedRouteProperties{
								MetricPresent: true,
								Metric:        fnetRoutes.SpecifiedMetricWithExplicitMetric(metric),
							},
						},
					},
					EffectivePropertiesPresent: true,
					EffectiveProperties: fnetRoutes.EffectiveRouteProperties{
						MetricPresent: true,
						Metric:        metric,
					},
					TableIdPresent: true,
					TableId:        V6MainRouteTableId,
				},
			},
		},
		{
			name: "IPv6 Metric Tracks Interface",
			extendedRoute: routetypes.ExtendedRoute{
				Route: tcpip.Route{
					Destination: makeSubnet(
						tcpip.AddrFromSlice([]byte(subnetV6Hex)),
						tcpip.MaskFromBytes([]byte(maskV6Hex))),
					Gateway: tcpip.AddrFromSlice([]byte(gatewayV6Hex)),
					NIC:     interfaceId,
				},
				Metric:                metric,
				MetricTracksInterface: true,
			},
			installedRoute: InstalledRoute{
				Version: routetypes.IPv6,
				V6: fnetRoutes.InstalledRouteV6{
					RoutePresent: true,
					Route: fnetRoutes.RouteV6{
						Destination: fnet.Ipv6AddressWithPrefix{
							Addr: fnet.Ipv6Address{
								Addr: subnetV6Bytes,
							},
							PrefixLen: prefixLenV6,
						},
						Action: fnetRoutes.RouteActionV6WithForward(fnetRoutes.RouteTargetV6{
							OutboundInterface: interfaceId,
							NextHop: &fnet.Ipv6Address{
								Addr: gatewayV6Bytes,
							},
						}),
						Properties: fnetRoutes.RoutePropertiesV6{
							SpecifiedPropertiesPresent: true,
							SpecifiedProperties: fnetRoutes.SpecifiedRouteProperties{
								MetricPresent: true,
								Metric: fnetRoutes.SpecifiedMetricWithInheritedFromInterface(
									fnetRoutes.Empty{},
								),
							},
						},
					},
					EffectivePropertiesPresent: true,
					EffectiveProperties: fnetRoutes.EffectiveRouteProperties{
						MetricPresent: true,
						Metric:        metric,
					},
					TableIdPresent: true,
					TableId:        V6MainRouteTableId,
				},
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got := ToInstalledRoute(test.extendedRoute)
			// `==` checks that pointers point to the same address, whereas
			// `cmp.Equal` checks the contents of those addresses.
			if !cmp.Equal(got, test.installedRoute) {
				t.Errorf(
					"got ToInstalledRoute(%+v) = %+v, want %+v",
					test.extendedRoute,
					got,
					test.installedRoute,
				)
			}
		})
	}

}

func TestToTCPIPSubnetChecked(t *testing.T) {
	ipv4 := func(a, b, c, d uint8) fnet.IpAddress {
		return fnet.IpAddressWithIpv4(fnet.Ipv4Address{
			Addr: [4]uint8{a, b, c, d},
		})
	}
	ipv6 := func(head []uint8, tail []uint8) fnet.IpAddress {
		addr := fnet.Ipv6Address{}
		if len(head)+len(tail) > header.IPv6AddressSize {
			panic(fmt.Sprintf("head %+v and tail %+v are too long to make an IPv6 address", head, tail))
		}
		for index, octet := range head {
			addr.Addr[index] = octet
		}
		for indexInTail, octet := range tail {
			index := indexInTail + header.IPv6AddressSize - len(tail)
			addr.Addr[index] = octet
		}
		return fnet.IpAddressWithIpv6(addr)
	}

	tests := []struct {
		name      string
		addr      fnet.IpAddress
		prefixLen uint8
		valid     bool
	}{
		{
			name:      "is valid for IPv4 address with valid prefix length",
			addr:      ipv4(192, 168, 1, 0),
			prefixLen: 24,
			valid:     true,
		},
		{
			name:      "is invalid for IPv4 address with host bits set",
			addr:      ipv4(192, 168, 1, 1),
			prefixLen: 24,
			valid:     false,
		},
		{
			name:      "is invalid for IPv4 address with invalid prefix length",
			addr:      ipv4(192, 168, 1, 1),
			prefixLen: 33,
			valid:     false,
		},
		{
			name:      "is valid for IPv6 address with valid prefix length",
			addr:      ipv6([]uint8{0xf}, []uint8{0}),
			prefixLen: 64,
			valid:     true,
		},
		{
			name:      "is invalid for IPv6 address with host bits set",
			addr:      ipv6([]uint8{0xf}, []uint8{0x1}),
			prefixLen: 64,
			valid:     false,
		},
		{
			name:      "is invalid for IPv6 address with invalid prefix length",
			addr:      ipv6([]uint8{0xf}, []uint8{0}),
			prefixLen: 129,
			valid:     false,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			subnet := fnet.Subnet{
				Addr:      test.addr,
				PrefixLen: test.prefixLen,
			}
			_, err := ToTCPIPSubnetChecked(subnet)
			if err != nil && test.valid {
				t.Errorf("got ToTCPIPSubnetChecked(%#v) = _, %s, want nil", subnet, err)
			}
			if err == nil && !test.valid {
				t.Errorf("got ToTCPIPSubnetChecked(%#v) = _, nil, want non-nil", subnet)
			}
		})
	}
}
