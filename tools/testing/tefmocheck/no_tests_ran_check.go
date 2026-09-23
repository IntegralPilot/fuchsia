// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

// NoTestsRanCheck checks whether the task reported running zero tests. It may
// actually have run tests but not reported them, which is still an issue.
type NoTestsRanCheck struct{ baseCheck }

func (c NoTestsRanCheck) Check(to *TestingOutputs) bool {
	return len(to.TestSummary.Tests) == 0
}

func (c NoTestsRanCheck) Name() string {
	return "no_tests_ran_or_cleanup_failed"
}

func (c NoTestsRanCheck) DebugText() string {
	return "The task didn't run any tests, didn't produce any test results, or failed to properly clean up its test artifacts."
}
