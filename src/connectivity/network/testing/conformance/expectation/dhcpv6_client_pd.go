// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var dhcpv6ClientPDExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 1}:  Pass,
	{2, 1}:  Pass,
	{2, 2}:  Pass,
	{2, 3}:  Pass,
	{2, 4}:  Fail,
	{3, 1}:  Pass,
	{3, 2}:  Pass,
	{3, 3}:  Pass,
	{3, 4}:  Pass,
	{3, 5}:  Fail,
	{3, 6}:  Pass,
	{3, 7}:  Pass,
	{3, 8}:  Pass,
	{4, 1}:  Pass,
	{4, 2}:  Pass,
	{4, 3}:  Pass,
	{5, 1}:  Inconclusive,
	{5, 2}:  Inconclusive,
	{5, 3}:  Inconclusive,
	{6, 1}:  Pass,
	{6, 2}:  Pass,
	{6, 3}:  Pass,
	{7, 1}:  Pass,
	{7, 2}:  Pass,
	{7, 3}:  Inconclusive,
	{7, 4}:  Skip, // TODO(https://fxbug.dev/433786237): Fix ANVL binary exiting unexpectedly.
	{7, 5}:  Pass,
	{7, 6}:  Inconclusive,
	{7, 7}:  Inconclusive,
	{7, 8}:  Pass,
	{7, 9}:  Pass,
	{7, 10}: Pass,
	{7, 11}: Inconclusive,
	{7, 12}: Fail,
	{7, 13}: Fail,
}
