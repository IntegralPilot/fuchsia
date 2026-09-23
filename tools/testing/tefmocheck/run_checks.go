// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"fmt"
	"log"
	"os"
	"path/filepath"
	"time"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

const checkTestNamePrefix = "testing_failure_mode/"

func debugPathForCheck(check FailureModeCheck) string {
	return filepath.Join(checkTestNamePrefix, check.Name(), "debug.txt")
}

// RunChecks runs the given checks on the given TestingOutputs.
// A failed test means the Check() returned true. After the first failed test, all
// later Checks() will be skipped. Tests will not be returned for skipped or passed checks.
// Rationale: In order for these bugs to be useful, we want a failure to be associated only with the most
// specific, helpful failure modes, which then get routed to specific bugs. Hence only a single failure
// is returned.
// Rationale for not returning passed tests:
// We want to be able to add many checks without cluttering the output test summary
// with noise. Our flake detection system will identify a test that appears a a failure on
// one run of a swarming task, and then disappears on other runs of that same task as a flake.
func RunChecks(checks []FailureModeCheck, to *TestingOutputs, outputsDir string) ([]runtests.TestDetails, error) {
	var checkTests []runtests.TestDetails
	for _, check := range checks {
		if failed := check.Check(to); !failed {
			continue
		}

		if check.IsExoneration() {
			if to == nil || to.TestSummary == nil {
				log.Printf("Warning: exoneration check %s matched but TestingOutputs or TestSummary is nil; ignoring", check.Name())
				continue
			}
			attributedTestName := check.TestName()
			if attributedTestName == "" {
				log.Printf("Warning: exoneration check %s matched but is not attributed to a specific test; ignoring", check.Name())
				continue
			}
			foundMatch := false
			for i := range to.TestSummary.Tests {
				test := &to.TestSummary.Tests[i]
				if !runtests.IsFailure(test.Status) {
					continue
				}
				if test.Name != attributedTestName {
					continue
				}
				foundMatch = true
				test.Status = runtests.TestExonerated
				for j := range test.Cases {
					if runtests.IsFailure(test.Cases[j].Status) {
						test.Cases[j].Status = runtests.TestExonerated
					}
				}
			}
			if !foundMatch {
				log.Printf("Warning: exoneration check %s attributed to test %q but test not found in summary", check.Name(), attributedTestName)
			}
			// For exoneration, we don't emit synthetic test or test case.
			// Just update the status and continue.
			continue
		}

		// When a tefmocheck fires, make a note of it in any matching failing tests by adding it to the
		// FailureReason.Errors list at both the top-level-test level and the test-case level.
		// If the check is attributed to a specific test, then only that test "matches". If it's not
		// attributed to a specific test, all failures "match".
		// Block 1: Failure reason enrichment where ever applicable.
		if to != nil && to.TestSummary != nil {
			attributedTestName := check.TestName()
			foundMatch := false
			errMsg := check.FailureReason()
			for i := range to.TestSummary.Tests {
				test := &to.TestSummary.Tests[i]
				if runtests.IsFailure(test.Status) {
					// Global or targeted match
					if attributedTestName == "" || test.Name == attributedTestName {
						if attributedTestName != "" {
							foundMatch = true
						}

						// 1. Dual-write to top-level test.FailureReason.
						if test.FailureReason == nil {
							test.FailureReason = &runtests.FailureReason{}
						}
						test.FailureReason.Errors = append(test.FailureReason.Errors, &runtests.FailureReasonError{
							Message: errMsg,
						})

						// 2. Dual-write to all failing test cases.
						for j := range test.Cases {
							tc := &test.Cases[j]
							if runtests.IsFailure(tc.Status) {
								if tc.FailureReason == nil {
									tc.FailureReason = &runtests.FailureReason{}
								}
								tc.FailureReason.Errors = append(tc.FailureReason.Errors, &runtests.FailureReasonError{
									Message: errMsg,
								})
							}
						}
					}
				}
			}
			if attributedTestName != "" && !foundMatch {
				log.Printf("Warning: targeted check %s attributed to test %q but test not found in summary", check.Name(), attributedTestName)
			}
		}

		// Block 2: Legacy block gated on check.EmitSyntheticTestCase()
		if check.EmitSyntheticTestCase() && to != nil && to.TestSummary != nil {
			attributedTestName := check.TestName()
			foundMatch := false
			for i := range to.TestSummary.Tests {
				test := &to.TestSummary.Tests[i]
				if runtests.IsFailure(test.Status) {
					if attributedTestName == "" || test.Name == attributedTestName {
						if attributedTestName != "" {
							foundMatch = true
						}
						test.Cases = append(test.Cases, runtests.TestCaseResult{
							DisplayName:   "tefmocheck: " + check.Name(),
							SuiteName:     "tefmocheck",
							CaseName:      check.Name(),
							Status:        runtests.TestFailure,
							FailureReason: runtests.FailureReasonFromMessage(check.FailureReason()),
						})
					}
				}
			}
			if attributedTestName != "" && !foundMatch {
				log.Printf("Warning: targeted check %s attributed to test %q but test not found in summary", check.Name(), attributedTestName)
			}
		}

		testDetails := runtests.TestDetails{
			Name:                 checkTestNamePrefix + check.Name(),
			IsTestingFailureMode: true,
			Status:               runtests.TestFailure,
			TestResult: runtests.TestResult{
				// Specify an empty slice so it gets serialized to an empty JSON
				// array instead of null.
				Cases:         []runtests.TestCaseResult{},
				FailureReason: runtests.FailureReasonFromMessage(check.FailureReason()),
			},
			StartTime: time.Now(), // needed by ResultDB
			Tags:      check.Tags(),
		}
		// Check if failure is an infrastructure failure.
		if check.IsInfraFailure() {
			// Infra failures will have status value of "INFRA_FAIL"
			// in summary.json. This will help distinguish regular failures
			// which have status value "FAIL" from infra failures.
			testDetails.Status = runtests.TestInfraFailure
		}

		if len(outputsDir) > 0 {
			outputFile := debugPathForCheck(check)
			testDetails.OutputFiles = []string{outputFile}
			outputFileAbsPath := filepath.Join(outputsDir, outputFile)
			if err := os.MkdirAll(filepath.Dir(outputFileAbsPath), 0o777); err != nil {
				return nil, err
			}
			debugText := fmt.Sprintf(
				"This is a synthetic test that was produced by the tefmocheck tool during post-processing of test results. See https://fuchsia.googlesource.com/fuchsia/+/HEAD/tools/testing/tefmocheck/README.md\n%s",
				check.DebugText())
			if err := os.WriteFile(outputFileAbsPath, []byte(debugText), 0o666); err != nil {
				return nil, err
			}
		}
		for _, cof := range check.OutputFiles() {
			relPath, err := filepath.Rel(outputsDir, cof)
			if err != nil {
				return nil, err
			}
			testDetails.OutputFiles = append(testDetails.OutputFiles, relPath)
		}
		checkTests = append(checkTests, testDetails)
		if check.IsFlake() {
			checkTests = append(checkTests, runtests.TestDetails{
				Name:                 checkTestNamePrefix + check.Name(),
				IsTestingFailureMode: true,
				TestResult: runtests.TestResult{
					// Specify an empty slice so it gets serialized to an empty JSON
					// array instead of null.
					Cases: []runtests.TestCaseResult{},
				},
				Status:    runtests.TestSuccess,
				StartTime: time.Now(), // needed by ResultDB
			})
			// If this check was a flake, continue to see if we get another failure.
			continue
		}
		// We run more specific checks first, so it's not useful to run any checks
		// once we have our first failure.
		break
	}
	return checkTests, nil
}
