// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var tcpAdvancedV6Expectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 17}:  Fail,
	{1, 18}:  Fail,
	{2, 18}:  Fail,
	{2, 19}:  Fail,
	{2, 20}:  Fail,
	{2, 21}:  Fail,
	{2, 22}:  Fail,
	{2, 23}:  Fail,
	{2, 24}:  Fail,
	{2, 25}:  Fail,
	{3, 17}:  Flaky, // TODO(https://fxbug.dev/450348456): Fix the flake.
	{4, 17}:  Flaky,
	{5, 17}:  Skip, // https://fxbug.dev/427248980: Causes ANVL exits.
	{5, 18}:  Skip, // https://fxbug.dev/427248980: Causes ANVL exits.
	{5, 19}:  Skip, // https://fxbug.dev/427248980: Causes ANVL exits.
	{5, 20}:  Skip, // https://fxbug.dev/427248980: Causes ANVL exits.
	{5, 21}:  Skip, // https://fxbug.dev/427248980: Causes ANVL exits.
	{5, 22}:  Skip, // https://fxbug.dev/427248980: Causes ANVL exits.
	{5, 23}:  Skip, // https://fxbug.dev/427248980: Causes ANVL exits.
	{6, 17}:  Pass,
	{7, 17}:  Skip,     // https://fxbug.dev/427248980: Causes ANVL exits.
	{7, 18}:  Skip,     // https://fxbug.dev/427248980: Causes ANVL exits.
	{8, 17}:  Skip,     // https://fxbug.dev/427248980: Causes ANVL exits.
	{8, 18}:  Skip,     // https://fxbug.dev/427248980: Causes ANVL exits.
	{8, 19}:  Skip,     // https://fxbug.dev/427248980: Causes ANVL exits.
	{8, 20}:  Skip,     // https://fxbug.dev/434686230: Causes ANVL exits.
	{8, 21}:  Skip,     // https://fxbug.dev/427248980: Causes ANVL exits.
	{8, 22}:  Skip,     // https://fxbug.dev/427248980: Causes ANVL exits.
	{8, 23}:  Skip,     // https://fxbug.dev/427248980: Causes ANVL exits.
	{9, 1}:   AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{9, 2}:   AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 2}:  AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 17}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 18}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 19}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 20}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{11, 17}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{12, 2}:  AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{12, 17}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{13, 1}:  AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{14, 17}: Skip,     // TODO(https://fxbug.dev/400788238): ANVL exits with abnormal code.
	{14, 19}: Skip,     // TODO(https://fxbug.dev/400788238): ANVL exits with abnormal code.
	{14, 20}: Fail,
	{15, 17}: Fail,
	{15, 18}: Pass,
	{15, 19}: Pass,
	{15, 20}: Pass,
	{16, 1}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 2}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 3}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 4}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 5}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 6}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{17, 1}:  Pass,
	{17, 17}: Pass,
	{17, 18}: Fail,
	{17, 19}: Pass,
	{17, 20}: Pass,
	{17, 21}: Pass,
	{17, 22}: Pass,
	{17, 23}: Pass,
	{17, 24}: Fail,
}

var tcpAdvancedV6ExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
	{1, 17}:  Fail,
	{1, 18}:  Fail,
	{2, 18}:  Fail,
	{2, 19}:  Flaky, // TODO(https://fxbug.dev/356692923): This should be a consistent failure.
	{2, 20}:  Fail,
	{2, 21}:  Fail,
	{2, 22}:  Fail,
	{2, 23}:  Fail,
	{2, 24}:  Fail,
	{2, 25}:  Fail,
	{3, 17}:  Flaky, // TODO(https://fxbug.dev/450348456): Fix the flake.
	{4, 17}:  Pass,
	{5, 17}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{5, 18}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{5, 19}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{5, 20}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{5, 21}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{5, 22}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{5, 23}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{6, 17}:  Pass,
	{7, 17}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{7, 18}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{8, 17}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{8, 18}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{8, 19}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{8, 20}:  Skip, // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{8, 21}:  Fail,
	{8, 22}:  Skip,     // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{8, 23}:  Skip,     // TODO(https://fxbug.dev/42095226): Implement IP_MTU_DISCOVER.
	{9, 1}:   AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{9, 2}:   AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 2}:  AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 17}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 18}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 19}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{10, 20}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{11, 17}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{12, 2}:  AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{12, 17}: AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{13, 1}:  AnvlSkip, // Tests RFC 2385 TCP MD5 Signature Option which is obsolete.
	{14, 17}: Skip,     // TODO(https://fxbug.dev/400788238): ANVL exits with abnormal code.
	{14, 19}: Pass,
	{14, 20}: Fail,
	{15, 17}: Fail,
	{15, 18}: Pass,
	{15, 19}: Pass,
	{15, 20}: Pass,
	{16, 1}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 2}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 3}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 4}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 5}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 6}:  Flaky, // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{17, 1}:  Fail,
	{17, 17}: Pass,
	{17, 18}: Fail,
	{17, 19}: Pass,
	{17, 20}: Pass,
	{17, 21}: Pass,
	{17, 22}: Pass,
	{17, 23}: Fail,
	{17, 24}: Fail,
}
