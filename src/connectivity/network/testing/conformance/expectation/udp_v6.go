// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var udpV6Expectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{2, 1}:  Pass,
	{2, 2}:  Pass,
	{2, 3}:  Pass,
	{2, 4}:  Pass,
	{2, 5}:  Pass,
	{2, 6}:  Pass,
	{2, 7}:  Pass,
	{2, 8}:  Pass,
	{2, 9}:  Pass,
	{2, 10}: Pass,
	{2, 11}: AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{2, 12}: AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{2, 13}: AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{3, 1}:  Pass,
	{3, 2}:  Pass,
	{3, 3}:  Pass,
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	{3, 6}:  Pass,
	{3, 7}:  Pass,
	{3, 8}:  Pass,
	{4, 1}:  Fail,
	{5, 1}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{5, 2}:  Fail,
	{5, 3}:  Pass,
	{6, 3}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{6, 6}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{7, 1}:  Pass,
	{7, 3}:  Pass,
	{8, 1}:  Pass,
	{8, 2}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{9, 1}:  Pass,
	{9, 2}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{10, 1}: Fail,
}

var udpV6ExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{2, 1}:  Pass,
	{2, 2}:  Pass,
	{2, 3}:  Pass,
	{2, 4}:  Pass,
	{2, 5}:  Pass,
	{2, 6}:  Pass,
	{2, 7}:  Pass,
	{2, 8}:  Pass,
	{2, 9}:  Pass,
	{2, 10}: Pass,
	{2, 11}: AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{2, 12}: AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{2, 13}: AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{3, 1}:  Pass,
	{3, 2}:  Pass,
	{3, 3}:  Pass,
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	{3, 6}:  Pass,
	{3, 7}:  Pass,
	{3, 8}:  Pass,
	{4, 1}:  Fail,
	{5, 1}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{5, 2}:  Fail,
	{5, 3}:  Pass,
	{6, 3}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{6, 6}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{7, 1}:  Pass,
	{7, 3}:  Pass,
	{8, 1}:  Pass,
	{8, 2}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{9, 1}:  Pass,
	{9, 2}:  AnvlSkip, // IPv4 test, but this is the IPv6 suite.
	{10, 1}: Fail,
}
