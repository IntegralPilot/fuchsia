// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import (
	"go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"
)

var igmpv3ExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}: AnvlSkip, // Router test, but this is the host suite.
	{1, 2}: Pass,
	{2, 1}: Pass,
	{2, 2}: Pass,
	{2, 3}: Inconclusive,
	{2, 4}: Inconclusive,
	{3, 1}: Inconclusive,
	{3, 2}: Inconclusive,
	// TODO(https://fxbug.dev/384382369): 3.3 crashes ANVL, possible it'd be a pass if we had SSM.
	{3, 3}: Skip,
	{4, 1}: AnvlSkip, // Router test, but this is the host suite.
	{4, 2}: Pass,
	{4, 3}: Pass,
	{4, 4}: AnvlSkip, // Router test, but this is the host suite.
	{4, 5}: AnvlSkip, // Router test, but this is the host suite.
	{4, 6}: Pass,
	{4, 7}: Pass,
	{4, 8}: AnvlSkip, // Router test, but this is the host suite.
	{4, 9}: AnvlSkip, // Router test, but this is the host suite.
	// Discard messages with bad TOS. Feels too strict, not explicitly called
	// out in RFC.
	{4, 10}: Fail,
	{4, 11}: Pass,
	{4, 13}: AnvlSkip, // Router test, but this is the host suite.
	{4, 14}: Pass,
	{4, 15}: Pass,
	{4, 16}: AnvlSkip, // Router test, but this is the host suite.
	{4, 17}: AnvlSkip, // Router test, but this is the host suite.
	{4, 18}: AnvlSkip, // Router test, but this is the host suite.
	{4, 19}: AnvlSkip, // Router test, but this is the host suite.
	{4, 20}: Pass,
	{5, 1}:  Pass,
	{5, 2}:  Pass,
	{5, 3}:  Pass,
	{5, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 5}:  Pass,
	{5, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 10}: Pass,
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
	{5, 24}: Pass,
	{5, 25}: Pass,
	{5, 26}: AnvlSkip, // Router test, but this is the host suite.
	{5, 27}: AnvlSkip, // Router test, but this is the host suite.
	{5, 28}: AnvlSkip, // Router test, but this is the host suite.
	{5, 29}: AnvlSkip, // Router test, but this is the host suite.
	{5, 30}: AnvlSkip, // Router test, but this is the host suite.
	// At odds with test 11.1. Both tests generate the same input. 5.31 expects
	// an answer, 11.1 expects the query to be ignored. The RFC itself is
	// ambiguous about this, but we side with 11.1.
	{5, 31}: Fail,
	{5, 32}: Pass,
	{6, 1}:  Pass,
	{6, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 3}:  Pass,
	{6, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 5}:  Pass,
	{6, 6}:  Inconclusive,
	{6, 7}:  Pass,
	{6, 8}:  Inconclusive,
	{6, 9}:  Pass,
	{6, 10}: AnvlSkip, // Router test, but this is the host suite.
	{6, 11}: Pass,
	{6, 12}: AnvlSkip, // Router test, but this is the host suite.
	{6, 13}: AnvlSkip, // Router test, but this is the host suite.
	{6, 14}: Inconclusive,
	{6, 15}: Fail,
	{6, 16}: Inconclusive,
	{6, 17}: Fail,
	{6, 18}: Inconclusive,
	{6, 19}: Inconclusive,
	{6, 20}: Inconclusive,
	{6, 21}: Inconclusive,
	{6, 22}: Inconclusive,
	{6, 23}: Pass,
	{6, 24}: AnvlSkip, // Router test, but this is the host suite.
	{6, 25}: Pass,
	{6, 26}: AnvlSkip, // Router test, but this is the host suite.
	{6, 27}: Pass,
	{6, 28}: Pass,
	{6, 29}: AnvlSkip, // Router test, but this is the host suite.
	{6, 30}: AnvlSkip, // Router test, but this is the host suite.
	{6, 31}: AnvlSkip, // Router test, but this is the host suite.
	{6, 32}: AnvlSkip, // Router test, but this is the host suite.
	{7, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{8, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{8, 2}:  Pass,
	{8, 3}:  Pass,
	{8, 4}:  Inconclusive,
	{8, 5}:  Inconclusive,
	{8, 6}:  Inconclusive,
	{8, 7}:  Inconclusive,
	{8, 8}:  Inconclusive,
	{8, 9}:  Inconclusive,
	{8, 10}: Inconclusive,
	{8, 11}: Inconclusive,
	{8, 12}: Fail,
	{8, 13}: Inconclusive,
	{8, 14}: Inconclusive,
	{8, 15}: Fail,
	{8, 16}: Pass,
	{8, 17}: Inconclusive,
	{9, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 10}: AnvlSkip, // Router test, but this is the host suite.
	{9, 11}: AnvlSkip, // Router test, but this is the host suite.
	{9, 12}: AnvlSkip, // Router test, but this is the host suite.
	{9, 13}: AnvlSkip, // Router test, but this is the host suite.
	{9, 14}: AnvlSkip, // Router test, but this is the host suite.
	{10, 1}: AnvlSkip, // Router test, but this is the host suite.
	{10, 2}: AnvlSkip, // Router test, but this is the host suite.
	{10, 3}: AnvlSkip, // Router test, but this is the host suite.
	{10, 4}: Pass,
	{10, 5}: Pass,
	{10, 6}: Pass,
	{10, 7}: Pass,
	// TODO(https://fxbug.dev/384382023): Test definition seems to be using
	// wrong timeout which causes this to be flaky.
	{10, 8}:  Flaky,
	{10, 9}:  Pass,
	{10, 10}: Pass,
	{10, 11}: Pass,
	{10, 12}: Pass,
	{10, 13}: Pass,
	{10, 14}: Pass,
	{10, 15}: Fail,
	{10, 16}: Fail,
	{10, 17}: AnvlSkip, // Router test, but this is the host suite.
	{10, 18}: AnvlSkip, // Router test, but this is the host suite.
	{10, 19}: AnvlSkip, // Router test, but this is the host suite.
	{10, 22}: AnvlSkip, // Router test, but this is the host suite.
	{10, 23}: AnvlSkip, // Router test, but this is the host suite.
	{10, 24}: AnvlSkip, // Router test, but this is the host suite.
	{10, 25}: AnvlSkip, // Router test, but this is the host suite.
	{11, 1}:  Pass,
	{12, 1}:  AnvlSkip, // TODO(https://fxbug.dev/381241191): Support Source-Specific Multicast.
	{12, 2}:  AnvlSkip, // TODO(https://fxbug.dev/381241191): Support Source-Specific Multicast.
	{12, 3}:  AnvlSkip, // TODO(https://fxbug.dev/381241191): Support Source-Specific Multicast.
	{12, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 10}: AnvlSkip, // Router test, but this is the host suite.
	{12, 11}: AnvlSkip, // Router test, but this is the host suite.
	{12, 12}: AnvlSkip, // Router test, but this is the host suite.
	{12, 13}: AnvlSkip, // Router test, but this is the host suite.
	{12, 14}: AnvlSkip, // Router test, but this is the host suite.
	{12, 15}: AnvlSkip, // Router test, but this is the host suite.
	{12, 16}: AnvlSkip, // TODO(https://fxbug.dev/381241191): Support Source-Specific Multicast.
}
