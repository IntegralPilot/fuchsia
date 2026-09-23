// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"testing"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

func TestTestSuiteFailureReasonCheck(t *testing.T) {
	const killerString = "KILLER STRING"
	const targetTest = "my-target-test"
	const otherTest = "other-test"

	c := testSuiteFailureReasonCheck{
		String:         killerString,
		targetTestName: targetTest,
	}

	// Helper to create TestingOutputs with FailureReason or Cases
	createOutputs := func(tests []runtests.TestDetails, taskFailed bool) TestingOutputs {
		return TestingOutputs{
			TestSummary: &runtests.TestSummary{
				Tests: tests,
			},
			SwarmingSummary: &SwarmingTaskSummary{
				Results: &SwarmingRpcsTaskResult{
					TaskId:  "abc",
					State:   "COMPLETED",
					Failure: taskFailed,
				},
			},
		}
	}

	t.Run("should match if string is in target test FailureReason", func(t *testing.T) {
		to := createOutputs([]runtests.TestDetails{
			{
				Name:   targetTest,
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					FailureReason: runtests.FailureReasonFromMessage("some error: KILLER STRING"),
				},
			},
		}, true)
		if !c.Check(&to) {
			t.Error("expected check to match")
		}
		if c.TestName() != targetTest {
			t.Errorf("got TestName() = %q, want %q", c.TestName(), targetTest)
		}
		if c.FailureReason() != "some error: KILLER STRING" {
			t.Errorf("got FailureReason() = %q, want %q", c.FailureReason(), "some error: KILLER STRING")
		}
	})

	t.Run("should match if string is in target test case FailureReason", func(t *testing.T) {
		to := createOutputs([]runtests.TestDetails{
			{
				Name:   targetTest,
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{
						{
							DisplayName:   "case1",
							Status:        runtests.TestFailure,
							FailureReason: runtests.FailureReasonFromMessage("case error: KILLER STRING"),
						},
					},
				},
			},
		}, true)
		if !c.Check(&to) {
			t.Error("expected check to match")
		}
		if c.TestName() != targetTest {
			t.Errorf("got TestName() = %q, want %q", c.TestName(), targetTest)
		}
		if c.FailureReason() != "case error: KILLER STRING" {
			t.Errorf("got FailureReason() = %q, want %q", c.FailureReason(), "case error: KILLER STRING")
		}
	})

	t.Run("should match even if task passed", func(t *testing.T) {
		to := createOutputs([]runtests.TestDetails{
			{
				Name:   targetTest,
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					FailureReason: runtests.FailureReasonFromMessage("some error: KILLER STRING"),
				},
			},
		}, false) // taskFailed = false
		if !c.Check(&to) {
			t.Error("expected check to match")
		}
	})

	t.Run("should NOT match if string is in other test FailureReason", func(t *testing.T) {
		to := createOutputs([]runtests.TestDetails{
			{
				Name:   otherTest,
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					FailureReason: runtests.FailureReasonFromMessage("some error: KILLER STRING"),
				},
			},
			{
				Name:   targetTest,
				Status: runtests.TestFailure,
			},
		}, true)
		if c.Check(&to) {
			t.Error("expected check NOT to match")
		}
	})

	t.Run("should match using substring for targetTestName", func(t *testing.T) {
		to := createOutputs([]runtests.TestDetails{
			{
				Name:   "host_x64/obj/my-target-test.sh",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					FailureReason: runtests.FailureReasonFromMessage("some error: KILLER STRING"),
				},
			},
		}, true)
		if !c.Check(&to) {
			t.Error("expected check to match using substring")
		}
		wantName := "host_x64/obj/my-target-test.sh"
		if c.TestName() != wantName {
			t.Errorf("got TestName() = %q, want %q", c.TestName(), wantName)
		}
	})
}
