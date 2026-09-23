// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package targets

import (
	"context"
	"net"
	"reflect"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestAddressLogic(t *testing.T) {
	t.Run("percentage signs are escaped", func(t *testing.T) {
		inputs := []string{
			"[fe80::a019:b0ff:fe21:64bd%qemu]:40860",
			"[fe80::a019:b0ff:fe21:64bd%25qemu]:40860",
		}
		expected := "[fe80::a019:b0ff:fe21:64bd%25qemu]:40860"
		for _, input := range inputs {
			actual := escapePercentSign(input)
			if actual != expected {
				t.Errorf("failed to escape percentage sign:\nactual: %s\nexpected: %s", actual, expected)
			}
		}
	})

	t.Run("derivation of the local-scoped local host", func(t *testing.T) {
		inputs := []string{
			"[fe80::a019:b0ff:fe21:64bd%qemu]:40860",
			"[fe80::a019:b0ff:fe21:64bd%25qemu]:40860",
		}
		expected := "[fe80::a019:b0ff:fe21:64bd%25qemu]"
		for _, input := range inputs {
			actual := localScopedLocalHost(input)
			if actual != expected {
				t.Errorf("failed to derive host:\nactual: %s\nexpected: %s", actual, expected)
			}
		}
	})
}

func TestFromJSON(t *testing.T) {
	ctx := context.Background()

	tests := []struct {
		name              string
		obj               string
		expected          reflect.Type
		expectedEmuBinary string
	}{
		{
			name:              "derive aemu target",
			obj:               `{"type": "aemu", "target": "x64"}`,
			expected:          reflect.TypeOf(&Emulator{}),
			expectedEmuBinary: "emulator",
		},
		{
			name:              "derive qemu target",
			obj:               `{"type": "qemu", "target": "arm64"}`,
			expected:          reflect.TypeOf(&Emulator{}),
			expectedEmuBinary: "qemu-system-aarch64",
		},
		// Testing FromJSON for "device" and "gce" is complex given that
		// the constructor functions for those two targets perform a good amount
		// of side effects, for example, creating a "gce" target will try to
		// initialize a gce instance.
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			target, err := FromJSON(ctx, []byte(test.obj), Options{})
			if err != nil {
				t.Errorf("failed to derive target. err=%q", err)
			}
			if reflect.TypeOf(target) != test.expected {
				t.Errorf("expected target type %q, got %q", test.expected, reflect.TypeOf(target))
			}
			if emuTarget, ok := target.(*Emulator); ok {
				if emuTarget.binary != test.expectedEmuBinary {
					t.Errorf("expected binary name %q, got %q", test.expectedEmuBinary, emuTarget.binary)
				}
			}
		})
	}
}

// Test implementation of FuchsiaTarget using TestbedConfig as its implementation
// of TestConfig.
type testTarget struct {
	FuchsiaTarget
	nodename string
	serial   string
	pdu      *targetPDU
	monsoon  *targetMonsoon
	ipv4     net.IP
	ipv6     *net.IPAddr
}

func (t *testTarget) TestConfig(expectsSSH bool) (any, error) {
	return TestbedConfig(t, expectsSSH, &testbedConfigOptions{PDU: t.pdu, Monsoon: t.monsoon})
}
func (t *testTarget) IPv4() (net.IP, error)      { return t.ipv4, nil }
func (t *testTarget) IPv6() (*net.IPAddr, error) { return t.ipv6, nil }
func (t *testTarget) Nodename() string           { return t.nodename }
func (t *testTarget) SerialSocketPath() string   { return t.serial }
func (t *testTarget) SSHKey() string             { return "" }
func (t *testTarget) GetSharedData() string      { return "/tmp/shared_data" }

func TestTestbedConfig(t *testing.T) {
	tests := []struct {
		name       string
		target     testTarget
		expectsSSH bool
		want       FuchsiaTestbedConfig
	}{
		{
			name:       "valid",
			target:     testTarget{nodename: "node", serial: "serial", ipv4: net.IPv4zero, ipv6: &net.IPAddr{IP: net.IPv6zero}},
			expectsSSH: true,
			want:       FuchsiaTestbedConfig{Type: "FuchsiaDevice", Nodename: "node", SerialSocket: "serial", IPv4: net.IPv4zero.String(), IPv6: net.IPv6zero.String(), PDU: nil, SharedData: "/tmp/shared_data"},
		},
		{
			name:       "valid without SSH",
			target:     testTarget{nodename: "node", serial: "serial", ipv4: net.IPv4zero, ipv6: &net.IPAddr{IP: net.IPv6zero}},
			expectsSSH: false,
			want:       FuchsiaTestbedConfig{Type: "FuchsiaDevice", Nodename: "node", SerialSocket: "serial", PDU: nil},
		},
		{
			name:       "valid no ip addresses",
			target:     testTarget{nodename: "node", serial: "serial"},
			expectsSSH: true,
			want:       FuchsiaTestbedConfig{Type: "FuchsiaDevice", Nodename: "node", SerialSocket: "serial", PDU: nil, SharedData: "/tmp/shared_data"},
		},
		{
			name: "valid with pdu",
			target: testTarget{nodename: "node", serial: "serial", pdu: &targetPDU{
				IP:   "192.168.1.1",
				MAC:  "12:34:56:78:9a:bc",
				Port: 1,
			}},
			expectsSSH: true,
			want: FuchsiaTestbedConfig{Type: "FuchsiaDevice", Nodename: "node", SerialSocket: "serial", PDU: &targetPDU{
				IP:   "192.168.1.1",
				MAC:  "12:34:56:78:9a:bc",
				Port: 1,
			}, SharedData: "/tmp/shared_data"},
		},
		{
			name: "valid with monsoon",
			target: testTarget{nodename: "node", serial: "serial", monsoon: &targetMonsoon{
				Sernum: "12345",
			}},
			expectsSSH: true,
			want: FuchsiaTestbedConfig{Type: "FuchsiaDevice", Nodename: "node", SerialSocket: "serial", Monsoon: &targetMonsoon{
				Sernum: "12345",
			}, SharedData: "/tmp/shared_data"},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got, err := test.target.TestConfig(test.expectsSSH)
			if err != nil {
				t.Errorf("unexpected error from target.TestConfig(false): %s", err)
			}
			if diff := cmp.Diff(test.want, got); diff != "" {
				t.Errorf("TestConfig(false) mismatch (-want +got):\n%s", diff)
			}
		})
	}
}
