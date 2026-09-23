// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package go_test_parser

import (
	"bytes"
	"fmt"
	"regexp"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

var (
	goTestPreamblePattern = regexp.MustCompile(`^=== RUN\s+(.[^/]*)(?:/?)(.*?)$`)
	goTestCasePattern     = regexp.MustCompile(`^\s*--- (\w*?): (.[^/]*)(?:/?)(.*?) \((.*?)\)$`)
	goTestPanicPattern    = regexp.MustCompile(`^panic: test timed out after (\S+)$`)
)

// Parse takes stdout from a go test program and returns structured results.
// If no structured results were identified, an empty slice is returned.
func Parse(stdout []byte) []runtests.TestCaseResult {
	lines := bytes.Split(stdout, []byte{'\n'})
	cases := parseGoTest(lines)

	// Ensure that an empty set of cases is serialized to JSON as an empty
	// array, not as null.
	if cases == nil {
		cases = []runtests.TestCaseResult{}
	}
	return cases
}

func parseGoTest(lines [][]byte) []runtests.TestCaseResult {
	var res []runtests.TestCaseResult
	var preambleName string
	var stdoutForCase []string
	for _, line := range lines {
		var matched bool
		line := string(line)
		m := goTestPreamblePattern.FindStringSubmatch(line)
		if m != nil {
			preambleName = m[1]
			stdoutForCase = []string{}
			continue
		}
		var status runtests.TestStatus
		var displayName string
		var suiteName string
		var caseName string
		var duration time.Duration
		m = goTestPanicPattern.FindStringSubmatch(line)
		if m != nil {
			status = runtests.TestFailure
			caseName = preambleName
			displayName = preambleName
			duration, _ = time.ParseDuration(m[1])
			matched = true
			stdoutForCase = []string{line}
		}
		m = goTestCasePattern.FindStringSubmatch(line)
		if m != nil {
			switch m[1] {
			case "PASS":
				status = runtests.TestSuccess
			case "FAIL":
				status = runtests.TestFailure
			case "SKIP":
				status = runtests.TestSkipped
			}
			if m[3] == "" {
				caseName = m[2]
				displayName = caseName
			} else {
				suiteName = m[2]
				caseName = m[3]
				displayName = fmt.Sprintf("%s/%s", suiteName, caseName)
			}
			duration, _ = time.ParseDuration(m[4])
			matched = true
		}
		if matched {
			caseResult := runtests.TestCaseResult{
				DisplayName: displayName,
				SuiteName:   suiteName,
				CaseName:    caseName,
				Status:      status,
				Duration:    duration,
				Format:      "Go",
			}
			if runtests.IsFailure(caseResult.Status) {
				caseResult.FailureReason = runtests.FailureReasonFromMessage(
					strings.TrimSpace(strings.Join(stdoutForCase, "\n")))
			}
			res = append(res, caseResult)
			stdoutForCase = []string{}
			continue
		}
		stdoutForCase = append(stdoutForCase, line)
	}
	return res
}
