// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var icmpRouterExpectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}: Pass,
	{1, 2}: Fail,
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{1, 3}: AnvlSkip,
	{1, 5}: Pass,
	{1, 6}: Fail,
	{2, 1}: Pass,
	{2, 2}: Pass,
	{2, 3}: Pass,
	{2, 4}: Pass,
	{2, 5}: Pass,
	{3, 1}: Pass,
	{3, 2}: Fail,
	{4, 1}: Fail,
	{4, 2}: Pass,
	{4, 3}: Fail,
	{4, 4}: Pass,
	{4, 5}: Fail,
	{5, 1}: Pass,
	{5, 2}: Pass,
	{5, 3}: Pass,
	{6, 1}: Fail,
	{7, 1}: Fail,
	{7, 2}: Fail,
	// NB: Tests Loose Source and Record Route that we will likely never support
	// as these packets are often dropped due to security considerations.
	{7, 3}: AnvlSkip,
	{7, 4}: Fail,
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

var icmpRouterExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}: Pass,
	{1, 2}: Fail,
	// TODO(https://fxbug.dev/413421898): Support enabling replying to ICMP timestamp messages.
	{1, 3}: AnvlSkip,
	{1, 5}: Pass,
	{1, 6}: Fail,
	{2, 1}: Pass,
	{2, 2}: Fail,
	{2, 3}: Pass,
	{2, 4}: Pass,
	{2, 5}: Pass,
	{3, 1}: Pass,
	{3, 2}: Fail,
	{4, 1}: Fail,
	{4, 2}: Pass,
	{4, 3}: Fail,
	{4, 4}: Pass,
	{4, 5}: Fail,
	{5, 1}: Fail,
	{5, 2}: Fail,
	{5, 3}: Fail,
	{6, 1}: Fail,
	{7, 1}: Fail,
	{7, 2}: Fail,
	// NB: Tests Loose Source and Record Route that we will likely never support
	// as these packets are often dropped due to security considerations.
	{7, 3}: AnvlSkip,
	{7, 4}: Fail,
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
