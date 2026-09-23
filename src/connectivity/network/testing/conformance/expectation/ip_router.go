// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var ipRouterExpectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  Inconclusive,
	{1, 3}:  Pass,
	{1, 4}:  Inconclusive,
	{2, 1}:  Inconclusive,
	{2, 2}:  Pass,
	{2, 3}:  Pass,
	{2, 4}:  Pass,
	{3, 1}:  AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{3, 2}:  AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{3, 3}:  AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	{3, 6}:  Pass,
	{3, 7}:  Pass,
	{3, 8}:  Pass,
	{4, 1}:  Pass,
	{4, 2}:  Pass,
	{4, 3}:  Inconclusive,
	{4, 4}:  Pass,
	{4, 5}:  Pass,
	{4, 6}:  Pass,
	{5, 1}:  Pass,
	{5, 2}:  Pass,
	{5, 3}:  Fail,
	{5, 4}:  Fail,
	{5, 5}:  AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{5, 6}:  Pass,
	{5, 7}:  Inconclusive,
	{5, 8}:  Fail,
	{5, 9}:  Inconclusive,
	{5, 10}: Inconclusive,
	{5, 11}: Fail,
	{5, 12}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{5, 13}: Pass,
	{5, 14}: Pass,
	{5, 15}: Inconclusive,
	{5, 16}: Inconclusive,
	{5, 17}: Inconclusive,
	{5, 18}: Pass,
	{5, 19}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{5, 20}: Pass,
	{5, 21}: Pass,
	{5, 22}: Inconclusive,
	{5, 23}: Inconclusive,
	{5, 24}: Pass,
	{5, 25}: Inconclusive,
	{5, 26}: Inconclusive,
	{5, 27}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{5, 28}: Inconclusive,
	{5, 29}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{6, 1}:  Pass,
	{6, 2}:  Pass,
	{6, 3}:  Pass,
	{6, 4}:  Pass,
	{6, 5}:  Pass,
	{6, 6}:  Inconclusive,
	{6, 7}:  Pass,
	{6, 8}:  Pass,
	{6, 9}:  Pass,
	{6, 10}: Pass,
	{6, 11}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{6, 12}: Pass,
	{6, 13}: Fail,
	{7, 1}:  Pass,
	{7, 2}:  Inconclusive,
	{7, 3}:  Fail,
	{7, 4}:  Pass,
	{7, 5}:  Pass,
	{7, 6}:  Pass,
}

var ipRouterExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  Inconclusive,
	{1, 3}:  Pass,
	{1, 4}:  Inconclusive,
	{2, 1}:  Inconclusive,
	{2, 2}:  Pass,
	{2, 3}:  Pass,
	{2, 4}:  Pass,
	{3, 1}:  AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{3, 2}:  AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{3, 3}:  AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	{3, 6}:  Pass,
	{3, 7}:  Pass,
	{3, 8}:  Pass,
	{4, 1}:  Pass,
	{4, 2}:  Pass,
	{4, 3}:  Inconclusive,
	{4, 4}:  Pass,
	{4, 5}:  Pass,
	{4, 6}:  Pass,
	{5, 1}:  Pass,
	{5, 2}:  Pass,
	{5, 3}:  Fail,
	{5, 4}:  Fail,
	{5, 5}:  AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{5, 6}:  Fail,
	{5, 7}:  Inconclusive,
	{5, 8}:  Fail,
	{5, 9}:  Inconclusive,
	{5, 10}: Inconclusive,
	{5, 11}: Fail,
	{5, 12}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{5, 13}: Pass,
	{5, 14}: Inconclusive,
	{5, 15}: Inconclusive,
	{5, 16}: Inconclusive,
	{5, 17}: Inconclusive,
	{5, 18}: Pass,
	{5, 19}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{5, 20}: Pass,
	{5, 21}: Inconclusive,
	{5, 22}: Inconclusive,
	{5, 23}: Inconclusive,
	{5, 24}: Pass,
	{5, 25}: Inconclusive,
	{5, 26}: Inconclusive,
	{5, 27}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{5, 28}: Inconclusive,
	{5, 29}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{6, 1}:  Fail,
	{6, 2}:  Fail,
	{6, 3}:  Fail,
	{6, 4}:  Fail,
	{6, 5}:  Fail,
	{6, 6}:  Inconclusive,
	{6, 7}:  Fail,
	{6, 8}:  Fail,
	{6, 9}:  Inconclusive,
	{6, 10}: Inconclusive,
	{6, 11}: AnvlSkip, // TODO(https://fxbug.dev/42095222): Support configuring MTU.
	{6, 12}: Inconclusive,
	{6, 13}: Fail,
	{7, 1}:  Pass,
	{7, 2}:  Inconclusive,
	{7, 3}:  Pass,
	// TODO(https://fxbug.dev/414413500) Consider the TTL of IPv4 fragments when
	// setting the reassembly timeout.
	{7, 4}: Flaky,
	{7, 5}: Pass,
	{7, 6}: Pass,
}
