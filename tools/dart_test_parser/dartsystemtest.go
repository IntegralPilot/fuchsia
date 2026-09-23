// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package dart_test_parser

import (
	"bytes"
	"encoding/json"
	"fmt"
	"regexp"
	"time"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

var (
	dartSystemTestPreamblePattern = regexp.MustCompile(`^\[----------\] Test results JSON:$`)
)

type dartSystemTestResults struct {
	TestGroups []TestGroup
}

type TestGroup struct {
	Name      string
	TestCases []TestCase
}

type TestCase struct {
	Name              string
	Result            string
	DurationInSeconds int
}

// Parse takes stdout from a dart test program and returns structured results.
// If no structured results were identified, an empty slice is returned.
func Parse(stdout []byte) []runtests.TestCaseResult {
	lines := bytes.Split(stdout, []byte{'\n'})
	cases := parseDartSystemTest(lines)

	// Ensure that an empty set of cases is serialized to JSON as an empty
	// array, not as null.
	if cases == nil {
		cases = []runtests.TestCaseResult{}
	}
	return cases
}

func parseDartSystemTest(lines [][]byte) []runtests.TestCaseResult {
	var jsonBytes []byte
	foundTestResultsStart := false
	for _, line := range lines {
		if !foundTestResultsStart {
			if dartSystemTestPreamblePattern.Match(line) {
				foundTestResultsStart = true
			}
			continue
		}
		jsonBytes = append(jsonBytes, line...)
	}
	parsed := dartSystemTestResults{}
	if err := json.Unmarshal(jsonBytes, &parsed); err != nil {
		return []runtests.TestCaseResult{}
	}

	var res []runtests.TestCaseResult
	for _, testGroup := range parsed.TestGroups {
		for _, testCase := range testGroup.TestCases {
			var status runtests.TestStatus
			switch testCase.Result {
			case "PASSED":
				status = runtests.TestSuccess
			case "FAILED":
				status = runtests.TestFailure
			}
			res = append(res, runtests.TestCaseResult{
				DisplayName: fmt.Sprintf("%s.%s", testGroup.Name, testCase.Name),
				SuiteName:   testGroup.Name,
				CaseName:    testCase.Name,
				Status:      status,
				Duration:    time.Duration(testCase.DurationInSeconds) * time.Second,
				Format:      "dart_system_test",
			})
		}
	}
	return res
}
