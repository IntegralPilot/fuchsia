// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var icmpExpectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}: Pass,
	{1, 2}: AnvlSkip, // Router test, but this is the host suite.
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{1, 3}: AnvlSkip,
	{1, 5}: Pass,
	{1, 6}: AnvlSkip, // Router test, but this is the host suite.
	{2, 1}: Pass,
	{2, 2}: Pass,
	{2, 3}: Pass,
	{2, 4}: Pass,
	{2, 5}: Pass,
	{3, 1}: Pass,
	{3, 2}: AnvlSkip, // Router test, but this is the host suite.
	{4, 1}: AnvlSkip, // Router test, but this is the host suite.
	{4, 2}: Pass,
	{4, 3}: Fail,
	{4, 4}: Pass,
	{4, 5}: AnvlSkip, // Router test, but this is the host suite.
	{5, 1}: Pass,
	{5, 2}: Pass,
	{5, 3}: Pass,
	{6, 1}: AnvlSkip, // Router test, but this is the host suite.
	{7, 1}: AnvlSkip, // Router test, but this is the host suite.
	{7, 2}: AnvlSkip, // Router test, but this is the host suite.
	{7, 3}: AnvlSkip, // Router test, but this is the host suite.
	{7, 4}: AnvlSkip, // Router test, but this is the host suite.
	{8, 1}: Pass,
	{8, 2}: Pass,
	{8, 3}: Pass,
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{9, 1}: AnvlSkip,
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{9, 2}: AnvlSkip,
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{9, 3}:  AnvlSkip,
	{10, 1}: Pass,
}

var icmpExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}: Pass,
	{1, 2}: AnvlSkip, // Router test, but this is the host suite.
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{1, 3}: AnvlSkip,
	{1, 5}: Pass,
	{1, 6}: AnvlSkip, // Router test, but this is the host suite.
	{2, 1}: Pass,
	{2, 2}: Fail,
	{2, 3}: Pass,
	{2, 4}: Pass,
	{2, 5}: Pass,
	{3, 1}: Pass,
	{3, 2}: AnvlSkip, // Router test, but this is the host suite.
	{4, 1}: AnvlSkip, // Router test, but this is the host suite.
	{4, 2}: Pass,
	{4, 3}: Fail,
	{4, 4}: Pass,
	{4, 5}: AnvlSkip, // Router test, but this is the host suite.
	{5, 1}: Fail,
	{5, 2}: Fail,
	{5, 3}: Fail,
	{6, 1}: AnvlSkip, // Router test, but this is the host suite.
	{7, 1}: AnvlSkip, // Router test, but this is the host suite.
	{7, 2}: AnvlSkip, // Router test, but this is the host suite.
	{7, 3}: AnvlSkip, // Router test, but this is the host suite.
	{7, 4}: AnvlSkip, // Router test, but this is the host suite.
	{8, 1}: Pass,
	{8, 2}: Pass,
	{8, 3}: Pass,
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{9, 1}: AnvlSkip,
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{9, 2}: AnvlSkip,
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{9, 3}:  AnvlSkip,
	{10, 1}: Pass,
}
