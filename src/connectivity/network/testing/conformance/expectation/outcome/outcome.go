// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package outcome

import (
	"fmt"
)

type Outcome int

const (
	_ Outcome = iota
	Pass
	Fail
	// No response should be considered a failure.
	NoResponse
	Inconclusive
	// Flaky is a special value that, when used as a test expectation, indicates
	// that we should accept any outcome as expected. Any expectation marked
	// as flaky should have an accompanying bug.
	// TODO(https://fxbug.dev/42180625): Align how flaky test cases are handled with
	// the rest of Fuchsia tests.
	Flaky
	// Skip means that we are electively skipping the test (usually because a
	// particular test case causes ANVL to crash or runs indefinitely)
	Skip
	// AnvlSkip means that a test case will be skipped by ANVL due to being not
	// applicable in a particular context, e.g. testing for behavior as a router
	// but we're configured as a host.
	AnvlSkip
)

func (o Outcome) String() string {
	switch o {
	case Pass:
		return "Passed"
	case Fail:
		return "!FAILED!"
	case NoResponse:
		return "!NO RESPONSE!"
	case Inconclusive:
		return "!INCONCLUSIVE!"
	case Flaky:
		return "Flaky"
	case Skip:
		return "Skip"
	case AnvlSkip:
		return "AnvlSkip"
	default:
		panic(fmt.Sprintf("unrecognized Outcome %d", o))
	}
}
