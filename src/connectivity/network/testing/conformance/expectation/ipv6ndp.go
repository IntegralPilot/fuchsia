// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var ipv6ndpExpectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:   Pass,
	{2, 1}:   Pass,
	{2, 2}:   AnvlSkip, // Router test but this is the host suite.
	{2, 3}:   Pass,
	{2, 4}:   AnvlSkip, // Router test but this is the host suite.
	{3, 1}:   AnvlSkip, // Router test but this is the host suite.
	{3, 2}:   AnvlSkip, // Router test but this is the host suite.
	{3, 3}:   AnvlSkip, // Router test but this is the host suite.
	{3, 4}:   AnvlSkip, // Router test but this is the host suite.
	{3, 5}:   Pass,
	{3, 6}:   Pass,
	{3, 7}:   Pass,
	{3, 8}:   AnvlSkip, // Router test but this is the host suite.
	{3, 9}:   Pass,
	{4, 1}:   Pass,
	{4, 2}:   Pass,
	{4, 3}:   Pass,
	{4, 4}:   Pass,
	{4, 5}:   Pass,
	{4, 6}:   Pass,
	{4, 7}:   Pass,
	{4, 8}:   Pass,
	{4, 9}:   Pass,
	{4, 10}:  Pass,
	{4, 11}:  Pass,
	{5, 1}:   Pass,
	{5, 2}:   Pass,
	{5, 3}:   Pass,
	{5, 4}:   Pass,
	{5, 5}:   AnvlSkip, // Router test but this is the host suite.
	{5, 6}:   Pass,
	{5, 7}:   Pass,
	{5, 8}:   Pass,
	{5, 9}:   AnvlSkip, // Router test but this is the host suite.
	{5, 10}:  Pass,
	{5, 12}:  Pass,
	{5, 13}:  Pass,
	{5, 14}:  Pass,
	{5, 15}:  Pass,
	{5, 16}:  Pass,
	{5, 17}:  Pass,
	{6, 1}:   AnvlSkip, // Router test but this is the host suite.
	{6, 2}:   AnvlSkip, // Router test but this is the host suite.
	{6, 3}:   AnvlSkip, // Router test but this is the host suite.
	{6, 4}:   Pass,
	{6, 5}:   AnvlSkip, // Router test but this is the host suite.
	{6, 6}:   AnvlSkip, // Router test but this is the host suite.
	{6, 7}:   AnvlSkip, // Router test but this is the host suite.
	{6, 8}:   AnvlSkip, // Router test but this is the host suite.
	{6, 9}:   AnvlSkip, // Router test but this is the host suite.
	{7, 1}:   AnvlSkip, // Router test but this is the host suite.
	{7, 2}:   Flaky,
	{7, 3}:   AnvlSkip, // Router test but this is the host suite.
	{7, 4}:   AnvlSkip, // Router test but this is the host suite.
	{7, 5}:   Pass,
	{7, 6}:   AnvlSkip, // Router test but this is the host suite.
	{8, 1}:   AnvlSkip, // Router test but this is the host suite.
	{8, 2}:   Inconclusive,
	{9, 1}:   AnvlSkip, // Router test but this is the host suite.
	{9, 2}:   AnvlSkip, // Router test but this is the host suite.
	{10, 1}:  Pass,
	{10, 2}:  Pass,
	{11, 1}:  Pass,
	{11, 2}:  AnvlSkip, // Router test but this is the host suite.
	{11, 3}:  AnvlSkip, // Router test but this is the host suite.
	{11, 4}:  AnvlSkip, // Router test but this is the host suite.
	{11, 5}:  AnvlSkip, // Router test but this is the host suite.
	{11, 6}:  AnvlSkip, // Router test but this is the host suite.
	{11, 7}:  AnvlSkip, // Router test but this is the host suite.
	{11, 8}:  AnvlSkip, // Router test but this is the host suite.
	{11, 9}:  AnvlSkip, // Router test but this is the host suite.
	{11, 10}: AnvlSkip, // Router test but this is the host suite.
	{11, 11}: AnvlSkip, // Router test but this is the host suite.
	{11, 12}: Pass,
	{12, 1}:  Pass,
	{12, 2}:  Pass,
	{12, 3}:  Pass,
	{12, 4}:  Pass,
	{12, 5}:  Pass,
	{12, 6}:  Pass,
	{12, 7}:  Pass,
	{12, 8}:  Pass,
	{12, 9}:  AnvlSkip, // Router test but this is the host suite.
	{13, 1}:  AnvlSkip, // Router test but this is the host suite.
	{13, 2}:  AnvlSkip, // Router test but this is the host suite.
	{13, 3}:  AnvlSkip, // Router test but this is the host suite.
	{13, 4}:  AnvlSkip, // Router test but this is the host suite.
	{13, 5}:  AnvlSkip, // Router test but this is the host suite.
	{13, 6}:  AnvlSkip, // Router test but this is the host suite.
	{13, 7}:  AnvlSkip, // Router test but this is the host suite.
	{13, 8}:  AnvlSkip, // Router test but this is the host suite.
	{13, 9}:  AnvlSkip, // Router test but this is the host suite.
	{13, 10}: AnvlSkip, // Router test but this is the host suite.
	{13, 11}: AnvlSkip, // Router test but this is the host suite.
	{13, 12}: AnvlSkip, // Router test but this is the host suite.
	{13, 13}: AnvlSkip, // Router test but this is the host suite.
	{13, 14}: AnvlSkip, // Router test but this is the host suite.
	{13, 15}: AnvlSkip, // Router test but this is the host suite.
	{13, 16}: AnvlSkip, // Router test but this is the host suite.
	{13, 17}: AnvlSkip, // Router test but this is the host suite.
	{13, 18}: AnvlSkip, // Router test but this is the host suite.
	{14, 1}:  AnvlSkip, // Router test but this is the host suite.
	{14, 2}:  AnvlSkip, // Router test but this is the host suite.
	{15, 1}:  AnvlSkip, // Router test but this is the host suite.
	{16, 1}:  Pass,
	{16, 2}:  AnvlSkip, // Router test but this is the host suite.
	{16, 3}:  AnvlSkip, // Router test but this is the host suite.
	{17, 1}:  AnvlSkip, // Router test but this is the host suite.
	{17, 2}:  AnvlSkip, // Router test but this is the host suite.
	{17, 3}:  AnvlSkip, // Router test but this is the host suite.
	{17, 4}:  AnvlSkip, // Router test but this is the host suite.
	{18, 1}:  Pass,
	{18, 2}:  AnvlSkip, // Router test but this is the host suite.
	{18, 3}:  AnvlSkip, // Router test but this is the host suite.
	{18, 4}:  AnvlSkip, // Router test but this is the host suite.
	{18, 5}:  AnvlSkip, // Router test but this is the host suite.
	{18, 6}:  AnvlSkip, // Router test but this is the host suite.
	{18, 7}:  AnvlSkip, // Router test but this is the host suite.
	{18, 8}:  AnvlSkip, // Router test but this is the host suite.
	{18, 9}:  AnvlSkip, // Router test but this is the host suite.
	{19, 1}:  AnvlSkip, // Router test but this is the host suite.
	{20, 1}:  Pass,
	{21, 1}:  Pass,
	{21, 2}:  Inconclusive,
	{21, 3}:  Pass,
	{21, 4}:  Pass,
	{21, 5}:  Pass,
	{21, 6}:  Inconclusive,
	{21, 7}:  Inconclusive,
	{21, 8}:  Inconclusive,
	{21, 9}:  Pass,
	{21, 10}: Pass,
	{21, 11}: Pass,
	{21, 12}: Pass,
	{21, 13}: Pass,
	{21, 14}: Fail,
	{21, 15}: Inconclusive,
	{21, 16}: Inconclusive,
	{21, 17}: Fail,
	{21, 18}: Pass,
	{22, 1}:  Pass,
	{22, 2}:  Fail,
	{23, 1}:  Pass,
	{23, 2}:  Pass,
	{23, 3}:  Pass,
	{23, 4}:  Pass,
	{23, 5}:  Pass,
	{23, 6}:  Pass,
	{23, 7}:  Pass,
	{23, 8}:  Pass,
	{23, 10}: Pass,
	{23, 11}: Pass,
	{23, 12}: Pass,
	{23, 13}: Pass,
	{24, 1}:  Pass,
	{24, 2}:  Pass,
	{24, 3}:  Pass,
	{24, 4}:  Pass,
	{24, 5}:  Pass,
	{24, 6}:  Pass,
	{24, 7}:  Pass,
	{24, 8}:  Pass,
	{24, 9}:  Pass,
	{24, 10}: Pass,
	{24, 11}: Pass,
	{25, 1}:  Pass,
	{25, 2}:  Pass,
	{26, 1}:  Pass,
	{26, 2}:  Pass,
	{27, 1}:  Pass,
	{27, 2}:  AnvlSkip, // Router test but this is the host suite.
	{27, 3}:  AnvlSkip, // Router test but this is the host suite.
	{27, 4}:  Pass,
	{27, 5}:  Pass,
	{27, 6}:  Pass,
	{28, 1}:  Pass,
	{28, 4}:  Pass,
	{28, 5}:  Fail,
	{28, 6}:  Pass,
	{29, 1}:  Pass,
	{29, 2}:  AnvlSkip, // Router test but this is the host suite.
	{29, 3}:  Pass,
	{29, 4}:  Pass,
	{29, 5}:  Pass,
	{30, 1}:  Pass,
	{30, 2}:  Pass,
	{30, 3}:  Pass,
	{30, 4}:  Inconclusive,
	{30, 5}:  Pass,
	{30, 6}:  Pass,
	{30, 7}:  Pass,
	{30, 8}:  Pass,
	{30, 9}:  Pass,
	{30, 10}: Pass,
	{30, 11}: Pass,
	{30, 13}: Pass,
	{30, 14}: Pass,
	{30, 15}: Pass,
	{30, 16}: Pass,
	{30, 17}: Pass,
	{31, 1}:  AnvlSkip, // Router test but this is the host suite.
	{32, 1}:  Pass,
	{32, 2}:  Pass,
	{32, 3}:  Pass,
	{32, 4}:  Pass,
	{33, 1}:  Pass,
	{33, 2}:  Pass,
	{33, 3}:  Pass,
	{33, 4}:  Pass,
	{33, 5}:  Pass,
	{33, 6}:  Pass,
	{33, 7}:  Pass,
	{33, 8}:  Pass,
	{33, 9}:  Pass,
	{33, 10}: Pass,
	{33, 11}: Inconclusive,
	{33, 12}: Inconclusive,
	{33, 13}: Inconclusive,
	{33, 14}: AnvlSkip, // Router test but this is the host suite.
	{33, 15}: AnvlSkip, // Router test but this is the host suite.
	{34, 1}:  AnvlSkip, // Router test but this is the host suite.
	{34, 2}:  AnvlSkip, // Router test but this is the host suite.
	{35, 1}:  Pass,
	{35, 2}:  Pass,
	{35, 3}:  Pass,
	{35, 5}:  Pass,
	{35, 6}:  Pass,
}

var ipv6ndpExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{2, 1}:  Pass,
	{2, 2}:  AnvlSkip, // Router test but this is the host suite.
	{2, 3}:  Pass,
	{2, 4}:  AnvlSkip, // Router test but this is the host suite.
	{3, 1}:  AnvlSkip, // Router test but this is the host suite.
	{3, 2}:  AnvlSkip, // Router test but this is the host suite.
	{3, 3}:  AnvlSkip, // Router test but this is the host suite.
	{3, 4}:  AnvlSkip, // Router test but this is the host suite.
	{3, 5}:  Pass,
	{3, 6}:  Pass,
	{3, 7}:  Pass,
	{3, 8}:  AnvlSkip, // Router test but this is the host suite.
	{3, 9}:  Pass,
	{4, 1}:  Pass,
	{4, 2}:  Pass,
	{4, 3}:  Pass,
	{4, 4}:  Pass,
	{4, 5}:  Pass,
	{4, 6}:  Pass,
	{4, 7}:  Pass,
	{4, 8}:  Pass,
	{4, 9}:  Pass,
	{4, 10}: Pass,
	{4, 11}: Pass,
	{5, 1}:  Pass,
	{5, 2}:  Pass,
	{5, 3}:  Pass,
	{5, 4}:  Pass,
	{5, 5}:  AnvlSkip, // Router test but this is the host suite.
	{5, 6}:  Pass,
	{5, 7}:  Pass,
	{5, 8}:  Pass,
	{5, 9}:  AnvlSkip, // Router test but this is the host suite.
	{5, 10}: Pass,
	{5, 12}: Pass,
	{5, 13}: Pass,
	{5, 14}: Pass,
	{5, 15}: Pass,
	{5, 16}: Pass,
	{5, 17}: Pass,
	{6, 1}:  AnvlSkip, // Router test but this is the host suite.
	{6, 2}:  AnvlSkip, // Router test but this is the host suite.
	{6, 3}:  AnvlSkip, // Router test but this is the host suite.
	{6, 4}:  Pass,
	{6, 5}:  AnvlSkip, // Router test but this is the host suite.
	{6, 6}:  AnvlSkip, // Router test but this is the host suite.
	{6, 7}:  AnvlSkip, // Router test but this is the host suite.
	{6, 8}:  AnvlSkip, // Router test but this is the host suite.
	{6, 9}:  AnvlSkip, // Router test but this is the host suite.
	{7, 1}:  AnvlSkip, // Router test but this is the host suite.
	// TODO(https://fxbug.dev/42078196): Test has a script problem this should
	// be pass once that's resolved.
	{7, 2}: Flaky,
	{7, 3}: AnvlSkip, // Router test but this is the host suite.
	{7, 4}: AnvlSkip, // Router test but this is the host suite.
	{7, 5}: Pass,
	{7, 6}: AnvlSkip, // Router test but this is the host suite.
	{8, 1}: AnvlSkip, // Router test but this is the host suite.
	// TODO(https://fxbug.dev/42095002): Support NDP Redirect Messages.
	{8, 2}:   Inconclusive,
	{9, 1}:   AnvlSkip, // Router test but this is the host suite.
	{9, 2}:   AnvlSkip, // Router test but this is the host suite.
	{10, 1}:  Pass,
	{10, 2}:  Pass,
	{11, 1}:  Pass,
	{11, 2}:  AnvlSkip, // Router test but this is the host suite.
	{11, 3}:  AnvlSkip, // Router test but this is the host suite.
	{11, 4}:  AnvlSkip, // Router test but this is the host suite.
	{11, 5}:  AnvlSkip, // Router test but this is the host suite.
	{11, 6}:  AnvlSkip, // Router test but this is the host suite.
	{11, 7}:  AnvlSkip, // Router test but this is the host suite.
	{11, 8}:  AnvlSkip, // Router test but this is the host suite.
	{11, 9}:  AnvlSkip, // Router test but this is the host suite.
	{11, 10}: AnvlSkip, // Router test but this is the host suite.
	{11, 11}: AnvlSkip, // Router test but this is the host suite.
	{11, 12}: Pass,
	{12, 1}:  Pass,
	{12, 2}:  Pass,
	{12, 3}:  Pass,
	{12, 4}:  Pass,
	{12, 5}:  Pass,
	{12, 6}:  Pass,
	{12, 7}:  Pass,
	{12, 8}:  Pass,
	{12, 9}:  AnvlSkip, // Router test but this is the host suite.
	{13, 1}:  AnvlSkip, // Router test but this is the host suite.
	{13, 2}:  AnvlSkip, // Router test but this is the host suite.
	{13, 3}:  AnvlSkip, // Router test but this is the host suite.
	{13, 4}:  AnvlSkip, // Router test but this is the host suite.
	{13, 5}:  AnvlSkip, // Router test but this is the host suite.
	{13, 6}:  AnvlSkip, // Router test but this is the host suite.
	{13, 7}:  AnvlSkip, // Router test but this is the host suite.
	{13, 8}:  AnvlSkip, // Router test but this is the host suite.
	{13, 9}:  AnvlSkip, // Router test but this is the host suite.
	{13, 10}: AnvlSkip, // Router test but this is the host suite.
	{13, 11}: AnvlSkip, // Router test but this is the host suite.
	{13, 12}: AnvlSkip, // Router test but this is the host suite.
	{13, 13}: AnvlSkip, // Router test but this is the host suite.
	{13, 14}: AnvlSkip, // Router test but this is the host suite.
	{13, 15}: AnvlSkip, // Router test but this is the host suite.
	{13, 16}: AnvlSkip, // Router test but this is the host suite.
	{13, 17}: AnvlSkip, // Router test but this is the host suite.
	{13, 18}: AnvlSkip, // Router test but this is the host suite.
	{14, 1}:  AnvlSkip, // Router test but this is the host suite.
	{14, 2}:  AnvlSkip, // Router test but this is the host suite.
	{15, 1}:  AnvlSkip, // Router test but this is the host suite.
	{16, 1}:  Pass,
	{16, 2}:  AnvlSkip, // Router test but this is the host suite.
	{16, 3}:  AnvlSkip, // Router test but this is the host suite.
	{17, 1}:  AnvlSkip, // Router test but this is the host suite.
	{17, 2}:  AnvlSkip, // Router test but this is the host suite.
	{17, 3}:  AnvlSkip, // Router test but this is the host suite.
	{17, 4}:  AnvlSkip, // Router test but this is the host suite.
	{18, 1}:  Pass,
	{18, 2}:  AnvlSkip, // Router test but this is the host suite.
	{18, 3}:  AnvlSkip, // Router test but this is the host suite.
	{18, 4}:  AnvlSkip, // Router test but this is the host suite.
	{18, 5}:  AnvlSkip, // Router test but this is the host suite.
	{18, 6}:  AnvlSkip, // Router test but this is the host suite.
	{18, 7}:  AnvlSkip, // Router test but this is the host suite.
	{18, 8}:  AnvlSkip, // Router test but this is the host suite.
	{18, 9}:  AnvlSkip, // Router test but this is the host suite.
	{19, 1}:  AnvlSkip, // Router test but this is the host suite.
	{20, 1}:  Pass,
	{21, 1}:  Pass,
	{21, 2}:  Pass,
	{21, 3}:  Pass,
	{21, 4}:  Pass,
	{21, 5}:  Pass,
	// TODO(https://fxbug.dev/469720574): Stale default routes from 21.4 + 21.5
	// contaminate this test. Support for Default Route Selection will fix this.
	{21, 6}: Inconclusive,
	// TODO(https://fxbug.dev/469720574): Stale default routes from 21.4 + 21.5
	// contaminate this test. Support for Default Route Selection will fix this.
	{21, 7}: Inconclusive,
	// TODO(https://fxbug.dev/469720574): Stale default routes from 21.4 + 21.5
	// contaminate this test. Support for Default Route Selection will fix this.
	{21, 8}:  Inconclusive,
	{21, 9}:  Pass,
	{21, 10}: Pass,
	{21, 11}: Pass,
	{21, 12}: Pass,
	{21, 13}: Pass,
	// TODO(https://fxbug.dev/469720574): Stale default routes from 21.4 + 21.5
	// contaminate this test. Support for Default Route Selection will fix this.
	{21, 14}: Fail,
	// TODO(https://fxbug.dev/469720574): Stale default routes from 21.4 + 21.5
	// contaminate this test. Support for Default Route Selection will fix this.
	{21, 15}: Inconclusive,
	// TODO(https://fxbug.dev/469720574): Stale default routes from 21.4 + 21.5
	// contaminate this test. Support for Default Route Selection will fix this.
	{21, 16}: Inconclusive,
	// TODO(https://fxbug.dev/469720574): Stale default routes from 21.4 + 21.5
	// contaminate this test. Support for Default Route Selection will fix this.
	{21, 17}: Flaky,
	{21, 18}: Pass,
	// TODO(https://fxbug.dev/469720574): Support Default Route Selection.
	{22, 1}: Flaky,
	// TODO(https://fxbug.dev/469720574): Support Default Route Selection.
	{22, 2}:  Fail,
	{23, 1}:  Pass,
	{23, 2}:  Pass,
	{23, 3}:  Pass,
	{23, 4}:  Pass,
	{23, 5}:  Pass,
	{23, 6}:  Pass,
	{23, 7}:  Pass,
	{23, 8}:  Pass,
	{23, 10}: Pass,
	{23, 11}: Pass,
	{23, 12}: Pass,
	{23, 13}: Pass,
	{24, 1}:  Pass,
	{24, 2}:  Pass,
	{24, 3}:  Pass,
	{24, 4}:  Pass,
	{24, 5}:  Pass,
	{24, 6}:  Pass,
	{24, 7}:  Pass,
	{24, 8}:  Pass,
	{24, 9}:  Pass,
	{24, 10}: Pass,
	{24, 11}: Pass,
	{25, 1}:  Pass,
	{25, 2}:  Pass,
	{26, 1}:  Pass,
	{26, 2}:  Pass,
	{27, 1}:  Pass,
	{27, 2}:  AnvlSkip, // Router test but this is the host suite.
	{27, 3}:  AnvlSkip, // Router test but this is the host suite.
	{27, 4}:  Pass,
	{27, 5}:  Pass,
	{27, 6}:  Pass,
	{28, 1}:  Pass,
	{28, 4}:  Pass,
	// TODO(https://fxbug.dev/42083367): Implement the IsRouter flag.
	{28, 5}: Fail,
	{28, 6}: Pass,
	{29, 1}: Pass,
	{29, 2}: AnvlSkip, // Router test but this is the host suite.
	{29, 3}: Pass,
	{29, 4}: Pass,
	{29, 5}: Pass,
	{30, 1}: Pass,
	{30, 2}: Pass,
	{30, 3}: Pass,
	// The test makes assertions that are not grounded in RFC text. When the DUT
	// transitions from UNREACHABLE to STALE, it sends any packets that had
	// been queued. The test asserts that the sending of queued packets then
	// causes the DUT to enter DELAY, and after a wait, PROBE. The RFC does not
	// state to do this, and as such our implementation remains in STALE.
	//
	// See https://fxbug.dev/455545032#comment9 for details.
	{30, 4}:  Inconclusive,
	{30, 5}:  Pass,
	{30, 6}:  Pass,
	{30, 7}:  Pass,
	{30, 8}:  Pass,
	{30, 9}:  Pass,
	{30, 10}: Pass,
	{30, 11}: Pass,
	{30, 13}: Pass,
	{30, 14}: Pass,
	{30, 15}: Pass,
	{30, 16}: Pass,
	{30, 17}: Pass,
	{31, 1}:  AnvlSkip, // Router test but this is the host suite.
	{32, 1}:  Pass,
	{32, 2}:  Pass,
	{32, 3}:  Pass,
	{32, 4}:  Pass,
	{33, 1}:  Pass,
	{33, 2}:  Pass,
	{33, 3}:  Pass,
	{33, 4}:  Pass,
	{33, 5}:  Pass,
	{33, 6}:  Pass,
	{33, 7}:  Pass,
	{33, 8}:  Pass,
	{33, 9}:  Pass,
	{33, 10}: Pass,
	// TODO(https://fxbug.dev/42095002): Support NDP Redirect Messages.
	{33, 11}: Inconclusive,
	// TODO(https://fxbug.dev/42095002): Support NDP Redirect Messages.
	{33, 12}: Inconclusive,
	// TODO(https://fxbug.dev/42095002): Support NDP Redirect Messages.
	{33, 13}: Inconclusive,
	{33, 14}: AnvlSkip, // Router test but this is the host suite.
	{33, 15}: AnvlSkip, // Router test but this is the host suite.
	{34, 1}:  AnvlSkip, // Router test but this is the host suite.
	{34, 2}:  AnvlSkip, // Router test but this is the host suite.
	{35, 1}:  Pass,
	{35, 2}:  Pass,
	{35, 3}:  Pass,
	{35, 5}:  Pass,
	{35, 6}:  Pass,
}
