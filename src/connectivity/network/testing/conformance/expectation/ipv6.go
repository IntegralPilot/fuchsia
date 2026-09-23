// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var ipv6Expectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 2}:  Pass,
	{3, 1}:  Pass,
	{3, 2}:  Pass,
	{3, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	{3, 6}:  Pass,
	{3, 7}:  Pass,
	{3, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 9}:  Pass,
	{3, 10}: Pass,
	{4, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 3}:  Pass,
	{4, 4}:  Pass,
	{5, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 2}:  Pass,
	{5, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 4}:  Pass,
	{5, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 7}:  Pass,
	{5, 8}:  Pass,
	{5, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 10}: AnvlSkip, // Router test, but this is the host suite.
	{5, 11}: Pass,
	{5, 12}: AnvlSkip, // Router test, but this is the host suite.
	{5, 13}: AnvlSkip, // Router test, but this is the host suite.
	{5, 14}: Pass,
	{5, 15}: Pass,
	{5, 16}: AnvlSkip, // Router test, but this is the host suite.
	{5, 17}: Pass,
	{5, 18}: AnvlSkip, // Router test, but this is the host suite.
	{5, 19}: Pass,
	{5, 20}: AnvlSkip, // Router test, but this is the host suite.
	{5, 21}: AnvlSkip, // Router test, but this is the host suite.
	{5, 22}: Pass,
	{5, 23}: Pass,
	{5, 24}: AnvlSkip, // Router test, but this is the host suite.
	{5, 25}: AnvlSkip, // Router test, but this is the host suite.
	{5, 26}: Pass,
	{5, 27}: Pass,
	{5, 28}: AnvlSkip, // Router test, but this is the host suite.
	{5, 29}: Pass,
	{5, 30}: AnvlSkip, // Router test, but this is the host suite.
	{5, 31}: Pass,
	{5, 32}: AnvlSkip, // Router test, but this is the host suite.
	{6, 1}:  Pass,
	{6, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 3}:  Pass,
	{6, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 7}:  Pass,
	{6, 8}:  Pass,
	{6, 9}:  Pass,
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  AnvlSkip, // Router test, but this is the host suite.
	// This test is skipped due to support for RFC 5095 - Deprecation of
	// RH0 being enabled, and is essentially covered by test case 16.1 as
	// the only difference is the address in the routing option header
	// which is not material to what's being tested.
	{7, 4}: AnvlSkip,
	// This test is skipped due to support for RFC 5095 - Deprecation of
	// RH0 being enabled, and is essentially covered by test case 16.3 as
	// the only difference is the address in the routing option header
	// which is not material to what's being tested.
	{7, 5}:   AnvlSkip,
	{8, 1}:   Pass,
	{8, 2}:   Pass,
	{8, 3}:   Pass,
	{8, 4}:   Pass,
	{8, 5}:   Pass,
	{8, 6}:   Pass,
	{8, 7}:   Pass,
	{8, 8}:   Pass,
	{8, 9}:   Pass,
	{8, 10}:  Pass,
	{8, 11}:  Pass,
	{8, 12}:  Pass,
	{8, 13}:  Pass,
	{8, 14}:  Pass,
	{8, 15}:  Pass,
	{8, 16}:  Pass,
	{8, 17}:  Flaky,
	{8, 18}:  Pass,
	{8, 19}:  Fail,
	{9, 1}:   AnvlSkip, // Router test, but this is the host suite.
	{9, 2}:   AnvlSkip, // Router test, but this is the host suite.
	{9, 3}:   Pass,
	{9, 4}:   Pass,
	{10, 1}:  Pass,
	{11, 1}:  Pass,
	{11, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{11, 3}:  Pass,
	{11, 4}:  Pass,
	{11, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{11, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 1}:  Pass,
	{12, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 3}:  Pass,
	{12, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 5}:  Fail,
	{13, 1}:  Pass,
	{13, 2}:  Pass,
	{14, 1}:  Pass,
	{14, 2}:  Pass,
	{14, 3}:  Pass,
	{14, 4}:  Fail,
	{15, 1}:  Pass,
	{15, 2}:  Pass,
	{15, 3}:  Pass,
	{15, 4}:  Pass,
	{15, 5}:  Pass,
	{15, 6}:  Pass,
	{15, 7}:  Pass,
	{15, 8}:  Pass,
	{15, 9}:  Pass,
	{15, 10}: Pass,
	{15, 11}: Pass,
	{15, 12}: Pass,
	{15, 13}: Pass,
	{15, 14}: Pass,
	{15, 15}: Pass,
	{15, 16}: Pass,
	{15, 17}: Pass,
	{16, 1}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 2}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 3}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 4}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 5}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 6}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 7}:  Skip, // Test should only be run when DUT is a router so skip it here.
}

var ipv6ExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 2}:  Pass,
	{3, 1}:  Pass,
	{3, 2}:  Pass,
	{3, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 4}:  Fail,
	{3, 5}:  Fail,
	{3, 6}:  Pass,
	{3, 7}:  Fail,
	{3, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 9}:  Pass,
	{3, 10}: Pass,
	{4, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 3}:  Pass,
	{4, 4}:  Pass,
	{5, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 2}:  Pass,
	{5, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 4}:  Pass,
	{5, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 7}:  Pass,
	{5, 8}:  Pass,
	{5, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 10}: AnvlSkip, // Router test, but this is the host suite.
	{5, 11}: Pass,
	{5, 12}: AnvlSkip, // Router test, but this is the host suite.
	{5, 13}: AnvlSkip, // Router test, but this is the host suite.
	{5, 14}: Pass,
	{5, 15}: Pass,
	{5, 16}: AnvlSkip, // Router test, but this is the host suite.
	{5, 17}: Pass,
	{5, 18}: AnvlSkip, // Router test, but this is the host suite.
	{5, 19}: Pass,
	{5, 20}: AnvlSkip, // Router test, but this is the host suite.
	{5, 21}: AnvlSkip, // Router test, but this is the host suite.
	{5, 22}: Pass,
	{5, 23}: Pass,
	{5, 24}: AnvlSkip, // Router test, but this is the host suite.
	{5, 25}: AnvlSkip, // Router test, but this is the host suite.
	{5, 26}: Pass,
	{5, 27}: Pass,
	{5, 28}: AnvlSkip, // Router test, but this is the host suite.
	{5, 29}: Pass,
	{5, 30}: AnvlSkip, // Router test, but this is the host suite.
	{5, 31}: Pass,
	{5, 32}: AnvlSkip, // Router test, but this is the host suite.
	{6, 1}:  Pass,
	{6, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 3}:  Pass,
	{6, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 7}:  Pass,
	{6, 8}:  Pass,
	{6, 9}:  Pass,
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  AnvlSkip, // Router test, but this is the host suite.
	// This test is skipped due to support for RFC 5095 - Deprecation of
	// RH0 being enabled, and is essentially covered by test case 16.1 as
	// the only difference is the address in the routing option header
	// which is not material to what's being tested.
	{7, 4}: AnvlSkip,
	// This test is skipped due to support for RFC 5095 - Deprecation of
	// RH0 being enabled, and is essentially covered by test case 16.3 as
	// the only difference is the address in the routing option header
	// which is not material to what's being tested.
	{7, 5}:  AnvlSkip,
	{8, 1}:  Pass,
	{8, 2}:  Pass,
	{8, 3}:  Pass,
	{8, 4}:  Pass,
	{8, 5}:  Pass,
	{8, 6}:  Pass,
	{8, 7}:  Pass,
	{8, 8}:  Pass,
	{8, 9}:  Pass,
	{8, 10}: Pass,
	{8, 11}: Pass,
	{8, 12}: Fail,
	{8, 13}: Pass,
	{8, 14}: Pass,
	{8, 15}: Fail,
	{8, 16}: Fail,
	{8, 17}: Pass,
	{8, 18}: Pass,
	{8, 19}: Fail,
	{9, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 3}:  Pass,
	{9, 4}:  Pass,
	{10, 1}: Pass,
	{11, 1}: Pass,
	{11, 2}: AnvlSkip, // Router test, but this is the host suite.
	{11, 3}: Pass,
	{11, 4}: Pass,
	{11, 5}: AnvlSkip, // Router test, but this is the host suite.
	{11, 6}: AnvlSkip, // Router test, but this is the host suite.
	{12, 1}: Pass,
	{12, 2}: AnvlSkip, // Router test, but this is the host suite.
	{12, 3}: Pass,
	{12, 4}: AnvlSkip, // Router test, but this is the host suite.
	{12, 5}: Fail,
	{13, 1}: Pass,
	{13, 2}: Pass,
	{14, 1}: Pass,
	{14, 2}: Pass,
	{14, 3}: Pass,
	// NB: This test sends a valid Neighbor Soliciation inside of an Ethernet
	// Frame with an incorrect destination MAC address. It expects the netstack
	// to drop the packet without sending back a Neighbor Advertisement.
	// This type of L2 filtering would typically be performed in a device
	// driver, and as such, is out of scope for a Networking stack. Accepting,
	// and processing such packets is in-line with Linux behavior, and not in
	// violation of any RFC.
	{14, 4}:  Fail,
	{15, 1}:  Pass,
	{15, 2}:  Pass,
	{15, 3}:  Pass,
	{15, 4}:  Pass,
	{15, 5}:  Pass,
	{15, 6}:  Pass,
	{15, 7}:  Pass,
	{15, 8}:  Pass,
	{15, 9}:  Pass,
	{15, 10}: Pass,
	{15, 11}: Pass,
	{15, 12}: Pass,
	{15, 13}: Pass,
	{15, 14}: Pass,
	{15, 15}: Pass,
	{15, 16}: Pass,
	{15, 17}: Pass,
	{16, 1}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 2}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 3}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 4}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 5}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 6}:  Skip, // Test should only be run when DUT is a router so skip it here.
	{16, 7}:  Skip, // Test should only be run when DUT is a router so skip it here.
}
