// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var ipv6Mldv2ExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{1, 2}:  Fail,
	{2, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{3, 10}: Inconclusive,
	{3, 11}: Inconclusive,
	{3, 12}: Inconclusive,
	{3, 14}: Pass,
	{3, 15}: Fail,
	{3, 16}: Fail,
	{3, 17}: Fail,
	{3, 18}: Pass,
	{3, 19}: Inconclusive,
	{3, 20}: AnvlSkip, // Router test, but this is the host suite.
	{3, 21}: AnvlSkip, // Router test, but this is the host suite.
	{3, 22}: AnvlSkip, // Router test, but this is the host suite.
	{3, 23}: AnvlSkip, // Router test, but this is the host suite.
	{3, 24}: AnvlSkip, // Router test, but this is the host suite.
	{3, 25}: AnvlSkip, // Router test, but this is the host suite.
	{3, 26}: AnvlSkip, // Router test, but this is the host suite.
	{4, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{4, 10}: AnvlSkip, // Router test, but this is the host suite.
	{4, 11}: AnvlSkip, // Router test, but this is the host suite.
	{4, 12}: AnvlSkip, // Router test, but this is the host suite.
	{4, 13}: AnvlSkip, // Router test, but this is the host suite.
	{5, 1}:  Inconclusive,
	{5, 2}:  Inconclusive,
	{6, 1}:  Inconclusive,
	{6, 2}:  Fail,
	{6, 3}:  Fail,
	{7, 1}:  Pass,
	{7, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 4}:  Pass,
	{7, 5}:  Pass,
	{7, 6}:  Pass,
	{7, 7}:  Pass,
	{7, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{7, 9}:  Pass,
	{7, 10}: AnvlSkip, // Router test, but this is the host suite.
	{7, 11}: Pass,
	{7, 12}: Pass,
	{7, 13}: Pass,
	{7, 14}: AnvlSkip, // Router test, but this is the host suite.
	{7, 15}: Pass,
	{7, 16}: AnvlSkip, // Router test, but this is the host suite.
	{7, 17}: AnvlSkip, // Router test, but this is the host suite.
	{7, 18}: AnvlSkip, // Router test, but this is the host suite.
	{7, 19}: AnvlSkip, // Router test, but this is the host suite.
	{7, 20}: AnvlSkip, // Router test, but this is the host suite.
	{7, 21}: AnvlSkip, // Router test, but this is the host suite.
	{7, 22}: AnvlSkip, // Router test, but this is the host suite.
	{7, 23}: AnvlSkip, // Router test, but this is the host suite.
	{7, 24}: AnvlSkip, // Router test, but this is the host suite.
	{7, 25}: AnvlSkip, // Router test, but this is the host suite.
	{7, 26}: AnvlSkip, // Router test, but this is the host suite.
	{7, 27}: AnvlSkip, // Router test, but this is the host suite.
	{7, 28}: AnvlSkip, // Router test, but this is the host suite.
	{7, 29}: AnvlSkip, // Router test, but this is the host suite.
	{7, 30}: Pass,
	{7, 31}: AnvlSkip, // Router test, but this is the host suite.
	{7, 32}: AnvlSkip, // Router test, but this is the host suite.
	{8, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{8, 2}:  Pass,
	{8, 3}:  Pass,
	{9, 1}:  Pass,
	{9, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 3}:  Pass,
	{9, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 5}:  Pass,
	{9, 6}:  Pass,
	{9, 7}:  Fail,
	{9, 8}:  Pass,
	{9, 9}:  Fail,
	{9, 10}: Fail,
	{9, 11}: AnvlSkip, // Router test, but this is the host suite.
	{9, 12}: AnvlSkip, // Router test, but this is the host suite.
	{9, 13}: Fail,
	{9, 14}: Fail,
	{9, 15}: Fail,
	{9, 16}: Fail,
	{9, 17}: Fail,
	{9, 18}: Fail,
	// TODO(https://fxbug.dev/42071938): 9.19 crashes ANVL, so we have to skip it.
	{9, 19}:  Skip,
	{9, 20}:  Inconclusive,
	{9, 21}:  Fail,
	{9, 22}:  Pass,
	{9, 23}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 24}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 25}:  Fail,
	{9, 26}:  AnvlSkip, // Router test, but this is the host suite.
	{9, 27}:  AnvlSkip, // Router test, but this is the host suite.
	{10, 1}:  Pass,
	{10, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{10, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{10, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{10, 5}:  Fail,
	{10, 6}:  Fail,
	{10, 7}:  Fail,
	{10, 8}:  Fail,
	{10, 9}:  Fail,
	{10, 10}: Fail,
	{10, 11}: Fail,
	{10, 12}: Pass,
	{10, 13}: Inconclusive,
	{10, 14}: Inconclusive,
	{10, 15}: Inconclusive,
	{10, 16}: Fail,
	{10, 17}: Pass,
	{10, 18}: Inconclusive,
	{10, 19}: Pass,
	{10, 20}: Pass,
	{10, 21}: Fail,
	{10, 22}: Pass,
	{10, 23}: Pass,
	{10, 24}: Inconclusive,
	{10, 25}: Pass,
	{10, 26}: Fail,
	{10, 27}: Pass,
	{10, 28}: Pass,
	{10, 29}: Fail,
	{10, 30}: Fail,
	{10, 31}: Fail,
	{11, 1}:  Fail,
	{11, 2}:  Fail,
	{12, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{12, 3}:  AnvlSkip, // Router test, but this is the host suite.
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
	{12, 16}: Fail,
	{12, 17}: AnvlSkip, // Router test, but this is the host suite.
	{12, 18}: AnvlSkip, // Router test, but this is the host suite.
	{12, 19}: AnvlSkip, // Router test, but this is the host suite.
	{12, 20}: AnvlSkip, // Router test, but this is the host suite.
	{12, 21}: AnvlSkip, // Router test, but this is the host suite.
	{12, 22}: AnvlSkip, // Router test, but this is the host suite.
	{12, 23}: AnvlSkip, // Router test, but this is the host suite.
	{12, 24}: AnvlSkip, // Router test, but this is the host suite.
	{12, 25}: AnvlSkip, // Router test, but this is the host suite.
	{12, 26}: AnvlSkip, // Router test, but this is the host suite.
	{13, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{13, 2}:  AnvlSkip, // Router test, but this is the host suite.
	{13, 3}:  AnvlSkip, // Router test, but this is the host suite.
	{13, 4}:  AnvlSkip, // Router test, but this is the host suite.
	{13, 5}:  AnvlSkip, // Router test, but this is the host suite.
	{13, 6}:  AnvlSkip, // Router test, but this is the host suite.
	{13, 7}:  AnvlSkip, // Router test, but this is the host suite.
	{13, 8}:  AnvlSkip, // Router test, but this is the host suite.
	{13, 9}:  AnvlSkip, // Router test, but this is the host suite.
	{13, 10}: AnvlSkip, // Router test, but this is the host suite.
	{13, 11}: AnvlSkip, // Router test, but this is the host suite.
	{13, 12}: AnvlSkip, // Router test, but this is the host suite.
	{13, 13}: AnvlSkip, // Router test, but this is the host suite.
	{13, 14}: AnvlSkip, // Router test, but this is the host suite.
	{13, 15}: AnvlSkip, // Router test, but this is the host suite.
	{13, 16}: AnvlSkip, // Router test, but this is the host suite.
	{13, 17}: AnvlSkip, // Router test, but this is the host suite.
	{14, 1}:  AnvlSkip, // Router test, but this is the host suite.
	{14, 2}:  Pass,
	{14, 3}:  Pass,
	{14, 4}:  Pass,
	{14, 5}:  Pass,
	{14, 6}:  Pass,
	{14, 7}:  Pass,
	{14, 8}:  Pass,
	{14, 9}:  Fail,
	{14, 10}: AnvlSkip, // Router test, but this is the host suite.
	{14, 11}: AnvlSkip, // Router test, but this is the host suite.
	{14, 12}: AnvlSkip, // Router test, but this is the host suite.
	{14, 13}: AnvlSkip, // Router test, but this is the host suite.
	{14, 14}: AnvlSkip, // Router test, but this is the host suite.
	{14, 15}: AnvlSkip, // Router test, but this is the host suite.
	{14, 16}: AnvlSkip, // Router test, but this is the host suite.
	{14, 17}: AnvlSkip, // Router test, but this is the host suite.
	{14, 18}: AnvlSkip, // Router test, but this is the host suite.
}
