// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var ipv6MldExpectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{1, 2}:  Pass,
	{1, 3}:  Pass,
	{1, 4}:  Pass,
	{1, 5}:  Pass,
	{1, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{1, 7}:  Pass,
	{1, 8}:  Pass,
	{1, 9}:  Pass,
	{1, 10}: Pass,
	{1, 11}: Pass,
	{1, 12}: AnvlSkip, // Router test, but this is the host suite.
	{1, 13}: AnvlSkip, // Router test, but this is the host suite.
	{1, 14}: AnvlSkip, // Router test, but this is the host suite.
	{1, 15}: AnvlSkip, // Router test, but this is the host suite.
	{1, 16}: Fail,
	{1, 17}: AnvlSkip, // Router test, but this is the host suite.
	{1, 18}: AnvlSkip, // Router test, but this is the host suite.
	{1, 19}: AnvlSkip, // Router test, but this is the host suite.
	{1, 20}: AnvlSkip, // Router test, but this is the host suite.
	{2, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 3}:  Pass,
	{2, 4}:  Fail,
	{2, 5}:  Pass,
	{2, 6}:  Fail,
	{2, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 1}:  Pass,
	{3, 2}:  Fail,
	{3, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 3}:  Pass,
	{4, 4}:  Fail,
	{4, 5}:  Pass,
	{4, 6}:  Fail,
	{4, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 3}:  Fail,
	{5, 4}:  Fail,
	{6, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 3}:  Pass,
	{6, 4}:  Fail,
	{6, 5}:  Pass,
	{6, 6}:  Fail,
	{6, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 9}:  Pass,
	{6, 10}: Pass,
	{6, 11}: AnvlSkip, // Router test, but this is the host suite.
	{6, 12}: AnvlSkip, // Router test, but this is the host suite.
	{7, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 10}: AnvlSkip, // Router test, but this is the host suite.
	{7, 11}: Pass,
	{7, 12}: Fail,
	{7, 13}: Pass,
	{7, 14}: Pass,
	{7, 15}: Pass,
	{7, 16}: Fail,
	{7, 17}: Fail,
	{7, 18}: Fail,
	{7, 19}: Pass,
	{7, 20}: AnvlSkip, // Router test, but this is the host suite.
	{7, 21}: AnvlSkip, // Router test, but this is the host suite.
	{7, 22}: AnvlSkip, // Router test, but this is the host suite.
	{7, 23}: Fail,
	{7, 24}: Fail,
	{7, 25}: Fail,
	{7, 26}: AnvlSkip, // TODO(https://fxbug.dev/413702633): Support turning off MLD other listener present optimization.
	{7, 27}: AnvlSkip, // Router test, but this is the host suite.
	{7, 28}: AnvlSkip, // Router test, but this is the host suite.
	{7, 29}: AnvlSkip, // Router test, but this is the host suite.
	{7, 30}: AnvlSkip, // Router test, but this is the host suite.
	{7, 31}: AnvlSkip, // Router test, but this is the host suite.
	{7, 32}: AnvlSkip, // Router test, but this is the host suite.
	{8, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{8, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{8, 3}:  Pass,
	{8, 4}:  Fail,
	{8, 5}:  Pass,
	{8, 6}:  Pass,
	{8, 7}:  Pass,
	{9, 1}:  Fail,
	{9, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 3}:  AnvlSkip, // Router test, but this is the host suite.
}

var ipv6MldExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{1, 2}:  Pass,
	{1, 3}:  Pass,
	{1, 4}:  Pass,
	{1, 5}:  Pass,
	{1, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{1, 7}:  Pass,
	{1, 8}:  Pass,
	{1, 9}:  Pass,
	{1, 10}: Pass,
	{1, 11}: Pass,
	{1, 12}: AnvlSkip, // Router test, but this is the host suite.
	{1, 13}: AnvlSkip, // Router test, but this is the host suite.
	{1, 14}: AnvlSkip, // Router test, but this is the host suite.
	{1, 15}: AnvlSkip, // Router test, but this is the host suite.
	{1, 16}: Fail,
	{1, 17}: AnvlSkip, // Router test, but this is the host suite.
	{1, 18}: AnvlSkip, // Router test, but this is the host suite.
	{1, 19}: AnvlSkip, // Router test, but this is the host suite.
	{1, 20}: AnvlSkip, // Router test, but this is the host suite.
	{2, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 3}:  Pass,
	{2, 4}:  Fail,
	{2, 5}:  Pass,
	{2, 6}:  Fail,
	{2, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{2, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 1}:  Pass,
	{3, 2}:  Fail,
	{3, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 3}:  Pass,
	{4, 4}:  Fail,
	{4, 5}:  Pass,
	{4, 6}:  Fail,
	{4, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{5, 3}:  Fail,
	{5, 4}:  Fail,
	{6, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 3}:  Pass,
	{6, 4}:  Fail,
	{6, 5}:  Fail,
	{6, 6}:  Fail,
	{6, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{6, 9}:  Pass,
	{6, 10}: Pass,
	{6, 11}: AnvlSkip, // Router test, but this is the host suite.
	{6, 12}: AnvlSkip, // Router test, but this is the host suite.
	{7, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 10}: AnvlSkip, // Router test, but this is the host suite.
	{7, 11}: Pass,
	{7, 12}: Fail,
	{7, 13}: Pass,
	{7, 14}: Pass,
	{7, 15}: Pass,
	{7, 16}: Fail,
	{7, 17}: Fail,
	{7, 18}: Fail,
	{7, 19}: Pass,
	{7, 20}: AnvlSkip, // Router test, but this is the host suite.
	{7, 21}: AnvlSkip, // Router test, but this is the host suite.
	{7, 22}: AnvlSkip, // Router test, but this is the host suite.
	{7, 23}: Fail,
	{7, 24}: Fail,
	{7, 25}: Fail,
	{7, 26}: AnvlSkip, // TODO(https://fxbug.dev/413702633): Support turning off MLD other listener present optimization.
	{7, 27}: AnvlSkip, // Router test, but this is the host suite.
	{7, 28}: AnvlSkip, // Router test, but this is the host suite.
	{7, 29}: AnvlSkip, // Router test, but this is the host suite.
	{7, 30}: AnvlSkip, // Router test, but this is the host suite.
	{7, 31}: AnvlSkip, // Router test, but this is the host suite.
	{7, 32}: AnvlSkip, // Router test, but this is the host suite.
	{8, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{8, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{8, 3}:  Pass,
	{8, 4}:  Fail,
	{8, 5}:  Pass,
	{8, 6}:  Pass,
	{8, 7}:  Pass,
	{9, 1}:  Fail,
	{9, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 3}:  AnvlSkip, // Router test, but this is the host suite.
}
