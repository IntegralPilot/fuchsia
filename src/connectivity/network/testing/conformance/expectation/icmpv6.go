// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var icmpv6Expectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  Pass,
	{2, 1}:  Fail,
	{2, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 3}:  Pass,
	{3, 1}:  Inconclusive,
	{3, 2}:  Pass,
	{4, 1}:  Pass,
	{4, 2}:  Pass,
	{4, 3}:  Pass,
	{4, 4}:  Pass,
	{4, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 6}:  Pass,
	{4, 7}:  Pass,
	{4, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 9}:  Pass,
	{4, 10}: Pass,
	{4, 11}: AnvlSkip, // Router test, but this is the host suite.
	{4, 12}: Fail,
	{4, 13}: Pass,
	{5, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 6}:  AnvlSkip, // NB: Tests PPP so not relevant.
	{5, 7}:  Pass,
	{5, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 10}: AnvlSkip, // Router test, but this is the host suite.
	{5, 11}: Pass,
	{5, 12}: Pass,
	{6, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{8, 1}:  Pass,
	{8, 2}:  Pass,
	{8, 3}:  Pass,
	{8, 4}:  Pass,
	{8, 5}:  Pass,
	{9, 1}:  Pass,
	{9, 2}:  Pass,
	{10, 1}: Pass,
}

var icmpv6ExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  Pass,
	{2, 1}:  Fail,
	{2, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 3}:  Pass,
	{3, 1}:  Inconclusive,
	{3, 2}:  Pass,
	{4, 1}:  Pass,
	{4, 2}:  Pass,
	{4, 3}:  Pass,
	{4, 4}:  Pass,
	{4, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 6}:  Pass,
	{4, 7}:  Pass,
	{4, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 9}:  Pass,
	{4, 10}: Pass,
	{4, 11}: AnvlSkip, // Router test, but this is the host suite.
	{4, 12}: Pass,
	{4, 13}: Pass,
	{5, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 6}:  AnvlSkip, // NB: Tests PPP so not relevant.
	{5, 7}:  Pass,
	{5, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 10}: AnvlSkip, // Router test, but this is the host suite.
	{5, 11}: Pass,
	{5, 12}: Pass,
	{6, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{8, 1}:  Pass,
	{8, 2}:  Fail,
	{8, 3}:  Pass,
	{8, 4}:  Pass,
	{8, 5}:  Fail,
	{9, 1}:  Pass,
	{9, 2}:  Pass,
	{10, 1}: Pass,
}
