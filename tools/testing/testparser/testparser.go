// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Package testparser parses test stdout into structured results.
package testparser

import (
	"bytes"
	"fmt"
	"regexp"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/parseoutput"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

var (
	// LINT.IfChange(mobly_test_start)
	moblyTestPreamblePatternStr = `^======== Mobly config content ========$`
	// LINT.ThenChange(//src/testing/end_to_end/mobly_driver/mobly_driver/api/api_infra.py:mobly_test_start)
	moblyTestPreamblePattern      = regexp.MustCompile(moblyTestPreamblePatternStr)
	ctsTestPreamblePattern        = regexp.MustCompile(`^dEQP Core .* starting\.\.$`)
	dartSystemTestPreamblePattern = regexp.MustCompile(`^\[----------\] Test results JSON:$`)
	trfTestPreamblePattern        = regexp.MustCompile(`^Running test 'fuchsia-pkg:\/\/.*$`)
	googleTestPreamblePattern     = regexp.MustCompile(`^\[==========\] Running \d* tests? from \d* test (?:(?:suites?)|(?:cases?))\.$`)
	zirconUtestPreamblePattern    = regexp.MustCompile(`^CASE\s*(.*?)\s*\[STARTED\]$`)
)

// Parse takes stdout from a test program and returns structured results.
// Internally, a variety of test program stdout formats are supported.
// If no structured results were identified, an empty slice is returned.
func Parse(stdout []byte) ([]runtests.TestCaseResult, error) {
	lines := bytes.Split(stdout, []byte{'\n'})
	res := []*regexp.Regexp{
		moblyTestPreamblePattern,
		ctsTestPreamblePattern,
		dartSystemTestPreamblePattern,
		trfTestPreamblePattern,
		googleTestPreamblePattern,
		zirconUtestPreamblePattern,
		parseoutput.TestPreamblePattern,
	}
	remainingLines, match := firstMatch(lines, res)

	// TODO(https://fxbug.dev/435723394): Replace each case with
	// `return nil, fmt.Errorf("this test did not parse its own test cases")`
	// when removing support for the test type.
	var cases []runtests.TestCaseResult
	switch match {
	case moblyTestPreamblePattern:
		return nil, fmt.Errorf("this test did not parse its own test cases")
	case ctsTestPreamblePattern:
		cases = parseVulkanCtsTest(remainingLines)
	case dartSystemTestPreamblePattern:
		return nil, fmt.Errorf("this test did not parse its own test cases")
	case trfTestPreamblePattern:
		cases = parseTrfTest(lines)
	case googleTestPreamblePattern:
		cases = parseGoogleTest(remainingLines)
	case zirconUtestPreamblePattern:
		cases = parseZirconUtest(remainingLines)
	case parseoutput.TestPreamblePattern:
		return nil, fmt.Errorf("this test did not parse its own test cases")
	}

	// Ensure that an empty set of cases is serialized to JSON as an empty
	// array, not as null.
	if cases == nil {
		cases = []runtests.TestCaseResult{}
	}
	return cases, nil
}

func firstMatch(
	lines [][]byte,
	res []*regexp.Regexp,
) ([][]byte, *regexp.Regexp) {
	for num, line := range lines {
		for _, re := range res {
			if re.Match(line) {
				return lines[num:], re
			}
		}
	}
	return nil, nil
}
