// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"bytes"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"
)

type alwaysTrueCheck struct {
	baseCheck
	outputsDir string
}

func (c alwaysTrueCheck) Check(*TestingOutputs) bool {
	return true
}

func (c alwaysTrueCheck) Name() string {
	return "always_true"
}

func (c alwaysTrueCheck) DebugText() string {
	return "True dat"
}

func (c alwaysTrueCheck) OutputFiles() []string {
	return []string{filepath.Join(c.outputsDir, "true.txt")}
}

type alwaysFalseCheck struct{ baseCheck }

func (c alwaysFalseCheck) Check(*TestingOutputs) bool {
	return false
}

func (c alwaysFalseCheck) Name() string {
	return "always_false"
}

func (c alwaysFalseCheck) DebugText() string {
	return "Lies!"
}

func (c alwaysFalseCheck) OutputFiles() []string {
	return []string{}
}

type alwaysPanicCheck struct{ baseCheck }

func (c alwaysPanicCheck) Check(*TestingOutputs) bool {
	panic("oh dear")
}

func (c alwaysPanicCheck) Name() string {
	return "always_panic"
}

func (c alwaysPanicCheck) DebugText() string {
	return ""
}

func (c alwaysPanicCheck) OutputFiles() []string {
	return []string{}
}

type alwaysFlakeCheck struct{ baseCheck }

func (c alwaysFlakeCheck) Check(*TestingOutputs) bool {
	return true
}

func (c alwaysFlakeCheck) Name() string {
	return "always_flake"
}

func (c alwaysFlakeCheck) IsFlake() bool {
	return true
}

type infraFailureCheck struct {
	baseCheck
}

func (c infraFailureCheck) Check(*TestingOutputs) bool {
	return true
}

func (c infraFailureCheck) Name() string {
	return "always_infra"
}

func (c infraFailureCheck) DebugText() string {
	return "Infra failure"
}

func (c infraFailureCheck) IsInfraFailure() bool {
	return true
}

func TestRunChecks(t *testing.T) {
	falseCheck := alwaysFalseCheck{}
	outputsDir := t.TempDir()
	trueCheck := alwaysTrueCheck{outputsDir: outputsDir}
	panicCheck := alwaysPanicCheck{}
	flakeCheck := alwaysFlakeCheck{}
	infraFailCheck := infraFailureCheck{}

	tests := []struct {
		name   string
		checks []FailureModeCheck
		want   []runtests.TestDetails
	}{
		{
			name: "mixed_checks",
			checks: []FailureModeCheck{
				flakeCheck, falseCheck, trueCheck, panicCheck,
			},
			want: []runtests.TestDetails{
				{
					Name:                 checkTestNamePrefix + flakeCheck.Name(),
					Status:               runtests.TestFailure,
					IsTestingFailureMode: true,
					TestResult:           runtests.TestResult{OutputFiles: []string{debugPathForCheck(flakeCheck)}},
				},
				{
					Name:                 checkTestNamePrefix + flakeCheck.Name(),
					Status:               runtests.TestSuccess,
					IsTestingFailureMode: true,
				},
				{
					Name:                 checkTestNamePrefix + trueCheck.Name(),
					Status:               runtests.TestFailure,
					IsTestingFailureMode: true,
					TestResult:           runtests.TestResult{OutputFiles: []string{debugPathForCheck(trueCheck)}},
				},
			},
		}, {
			name:   "infra_failure_check",
			checks: []FailureModeCheck{infraFailCheck},
			want: []runtests.TestDetails{
				{
					Name:                 checkTestNamePrefix + infraFailCheck.Name(),
					Status:               runtests.TestInfraFailure,
					IsTestingFailureMode: true,
					TestResult:           runtests.TestResult{OutputFiles: []string{debugPathForCheck(infraFailCheck)}},
				},
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if test.name == "mixed_checks" {
				for _, of := range trueCheck.OutputFiles() {
					test.want[2].OutputFiles = append(test.want[2].OutputFiles, filepath.Base(of))
				}
			}
			startTime := time.Now()

			got, err := RunChecks(test.checks, nil, outputsDir)
			if err != nil {
				t.Error("RunChecks() failed with:", err)
			}
			for i, td := range got {
				if td.StartTime.Sub(startTime) < 0 {
					t.Errorf("start time should be later than %v, got %v", startTime, td.StartTime)
				}
				// Since the start time and duration are based on the current time, we should
				// set those values to the default values so that we don't check them when
				// comparing the actual and expected test details.
				var defaultTime time.Time
				got[i].StartTime = defaultTime
				got[i].DurationMillis = 0
				for _, outputFile := range td.OutputFiles {
					// RunChecks() is only responsible for writing the debug text to a file.
					if outputFile != test.want[i].OutputFiles[0] {
						continue
					}
					if _, err := os.Stat(filepath.Join(outputsDir, outputFile)); err != nil {
						t.Errorf("failed to stat OutputFile %s: %v", outputFile, err)
					}
				}
			}
			if diff := cmp.Diff(test.want, got, cmpopts.EquateEmpty()); diff != "" {
				t.Errorf("RunChecks() returned unexpected tests (-want +got):\n%s", diff)
			}
		})
	}
}

type syntheticCheck struct {
	baseCheck
}

func (c syntheticCheck) Check(*TestingOutputs) bool {
	return true
}

func (c syntheticCheck) Name() string {
	return "synthetic_check"
}

func (c syntheticCheck) EmitSyntheticTestCase() bool {
	return true
}

func (c syntheticCheck) FailureReason() string {
	return "synthetic failure"
}

type targetedCheck struct {
	syntheticCheck
	testName string
}

func (c targetedCheck) TestName() string {
	return c.testName
}

func TestRunChecks_TargetedSyntheticTestCase(t *testing.T) {
	summary := runtests.TestSummary{
		Tests: []runtests.TestDetails{
			{
				Name:   "passing_test",
				Status: runtests.TestSuccess,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{},
				},
			},
			{
				Name:   "failing_test_1",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{},
				},
			},
			{
				Name:   "failing_test_2",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{},
				},
			},
		},
	}
	to := TestingOutputs{
		TestSummary: &summary,
	}
	outputsDir := t.TempDir()

	// This check should only be attributed to "failing_test_1".
	check := targetedCheck{testName: "failing_test_1"}

	_, err := RunChecks([]FailureModeCheck{check}, &to, outputsDir)
	if err != nil {
		t.Fatalf("RunChecks() failed: %v", err)
	}

	// Passing test should have NO synthetic case.
	if got := len(summary.Tests[0].Cases); got != 0 {
		t.Errorf("summary.Tests[0].Cases length = %d, want 0", got)
	}

	// failing_test_1 should have ONE synthetic case (Targeted attribution).
	if got := len(summary.Tests[1].Cases); got != 1 {
		t.Errorf("summary.Tests[1].Cases length = %d, want 1", got)
	}

	// failing_test_2 should have ZERO synthetic cases.
	if got := len(summary.Tests[2].Cases); got != 0 {
		t.Errorf("summary.Tests[2].Cases length = %d, want 0", got)
	}
}

func TestRunChecks_GlobalSyntheticTestCase(t *testing.T) {
	summary := runtests.TestSummary{
		Tests: []runtests.TestDetails{
			{
				Name:   "passing_test",
				Status: runtests.TestSuccess,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{},
				},
			},
			{
				Name:   "failing_test_1",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{},
				},
			},
			{
				Name:   "failing_test_2",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{
						{
							CaseName:      "failing_test_2_test-case-name",
							Status:        runtests.TestFailure,
							FailureReason: runtests.FailureReasonFromMessage("failing_test_2_test-case-failure-reason"),
						},
					},
				},
			},
		},
	}
	to := TestingOutputs{
		TestSummary: &summary,
	}
	outputsDir := t.TempDir()
	check := syntheticCheck{}

	_, err := RunChecks([]FailureModeCheck{check}, &to, outputsDir)
	if err != nil {
		t.Fatalf("RunChecks() failed: %v", err)
	}

	// Passing test should have NO synthetic case.
	if got := len(summary.Tests[0].Cases); got != 0 {
		t.Errorf("summary.Tests[0].Cases length = %d, want 0", got)
	}

	// Failing tests with no test cases should have exactly ONE synthetic case.
	if got := len(summary.Tests[1].Cases); got != 1 {
		t.Errorf("summary.Tests[1].Cases length = %d, want 1", got)
	} else {
		tc := summary.Tests[1].Cases[0]
		if tc.SuiteName != "tefmocheck" {
			t.Errorf("TestCase.SuiteName = %q, want %q", tc.SuiteName, "tefmocheck")
		}
		if tc.CaseName != check.Name() {
			t.Errorf("TestCase.CaseName = %q, want %q", tc.CaseName, check.Name())
		}
		if tc.FailureReason == nil || len(tc.FailureReason.Errors) == 0 || tc.FailureReason.Errors[0].Message != check.FailureReason() {
			t.Errorf("TestCase.FailureReason = %v, want error message %q", tc.FailureReason, check.FailureReason())
		}
	}

	// Failing tests with test cases should have exactly ONE synthetic case (one more test case than before).
	if got := len(summary.Tests[2].Cases); got != 2 {
		t.Errorf("summary.Tests[2].Cases length = %d, want 2", got)
	} else {
		tc := summary.Tests[2].Cases[1] // The last test case is the synthetic one.
		if tc.SuiteName != "tefmocheck" {
			t.Errorf("TestCase.SuiteName = %q, want %q", tc.SuiteName, "tefmocheck")
		}
		if tc.CaseName != check.Name() {
			t.Errorf("TestCase.CaseName = %q, want %q", tc.CaseName, check.Name())
		}
		if tc.FailureReason == nil || len(tc.FailureReason.Errors) == 0 || tc.FailureReason.Errors[0].Message != check.FailureReason() {
			t.Errorf("TestCase.FailureReason = %v, want error message %q", tc.FailureReason, check.FailureReason())
		}
	}
}

func TestRunChecks_TargetedSyntheticTestCase_NotFound(t *testing.T) {
	summary := runtests.TestSummary{
		Tests: []runtests.TestDetails{
			{
				Name:   "failing_test_1",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{},
				},
			},
			{
				Name:   "passing_test",
				Status: runtests.TestSuccess,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{},
				},
			},
		},
	}
	to := TestingOutputs{
		TestSummary: &summary,
	}
	outputsDir := t.TempDir()

	t.Run("not_in_summary", func(t *testing.T) {
		check := targetedCheck{testName: "non_existent_test"}
		var buf bytes.Buffer
		log.SetOutput(&buf)
		defer log.SetOutput(os.Stderr)

		_, err := RunChecks([]FailureModeCheck{check}, &to, outputsDir)
		if err != nil {
			t.Fatalf("RunChecks() failed: %v", err)
		}
		wantLog := "Warning: targeted check synthetic_check attributed to test \"non_existent_test\" but test not found in summary"
		if !strings.Contains(buf.String(), wantLog) {
			t.Errorf("Log output %q does not contain expected warning %q", buf.String(), wantLog)
		}
	})

	t.Run("passing_test_in_summary", func(t *testing.T) {
		check := targetedCheck{testName: "passing_test"}
		var buf bytes.Buffer
		log.SetOutput(&buf)
		defer log.SetOutput(os.Stderr)

		_, err := RunChecks([]FailureModeCheck{check}, &to, outputsDir)
		if err != nil {
			t.Fatalf("RunChecks() failed: %v", err)
		}
		wantLog := "Warning: targeted check synthetic_check attributed to test \"passing_test\" but test not found in summary"
		if !strings.Contains(buf.String(), wantLog) {
			t.Errorf("Log output %q does not contain expected warning %q", buf.String(), wantLog)
		}
	})
}

type mockCheck struct {
	baseCheck
	name                  string
	testName              string
	failureReason         string
	emitSyntheticTestCase bool
}

func (c mockCheck) Name() string {
	if c.name != "" {
		return c.name
	}
	return "test_check"
}

func (c mockCheck) TestName() string            { return c.testName }
func (c mockCheck) FailureReason() string       { return c.failureReason }
func (c mockCheck) EmitSyntheticTestCase() bool { return c.emitSyntheticTestCase }

func TestRunChecks_TargetedEnrichment(t *testing.T) {
	outputsDir := t.TempDir()

	summary := runtests.TestSummary{
		Tests: []runtests.TestDetails{
			{
				Name:   "targeted_failing_test",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{
						{CaseName: "failed_case_1", Status: runtests.TestFailure},
						{CaseName: "failed_case_2", Status: runtests.TestFailure},
						{CaseName: "passed_case", Status: runtests.TestSuccess},
					},
				},
			},
			{
				Name:       "sibling_failing_test",
				Status:     runtests.TestFailure,
				TestResult: runtests.TestResult{Cases: []runtests.TestCaseResult{{CaseName: "sibling_failed_case", Status: runtests.TestFailure}}},
			},
			{
				Name:       "passing_test",
				Status:     runtests.TestSuccess,
				TestResult: runtests.TestResult{Cases: []runtests.TestCaseResult{{CaseName: "sibling_passed_case", Status: runtests.TestSuccess}}},
			},
		},
	}
	check := mockCheck{
		name:          "targeted_check",
		testName:      "targeted_failing_test",
		failureReason: "fake failure reason",
	}
	wantErrMsg := check.FailureReason()

	if _, err := RunChecks([]FailureModeCheck{check}, &TestingOutputs{TestSummary: &summary}, outputsDir); err != nil {
		t.Fatalf("RunChecks() failed: %v", err)
	}

	// Verify targeted top-level test gets a FailureReason.
	target := &summary.Tests[0]
	if len(target.Cases) != 3 {
		t.Errorf("len(target.Cases) = %d, want 3", len(target.Cases))
	}
	if target.FailureReason == nil || len(target.FailureReason.Errors) != 1 || target.FailureReason.Errors[0].Message != wantErrMsg {
		t.Fatalf("target.FailureReason errors count = %v, want 1 error %q", target.FailureReason, wantErrMsg)
	}

	// Verify targeted test's failing test cases get a FailureReason.
	for caseIdx, failedCase := range []*runtests.TestCaseResult{&target.Cases[0], &target.Cases[1]} {
		if failedCase.FailureReason == nil || len(failedCase.FailureReason.Errors) != 1 || failedCase.FailureReason.Errors[0].Message != wantErrMsg {
			t.Fatalf("target.Cases[%d].FailureReason = %v, want 1 error %q", caseIdx, failedCase.FailureReason, wantErrMsg)
		}
	}

	// Passing tests/test cases, as well as non-targeted tests, should not get a FailureReason.
	if target.Cases[2].FailureReason != nil {
		t.Errorf("passing case FailureReason = %+v, want nil", target.Cases[2].FailureReason)
	}
	if summary.Tests[1].FailureReason != nil || summary.Tests[1].Cases[0].FailureReason != nil {
		t.Errorf("sibling failing test modified unexpectedly")
	}
	if summary.Tests[2].FailureReason != nil || summary.Tests[2].Cases[0].FailureReason != nil {
		t.Errorf("sibling passing test modified unexpectedly")
	}

	// Test warning when the targeted test is not found or passing (doesn't have a failure status).
	t.Run("not_found_or_passing", func(t *testing.T) {
		tests := []struct {
			name     string
			testName string
		}{
			{name: "not_in_summary", testName: "non_existent_test"},
			{name: "passing_test_in_summary", testName: "passing_test"},
		}

		for _, tc := range tests {
			t.Run(tc.name, func(t *testing.T) {
				summary := runtests.TestSummary{Tests: []runtests.TestDetails{
					{Name: "failing_test", Status: runtests.TestFailure},
					{Name: "passing_test", Status: runtests.TestSuccess},
				}}
				to := TestingOutputs{TestSummary: &summary}
				check := mockCheck{name: "warning_check", testName: tc.testName}
				var buf bytes.Buffer
				log.SetOutput(&buf)
				defer log.SetOutput(os.Stderr)

				if _, err := RunChecks([]FailureModeCheck{check}, &to, outputsDir); err != nil {
					t.Fatalf("RunChecks() failed: %v", err)
				}
				wantLog := fmt.Sprintf("Warning: targeted check %s attributed to test %q but test not found in summary", check.Name(), tc.testName)
				if got := strings.Count(buf.String(), wantLog); got != 1 {
					t.Errorf("Log output contained %d occurrences of %q, want 1. Full log: %q", got, wantLog, buf.String())
				}

				// No test should receive a FailureReason.
				if summary.Tests[0].FailureReason != nil || summary.Tests[1].FailureReason != nil {
					t.Errorf("expected no FailureReason to be enriched when target was not found or passing")
				}
			})
		}
	})
}

func TestRunChecks_GlobalEnrichment(t *testing.T) {
	outputsDir := t.TempDir()

	preexistingErr := &runtests.FailureReasonError{Message: "pre-existing error"}
	summary := runtests.TestSummary{
		Tests: []runtests.TestDetails{
			{
				Name:   "failing_test_mixed_cases",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{
						{CaseName: "failed_case", Status: runtests.TestFailure},
						{CaseName: "exonerated_case", Status: runtests.TestExonerated},
						{CaseName: "passed_case", Status: runtests.TestSuccess},
						{CaseName: "skipped_case", Status: runtests.TestSkipped},
					},
				},
			},
			{
				Name:   "failing_test_with_preexisting_error",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{
						{
							CaseName: "failed_case",
							Status:   runtests.TestFailure,
							FailureReason: &runtests.FailureReason{
								Errors: []*runtests.FailureReasonError{preexistingErr},
							},
						},
					},
					FailureReason: &runtests.FailureReason{Errors: []*runtests.FailureReasonError{preexistingErr}},
				},
			},
			{
				Name:       "passing_test",
				Status:     runtests.TestSuccess,
				TestResult: runtests.TestResult{Cases: []runtests.TestCaseResult{{CaseName: "passed_case", Status: runtests.TestSuccess}}},
			},
		},
	}

	check := mockCheck{name: "global_check", failureReason: "global failure reason"}
	wantErrMsg := check.FailureReason()

	if _, err := RunChecks([]FailureModeCheck{check}, &TestingOutputs{TestSummary: &summary}, outputsDir); err != nil {
		t.Fatalf("RunChecks() failed: %v", err)
	}

	// 1. failing_test_mixed_cases: top-level test and both failing cases enriched.
	t0 := &summary.Tests[0]
	if len(t0.Cases) != 4 {
		t.Errorf("len(t0.Cases) = %d, want 4", len(t0.Cases))
	}
	if t0.FailureReason == nil || len(t0.FailureReason.Errors) != 1 || t0.FailureReason.Errors[0].Message != wantErrMsg {
		t.Errorf("t0.FailureReason = %+v, want 1 error %q", t0.FailureReason, wantErrMsg)
	}
	for i, caseIdx := range []int{0, 1} {
		tc := &t0.Cases[caseIdx]
		if tc.FailureReason == nil || len(tc.FailureReason.Errors) != 1 || tc.FailureReason.Errors[0].Message != wantErrMsg {
			t.Errorf("t0.Cases[%d].FailureReason = %+v, want 1 error %q", i, tc.FailureReason, wantErrMsg)
		}
	}
	// Passed_case and skipped_case are not enriched.
	if t0.Cases[2].FailureReason != nil || t0.Cases[3].FailureReason != nil {
		t.Errorf("t0 non-failing cases enriched unexpectedly")
	}

	// 2. failing_test_with_preexisting_error: top-level test and case preserve pre-existing error at index 0 and append check error.
	t1 := &summary.Tests[1]
	if len(t1.Cases) != 1 {
		t.Errorf("len(t1.Cases) = %d, want 1", len(t1.Cases))
	}
	if t1.FailureReason == nil || len(t1.FailureReason.Errors) != 2 || t1.FailureReason.Errors[0] != preexistingErr || t1.FailureReason.Errors[1].Message != wantErrMsg {
		t.Errorf("t1.FailureReason = %+v, want pre-existing error preserved at [0] and %q at [1]", t1.FailureReason, wantErrMsg)
	}
	if tc := &t1.Cases[0]; tc.FailureReason == nil || len(tc.FailureReason.Errors) != 2 || tc.FailureReason.Errors[0] != preexistingErr || tc.FailureReason.Errors[1].Message != wantErrMsg {
		t.Errorf("t1.Cases[0].FailureReason = %+v, want pre-existing error preserved at [0] and %q at [1]", tc.FailureReason, wantErrMsg)
	}

	// 3. passing_test remains untouched.
	t2 := &summary.Tests[2]
	if t2.FailureReason != nil || t2.Cases[0].FailureReason != nil {
		t.Errorf("summary.Tests[2] passing test enriched unexpectedly")
	}
	if len(t2.Cases) != 1 {
		t.Errorf("len(summary.Tests[2].Cases) = %d, want 1", len(t2.Cases))
	}
}

type exonerationCheck struct {
	syntheticCheck
	testName string
}

func (c exonerationCheck) Name() string {
	return "exoneration_check"
}

func (c exonerationCheck) IsExoneration() bool {
	return true
}

func (c exonerationCheck) TestName() string {
	return c.testName
}

func TestRunChecks_Exoneration(t *testing.T) {
	summary := runtests.TestSummary{
		Tests: []runtests.TestDetails{
			{
				Name:   "failing_test",
				Status: runtests.TestFailure,
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{
						{
							CaseName: "failed_case",
							Status:   runtests.TestFailure,
						},
						{
							CaseName: "passed_case",
							Status:   runtests.TestSuccess,
						},
					},
				},
			},
		},
	}
	to := TestingOutputs{
		TestSummary: &summary,
	}
	outputsDir := t.TempDir()
	check := exonerationCheck{testName: "failing_test"}

	got, err := RunChecks([]FailureModeCheck{check}, &to, outputsDir)
	if err != nil {
		t.Fatalf("RunChecks() failed: %v", err)
	}

	// No synthetic tests should be returned for exoneration.
	if len(got) != 0 {
		t.Fatalf("RunChecks() returned %d tests, want 0", len(got))
	}

	// The actual test status should be updated to TestExonerated.
	if summary.Tests[0].Status != runtests.TestExonerated {
		t.Errorf("summary.Tests[0].Status = %v, want %v", summary.Tests[0].Status, runtests.TestExonerated)
	}

	// The failed test case status should be updated to TestExonerated.
	if summary.Tests[0].Cases[0].Status != runtests.TestExonerated {
		t.Errorf("summary.Tests[0].Cases[0].Status = %v, want %v", summary.Tests[0].Cases[0].Status, runtests.TestExonerated)
	}

	// The passed test case status should remain TestSuccess.
	if summary.Tests[0].Cases[1].Status != runtests.TestSuccess {
		t.Errorf("summary.Tests[0].Cases[1].Status = %v, want %v", summary.Tests[0].Cases[1].Status, runtests.TestSuccess)
	}

	// No synthetic test case should be added.
	if len(summary.Tests[0].Cases) != 2 {
		t.Fatalf("len(summary.Tests[0].Cases) = %d, want 2", len(summary.Tests[0].Cases))
	}
}
