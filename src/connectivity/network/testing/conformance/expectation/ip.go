// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var ipExpectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{1, 3}:  Pass,
	{1, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 3}:  Pass,
	{2, 4}:  Pass,
	{3, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	{3, 6}:  Pass,
	{3, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 8}:  Pass,
	{4, 1}:  Pass,
	{4, 2}:  Pass,
	{4, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 4}:  Pass,
	{4, 5}:  Pass,
	{4, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 1}:  Pass,
	{5, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 6}:  Pass,
	{5, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 10}: AnvlSkip, // Router test, but this is the host suite.
	{5, 11}: AnvlSkip, // Router test, but this is the host suite.
	{5, 12}: AnvlSkip, // Router test, but this is the host suite.
	{5, 13}: AnvlSkip, // Router test, but this is the host suite.
	{5, 14}: AnvlSkip, // Router test, but this is the host suite.
	{5, 15}: AnvlSkip, // Router test, but this is the host suite.
	{5, 16}: AnvlSkip, // Router test, but this is the host suite.
	{5, 17}: AnvlSkip, // Router test, but this is the host suite.
	{5, 18}: AnvlSkip, // Router test, but this is the host suite.
	{5, 19}: AnvlSkip, // Router test, but this is the host suite.
	{5, 20}: AnvlSkip, // Router test, but this is the host suite.
	{5, 21}: AnvlSkip, // Router test, but this is the host suite.
	{5, 22}: AnvlSkip, // Router test, but this is the host suite.
	{5, 23}: AnvlSkip, // Router test, but this is the host suite.
	{5, 24}: AnvlSkip, // Router test, but this is the host suite.
	{5, 25}: AnvlSkip, // Router test, but this is the host suite.
	{5, 26}: AnvlSkip, // Router test, but this is the host suite.
	{5, 27}: AnvlSkip, // Router test, but this is the host suite.
	{5, 28}: AnvlSkip, // Router test, but this is the host suite.
	{5, 29}: AnvlSkip, // Router test, but this is the host suite.
	{6, 1}:  Pass,
	{6, 2}:  Pass,
	{6, 3}:  Pass,
	{6, 4}:  Pass,
	{6, 5}:  Pass,
	{6, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 7}:  Pass,
	{6, 8}:  Pass,
	{6, 9}:  Pass,
	{6, 10}: Pass,
	{6, 11}: AnvlSkip, // Router test, but this is the host suite.
	{6, 12}: Pass,
	{6, 13}: AnvlSkip, // Router test, but this is the host suite.
	{7, 1}:  Pass,
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  Fail,
	{7, 4}:  Pass,
	{7, 5}:  Pass,
	{7, 6}:  Pass,
}

var ipExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{1, 3}:  Pass,
	{1, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 3}:  Pass,
	{2, 4}:  Pass,
	{3, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	{3, 6}:  Pass,
	{3, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 8}:  Pass,
	{4, 1}:  Pass,
	{4, 2}:  Pass,
	{4, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 4}:  Pass,
	{4, 5}:  Pass,
	{4, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 1}:  Pass,
	{5, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 6}:  Fail,
	{5, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 10}: AnvlSkip, // Router test, but this is the host suite.
	{5, 11}: AnvlSkip, // Router test, but this is the host suite.
	{5, 12}: AnvlSkip, // Router test, but this is the host suite.
	{5, 13}: AnvlSkip, // Router test, but this is the host suite.
	{5, 14}: AnvlSkip, // Router test, but this is the host suite.
	{5, 15}: AnvlSkip, // Router test, but this is the host suite.
	{5, 16}: AnvlSkip, // Router test, but this is the host suite.
	{5, 17}: AnvlSkip, // Router test, but this is the host suite.
	{5, 18}: AnvlSkip, // Router test, but this is the host suite.
	{5, 19}: AnvlSkip, // Router test, but this is the host suite.
	{5, 20}: AnvlSkip, // Router test, but this is the host suite.
	{5, 21}: AnvlSkip, // Router test, but this is the host suite.
	{5, 22}: AnvlSkip, // Router test, but this is the host suite.
	{5, 23}: AnvlSkip, // Router test, but this is the host suite.
	{5, 24}: AnvlSkip, // Router test, but this is the host suite.
	{5, 25}: AnvlSkip, // Router test, but this is the host suite.
	{5, 26}: AnvlSkip, // Router test, but this is the host suite.
	{5, 27}: AnvlSkip, // Router test, but this is the host suite.
	{5, 28}: AnvlSkip, // Router test, but this is the host suite.
	{5, 29}: AnvlSkip, // Router test, but this is the host suite.
	{6, 1}:  Fail,
	{6, 2}:  Fail,
	{6, 3}:  Fail,
	{6, 4}:  Fail,
	{6, 5}:  Fail,
	{6, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 7}:  Fail,
	{6, 8}:  Fail,
	{6, 9}:  Inconclusive,
	{6, 10}: Inconclusive,
	{6, 11}: AnvlSkip, // Router test, but this is the host suite.
	{6, 12}: Inconclusive,
	{6, 13}: AnvlSkip, // Router test, but this is the host suite.
	{7, 1}:  Pass,
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  Pass,
	// TODO(https://fxbug.dev/414413500) Consider the TTL of IPv4 fragments when
	// setting the reassembly timeout.
	{7, 4}: Flaky,
	{7, 5}: Pass,
	{7, 6}: Pass,
}
