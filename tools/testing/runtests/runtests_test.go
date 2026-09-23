// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package runtests

import (
	"encoding/json"
	"testing"
	"time"
)

func TestTestSummarySerialization(t *testing.T) {
	exitCode := 1
	setupOk := true
	teardownOk := false

	summary := TestSummary{
		Tests: []TestDetails{
			{
				Name:           "fuchsia-pkg://fuchsia.com/my-test#meta/my-test.cm",
				GNLabel:        "//src/sys/test:my-test(//build/toolchain:fuchsia)",
				SourceLabel:    "//src/sys/test:my-test",
				Status:         TestFailure,
				StartTime:      time.Unix(1700000000, 0).UTC(),
				DurationMillis: 1234,
				TestResult: TestResult{
					OutputDir:         "my-test",
					OutputFiles:       []string{"stdout-and-stderr.txt"},
					FailureReason:     FailureReasonFromMessage("Assertion failed in main.cc:42"),
					SetupSucceeded:    &setupOk,
					TeardownSucceeded: &teardownOk,
					ExitCode:          &exitCode,
					Cases: []TestCaseResult{
						{
							DisplayName:   "TestFoo",
							SuiteName:     "MySuite",
							CaseName:      "TestFoo",
							Status:        TestFailure,
							Duration:      500 * time.Millisecond,
							Format:        "gtest",
							FailureReason: FailureReasonFromMessage("Expected true, got false"),
						},
					},
				},
			},
		},
		Outputs: map[string]string{
			"syslog.txt": "path/to/syslog.txt",
		},
	}

	data, err := json.Marshal(summary)
	if err != nil {
		t.Fatalf("json.Marshal failed: %v", err)
	}

	var parsed TestSummary
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("json.Unmarshal failed: %v", err)
	}

	if len(parsed.Tests) != 1 {
		t.Fatalf("expected 1 test, got %d", len(parsed.Tests))
	}

	test := parsed.Tests[0]
	if test.Name != summary.Tests[0].Name {
		t.Errorf("test.Name mismatch: got %q, want %q", test.Name, summary.Tests[0].Name)
	}
	if test.ExitCode == nil || *test.ExitCode != exitCode {
		t.Errorf("test.ExitCode mismatch: got %v, want %v", test.ExitCode, exitCode)
	}
	if test.SetupSucceeded == nil || *test.SetupSucceeded != setupOk {
		t.Errorf("test.SetupSucceeded mismatch: got %v, want %v", test.SetupSucceeded, setupOk)
	}
	if test.TeardownSucceeded == nil || *test.TeardownSucceeded != teardownOk {
		t.Errorf("test.TeardownSucceeded mismatch: got %v, want %v", test.TeardownSucceeded, teardownOk)
	}
	if test.FailureReason == nil || len(test.FailureReason.Errors) != 1 || test.FailureReason.Errors[0].Message != "Assertion failed in main.cc:42" {
		t.Errorf("test.FailureReason mismatch: got %+v", test.FailureReason)
	}
	if len(test.Cases) != 1 {
		t.Fatalf("expected 1 case, got %d", len(test.Cases))
	}
	if test.Cases[0].FailureReason == nil || len(test.Cases[0].FailureReason.Errors) != 1 || test.Cases[0].FailureReason.Errors[0].Message != "Expected true, got false" {
		t.Errorf("case.FailureReason mismatch: got %+v", test.Cases[0].FailureReason)
	}
}

func TestTestResultOmitEmpty(t *testing.T) {
	result := TestResult{
		OutputDir:   "test_dir",
		OutputFiles: []string{"output.txt"},
	}

	data, err := json.Marshal(result)
	if err != nil {
		t.Fatalf("json.Marshal failed: %v", err)
	}

	jsonStr := string(data)
	for _, field := range []string{"setup_succeeded", "teardown_succeeded", "exit_code", "failure_reason"} {
		if jsonContainsField(jsonStr, field) {
			t.Errorf("expected field %q to be omitted when unset, got json: %s", field, jsonStr)
		}
	}
}

func jsonContainsField(jsonStr, field string) bool {
	var m map[string]interface{}
	if err := json.Unmarshal([]byte(jsonStr), &m); err != nil {
		return false
	}
	_, ok := m[field]
	return ok
}
