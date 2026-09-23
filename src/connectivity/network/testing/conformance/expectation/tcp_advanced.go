// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"

var tcpAdvancedExpectations map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
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
	{3, 17}:  Flaky, // TODO(https://fxbug.dev/450348456): Fix flake.
	{4, 17}:  Pass,
	{5, 17}:  Fail,
	{5, 18}:  Fail,
	{5, 19}:  Fail,
	{5, 20}:  Fail,
	{5, 21}:  Fail,
	{5, 22}:  Fail,
	{5, 23}:  Fail,
	{6, 17}:  Pass,
	{7, 17}:  Fail,
	{7, 18}:  Fail,
	{8, 17}:  Fail,
	{8, 18}:  Fail,
	{8, 19}:  Fail,
	{8, 20}:  Fail,
	{8, 21}:  Fail,
	{8, 22}:  Fail,
	{8, 23}:  Fail,
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
	{14, 17}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{14, 19}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{14, 20}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{15, 17}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{15, 18}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{15, 19}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{15, 20}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{16, 1}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 2}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 3}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 4}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 5}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 6}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
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

var tcpAdvancedExpectationsNS3 map[AnvlCaseNumber]outcome.Outcome = map[AnvlCaseNumber]outcome.Outcome{
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
	{3, 17}:  Flaky, // TODO(https://fxbug.dev/450348456): Fix flake.
	{4, 17}:  Flaky,
	{5, 17}:  Fail,
	{5, 18}:  Fail,
	{5, 19}:  Fail,
	{5, 20}:  Fail,
	{5, 21}:  Fail,
	{5, 22}:  Fail,
	{5, 23}:  Fail,
	{6, 17}:  Pass,
	{7, 17}:  Fail,
	{7, 18}:  Fail,
	{8, 17}:  Fail,
	{8, 18}:  Fail,
	{8, 19}:  Fail,
	{8, 20}:  Fail,
	{8, 21}:  Fail,
	{8, 22}:  Fail,
	{8, 23}:  Fail,
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
	{14, 17}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{14, 19}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{14, 20}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{15, 17}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{15, 18}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{15, 19}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{15, 20}: AnvlSkip, // Test requires IPv6, but this is the IPv4 suite.
	{16, 1}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 2}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 3}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 4}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 5}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
	{16, 6}:  Flaky,    // TODO(https://fxbug.dev/414886191): Fix flakiness.
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
