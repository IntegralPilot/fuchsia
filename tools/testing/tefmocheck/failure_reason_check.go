// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"path"
	"strings"
)

// testSuiteFailureReasonCheck checks if String is found in the failure reason of a specific targetTestName.
type testSuiteFailureReasonCheck struct {
	baseCheck
	String         string
	targetTestName string
	isExoneration  bool

	testName      string
	failureReason string
}

func (c *testSuiteFailureReasonCheck) Check(to *TestingOutputs) bool {
	c.testName = ""
	c.failureReason = ""

	if to.TestSummary == nil {
		return false
	}

	for _, test := range to.TestSummary.Tests {
		if !strings.Contains(test.Name, c.targetTestName) {
			continue
		}

		if test.FailureReason != nil {
			for _, err := range test.FailureReason.Errors {
				if err != nil && strings.Contains(err.Message, c.String) {
					c.testName = test.Name
					c.failureReason = err.Message
					return true
				}
			}
		}

		for _, tc := range test.Cases {
			if tc.FailureReason != nil {
				for _, err := range tc.FailureReason.Errors {
					if err != nil && strings.Contains(err.Message, c.String) {
						c.testName = test.Name
						c.failureReason = err.Message
						return true
					}
				}
			}
		}
	}
	return false
}

func (c *testSuiteFailureReasonCheck) Name() string {
	return path.Join("test_suite_failure_reason", c.targetTestName, strings.ReplaceAll(c.String, " ", "_"))
}

func (c *testSuiteFailureReasonCheck) IsExoneration() bool {
	return c.isExoneration
}

func (c *testSuiteFailureReasonCheck) TestName() string {
	return c.testName
}

func (c *testSuiteFailureReasonCheck) FailureReason() string {
	return c.failureReason
}

// FailureReasonChecks returns checks to detect bad strings in failure reasons of test suites.
func FailureReasonChecks() []FailureModeCheck {
	return []FailureModeCheck{
		// For b/502668022 (staging verification)
		&testSuiteFailureReasonCheck{
			String:         "[DeviceDidNotSuspendError] Dummy exception raised to demonstrate test exoneration in tefmocheck. 8d3d922a-8c8c-44bb-bc8c-f09c7a72d733",
			isExoneration:  true,
			targetTestName: "exoneration_failing_test",
		},
	}
}
