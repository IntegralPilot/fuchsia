// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var ipv6AutoconfigExpectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{1, 2}:  Pass,
	{1, 3}:  Pass,
	{1, 4}:  Pass,
	{1, 5}:  Pass,
	{2, 1}:  Pass,
	{2, 2}:  Skip, // Skipped due to lack of support for setting DAD transmits.
	{2, 3}:  Pass,
	{3, 1}:  Skip,     // Skipped due to lack of support for setting DAD transmits.
	{3, 2}:  AnvlSkip, // TODO(https://fxbug.dev/416093959): Need router version of this suite.
	{3, 3}:  Pass,
	{3, 4}:  Pass,
	{3, 5}:  Pass,
	{4, 1}:  Pass,
	{4, 2}:  Pass,
	{5, 1}:  Skip, // Skipped due to lack of support for setting DAD transmits.
	{5, 2}:  Pass,
	{5, 3}:  Pass,
	{5, 4}:  Pass,
	{6, 1}:  Pass,
	{6, 2}:  Fail,
	{6, 3}:  Pass,
	{7, 1}:  Pass,
	{7, 2}:  Pass,
	{8, 1}:  Pass,
	{8, 2}:  Pass,
	{8, 3}:  Pass,
	{8, 4}:  Pass,
	{8, 5}:  Pass,
	{8, 6}:  Pass,
	{8, 7}:  Pass,
	{8, 8}:  Pass,
	{8, 9}:  Fail,
	{8, 10}: Pass,
	{9, 1}:  Pass,
	{9, 2}:  Pass,
	{10, 1}: Fail,
}

var ipv6AutoconfigExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}: Pass,
	{1, 2}: Pass,
	{1, 3}: Pass,
	{1, 4}: Pass,
	{1, 5}: Pass,
	{2, 1}: Pass,
	// This test fails because ANVL observes an unexpected Neighbor Solicitation
	// from the netstack. The unexpected solicitation is for the interface's
	// link-local address, which is undergoing DAD. This solicitation is in-line
	// with expected behavior, and is infeasible to suppress. See
	// https://fxbug.dev/454298676#comment2.
	{2, 2}: Fail,
	{2, 3}: Pass,
	{3, 1}: Pass,
	{3, 2}: AnvlSkip, // TODO(https://fxbug.dev/416093959): Need router version of this suite.
	{3, 3}: Pass,
	{3, 4}: Pass,
	{3, 5}: Pass,
	{4, 1}: Pass,
	{4, 2}: Pass,
	{5, 1}: Pass,
	{5, 2}: Pass,
	{5, 3}: Pass,
	{5, 4}: Pass,
	{6, 1}: Pass,
	// TODO(https://fxbug.dev/454651266): Delay the start of DAD. This test
	// generates a conflicting Neighbor Solicitations while the DUT is
	// initializing DAD for a tentative address. It expects the DUT to recognize
	// the conflict before sending the first Neighbor Solicitation, and abandon
	// the address. This currently fails, because the DUT sends its Neighbor
	// Solicitation immediately rather than after a randomized delay.
	{6, 2}:  Fail,
	{6, 3}:  Pass,
	{7, 1}:  Pass,
	{7, 2}:  Pass,
	{8, 1}:  Pass,
	{8, 2}:  Pass,
	{8, 3}:  Pass,
	{8, 4}:  Pass,
	{8, 5}:  Pass,
	{8, 6}:  Pass,
	{8, 7}:  Pass,
	{8, 8}:  Pass,
	{8, 9}:  Pass,
	{8, 10}: Pass,
	{9, 1}:  Pass,
	{9, 2}:  Pass,
	{10, 1}: Pass,
}
