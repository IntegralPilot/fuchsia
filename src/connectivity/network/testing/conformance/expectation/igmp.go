// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import (
	"go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"
)

var igmpExpectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 9}:  Pass,
	{2, 10}: Pass,
	{2, 11}: Pass,
	{3, 1}:  Pass,
	{3, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	// TODO(https://fxbug.dev/42055899): Investigate flake.
	{3, 6}:  Flaky,
	{3, 7}:  Pass,
	{3, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 10}: AnvlSkip, // Router test, but this is the host suite.
	{3, 11}: AnvlSkip, // Router test, but this is the host suite.
	{4, 1}:  Fail,
	{4, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 4}:  Pass,
	{4, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 1}:  Pass,
	{5, 2}:  Pass,
	{5, 3}:  Pass,
	{5, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 5}:  Pass,
	{5, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 7}:  Pass,
	{5, 8}:  Fail,
	{5, 9}:  Fail,
	{5, 10}: AnvlSkip, // Router test, but this is the host suite.
	// TODO(https://fxbug.dev/42069359): Fix.
	{5, 11}: Fail,
	{6, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 2}:  Pass,
	{6, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 8}:  Pass,
	{6, 9}:  AnvlSkip, // Router test, but this is the host suite.
}

var igmpExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 9}:  Pass,
	{2, 10}: Pass,
	{2, 11}: Pass,
	{3, 1}:  Pass,
	{3, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	// TODO(https://fxbug.dev/42055899): Investigate flake.
	{3, 6}:  Flaky,
	{3, 7}:  Pass,
	{3, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 10}: AnvlSkip, // Router test, but this is the host suite.
	{3, 11}: AnvlSkip, // Router test, but this is the host suite.
	{4, 1}:  Pass,
	{4, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 4}:  Pass,
	{4, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 1}:  Pass,
	{5, 2}:  Pass,
	{5, 3}:  Pass,
	{5, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 5}:  Pass,
	{5, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 7}:  Pass,
	{5, 8}:  Fail,
	{5, 9}:  Pass,
	{5, 10}: AnvlSkip, // Router test, but this is the host suite.
	// TODO(https://fxbug.dev/42069359): Fix.
	{5, 11}: Flaky,
	{6, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 2}:  Pass,
	{6, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 8}:  Pass,
	{6, 9}:  AnvlSkip, // Router test, but this is the host suite.
}
