// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"testing"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"

	"github.com/google/go-cmp/cmp"
)

func TestNearbyStringCheck(t *testing.T) {
	tests := []struct {
		name            string
		log             string
		check           *NearbyStringCheck
		expectedCheck   bool
		expectedName    string
		expectedFailure string
		expectedLine1   string
		expectedLine2   string
	}{
		{
			name: "strings within distance",
			log: `line 1
string1
line 3
string2
line 5`,
			check: &NearbyStringCheck{
				String1:          "string1",
				String2:          "string2",
				MaxDistanceLines: 2,
				Type:             serialLogType,
			},
			expectedCheck:   true,
			expectedName:    "nearby_string/serial_log.txt/string1/string2",
			expectedFailure: "Found lines nearby:\nstring1\nstring2",
			expectedLine1:   "string1",
			expectedLine2:   "string2",
		},
		{
			name: "strings outside distance",
			log: `string1
line 2
line 3
line 4
string2`,
			check: &NearbyStringCheck{
				String1:          "string1",
				String2:          "string2",
				MaxDistanceLines: 2,
				Type:             serialLogType,
			},
			expectedCheck: false,
		},
		{
			name: "strings in reverse order within distance",
			log: `string2
line 2
string1`,
			check: &NearbyStringCheck{
				String1:          "string1",
				String2:          "string2",
				MaxDistanceLines: 2,
				Type:             serialLogType,
			},
			expectedCheck:   true,
			expectedName:    "nearby_string/serial_log.txt/string1/string2",
			expectedFailure: "Found lines nearby:\nstring1\nstring2",
			expectedLine1:   "string1",
			expectedLine2:   "string2",
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			outputs := &TestingOutputs{
				SerialLogs:      [][]byte{[]byte(test.log)},
				SwarmingSummary: &SwarmingTaskSummary{Results: &SwarmingRpcsTaskResult{}},
			}
			if got := test.check.Check(outputs); got != test.expectedCheck {
				t.Errorf("Check() = %v, want %v", got, test.expectedCheck)
			}
			if test.expectedCheck {
				if name := test.check.Name(); name != test.expectedName {
					t.Errorf("Name() = %q, want %q", name, test.expectedName)
				}
				if reason := test.check.FailureReason(); reason != test.expectedFailure {
					t.Errorf("FailureReason() = %q, want %q", reason, test.expectedFailure)
				}
				if test.check.line1 != test.expectedLine1 {
					t.Errorf("line1 = %q, want %q", test.check.line1, test.expectedLine1)
				}
				if test.check.line2 != test.expectedLine2 {
					t.Errorf("line2 = %q, want %q", test.check.line2, test.expectedLine2)
				}
			}
		})
	}
}

func TestNearbyStringCheckWithSwarming(t *testing.T) {
	const (
		passedTest1 = "passedTest1"
		passedTest2 = "passedTest2"
		failedTest1 = "failedTest1"
		failedTest2 = "failedTest2"
	)
	failedTestLog := "string1\nstring2"
	passedTestLog := "nothing interesting"
	testCases := []struct {
		name                  string
		check                 *NearbyStringCheck
		swarmingResult        *SwarmingRpcsTaskResult
		testSummary           *runtests.TestSummary
		swarmingOutputPerTest []TestLog
		expectedCheck         bool
		expectedIsFlake       bool
	}{
		{
			name: "SkipPassedTask returns false for passed task",
			check: &NearbyStringCheck{
				String1:          "string1",
				String2:          "string2",
				MaxDistanceLines: 20,
				SkipPassedTask:   true,
				Type:             swarmingOutputType,
			},
			swarmingResult: &SwarmingRpcsTaskResult{State: "COMPLETED"},
			expectedCheck:  false,
		},
		{
			name: "SkipPassedTask returns true for failed task",
			check: &NearbyStringCheck{
				String1:          "string1",
				String2:          "string2",
				MaxDistanceLines: 20,
				SkipPassedTask:   true,
				Type:             swarmingOutputType,
			},
			swarmingResult: &SwarmingRpcsTaskResult{State: "COMPLETED", Failure: true},
			expectedCheck:  true, // because the log contains the strings
		},
		{
			name: "SkipAllPassedTests returns false for all passed tests",
			check: &NearbyStringCheck{
				String1:            "string1",
				String2:            "string2",
				MaxDistanceLines:   20,
				SkipAllPassedTests: true,
				Type:               swarmingOutputType,
			},
			swarmingResult: &SwarmingRpcsTaskResult{State: "COMPLETED"},
			testSummary: &runtests.TestSummary{
				Tests: []runtests.TestDetails{
					{Name: passedTest1, Status: runtests.TestSuccess},
					{Name: passedTest2, Status: runtests.TestSuccess},
				},
			},
			expectedCheck: false,
		},
		{
			name: "SkipAllPassedTests returns true for some failed tests",
			check: &NearbyStringCheck{
				String1:            "string1",
				String2:            "string2",
				MaxDistanceLines:   20,
				SkipAllPassedTests: true,
				Type:               swarmingOutputType,
			},
			swarmingResult: &SwarmingRpcsTaskResult{State: "COMPLETED"},
			testSummary: &runtests.TestSummary{
				Tests: []runtests.TestDetails{
					{Name: passedTest1, Status: runtests.TestSuccess},
					{Name: failedTest1, Status: runtests.TestFailure},
				},
			},
			expectedCheck: true,
		},
		{
			name: "SkipAllPassedTests with IgnoreFlakes returns false",
			check: &NearbyStringCheck{
				String1:            "string1",
				String2:            "string2",
				MaxDistanceLines:   20,
				SkipAllPassedTests: true,
				IgnoreFlakes:       true,
				Type:               swarmingOutputType,
			},
			swarmingResult: &SwarmingRpcsTaskResult{State: "COMPLETED"},
			testSummary: &runtests.TestSummary{
				Tests: []runtests.TestDetails{
					{Name: failedTest1, Status: runtests.TestFailure},
					{Name: failedTest1, Status: runtests.TestSuccess},
				},
			},
			expectedCheck: false,
		},
		{
			name: "AlwaysFlake returns true and isFlake",
			check: &NearbyStringCheck{
				String1:          "string1",
				String2:          "string2",
				MaxDistanceLines: 20,
				AlwaysFlake:      true,
				Type:             swarmingOutputType,
			},
			swarmingResult:  &SwarmingRpcsTaskResult{State: "COMPLETED", Failure: true},
			expectedCheck:   true,
			expectedIsFlake: true,
		},
		{
			name: "SkipPassedTest finds in failed test",
			check: &NearbyStringCheck{
				String1:          "string1",
				String2:          "string2",
				MaxDistanceLines: 20,
				SkipPassedTest:   true,
				Type:             swarmingOutputType,
			},
			swarmingResult: &SwarmingRpcsTaskResult{State: "COMPLETED", Failure: true},
			testSummary: &runtests.TestSummary{
				Tests: []runtests.TestDetails{
					{Name: failedTest1, Status: runtests.TestFailure},
					{Name: passedTest1, Status: runtests.TestSuccess},
				},
			},
			swarmingOutputPerTest: []TestLog{
				{TestName: failedTest1, Bytes: []byte(failedTestLog)},
				{TestName: passedTest1, Bytes: []byte(passedTestLog)},
			},
			expectedCheck: true,
		},
		{
			name: "SkipPassedTest does not find in passed test",
			check: &NearbyStringCheck{
				String1:          "string1",
				String2:          "string2",
				MaxDistanceLines: 20,
				SkipPassedTest:   true,
				Type:             swarmingOutputType,
			},
			swarmingResult: &SwarmingRpcsTaskResult{State: "COMPLETED", Failure: true},
			testSummary: &runtests.TestSummary{
				Tests: []runtests.TestDetails{
					{Name: failedTest1, Status: runtests.TestFailure},
					{Name: passedTest1, Status: runtests.TestSuccess},
				},
			},
			swarmingOutputPerTest: []TestLog{
				{TestName: failedTest1, Bytes: []byte(passedTestLog)},
				{TestName: passedTest1, Bytes: []byte(failedTestLog)},
			},
			expectedCheck: false,
		},
		{
			name: "SkipPassedTest with IgnoreFlakes",
			check: &NearbyStringCheck{
				String1:          "string1",
				String2:          "string2",
				MaxDistanceLines: 20,
				SkipPassedTest:   true,
				IgnoreFlakes:     true,
				Type:             swarmingOutputType,
			},
			swarmingResult: &SwarmingRpcsTaskResult{State: "COMPLETED", Failure: true},
			testSummary: &runtests.TestSummary{
				Tests: []runtests.TestDetails{
					{Name: failedTest1, Status: runtests.TestFailure},
					{Name: failedTest2, Status: runtests.TestFailure},
					{Name: failedTest2, Status: runtests.TestSuccess},
				},
			},
			swarmingOutputPerTest: []TestLog{
				{TestName: failedTest1, Bytes: []byte(passedTestLog)},
				{TestName: failedTest2, Bytes: []byte(failedTestLog)},
				{TestName: failedTest2, Bytes: []byte(passedTestLog)},
			},
			expectedCheck:   true,
			expectedIsFlake: true,
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			outputs := &TestingOutputs{
				SwarmingOutput:        []byte(failedTestLog),
				SwarmingSummary:       &SwarmingTaskSummary{Results: tc.swarmingResult},
				TestSummary:           tc.testSummary,
				SwarmingOutputPerTest: tc.swarmingOutputPerTest,
			}
			if got := tc.check.Check(outputs); got != tc.expectedCheck {
				t.Errorf("Check() got %v, want %v", got, tc.expectedCheck)
			}
			if tc.expectedCheck {
				if diff := cmp.Diff(tc.expectedIsFlake, tc.check.IsFlake()); diff != "" {
					t.Errorf("IsFlake() mismatch (-want +got):\n%s", diff)
				}
			}
		})
	}
}
