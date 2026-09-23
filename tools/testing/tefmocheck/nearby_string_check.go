// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"path"
	"slices"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

// NearbyStringCheck checks for two strings that appear close to each other in a log.
type NearbyStringCheck struct {
	baseCheck
	// String1 to search for.
	String1 string
	// String2 to search for.
	String2 string
	// MaxDistanceLines is the maximum number of lines apart the two strings can be.
	MaxDistanceLines int
	// SkipPassedTask will cause Check() to return false if the
	// Swarming task succeeded.
	SkipPassedTask bool
	// SkipAllPassedTests will cause Check() to return false if all tests
	// in the Swarming task passed.
	SkipAllPassedTests bool
	// SkipPassedTest will cause Check() to return true only if it finds the
	// log in the per-test swarming output of a failed test.
	SkipPassedTest bool
	// IgnoreFlakes will cause Check() to behave in the following ways when
	// combined with other options:
	//   SkipAllPassedTests: Check() will ignore flakes when determining if all
	//     tests passed.
	//   SkipPassedTest: Check() will return the failure as a flake if the
	//     associated test later passes.
	IgnoreFlakes bool
	// AlwaysFlake will always return the failure as a flake so that it doesn't
	// fail the build, but will still be reported as a flake.
	AlwaysFlake bool
	// Type of log to search in.
	Type logType
	// InfraFailure is true if the check is related to infra.
	InfraFailure bool
	line1        string
	line2        string
	isFlake      bool
}

func (c *NearbyStringCheck) findNearbyStrings(log []byte) bool {
	lines := strings.Split(string(log), "\n")
	var string1Lines []int
	var string2Lines []int
	for i, line := range lines {
		if strings.Contains(line, c.String1) {
			string1Lines = append(string1Lines, i)
		}
		if strings.Contains(line, c.String2) {
			string2Lines = append(string2Lines, i)
		}
	}

	for _, line1 := range string1Lines {
		for _, line2 := range string2Lines {
			distance := line1 - line2
			if distance < 0 {
				distance = -distance
			}
			if distance <= c.MaxDistanceLines {
				c.line1 = lines[line1]
				c.line2 = lines[line2]
				return true
			}
		}
	}
	return false
}

func (c *NearbyStringCheck) Check(outputs *TestingOutputs) bool {
	swarmmingResult := outputs.SwarmingSummary.Results
	if !swarmmingResult.Failure && swarmmingResult.State == "COMPLETED" {
		if c.SkipPassedTask {
			return false
		}
		if c.SkipAllPassedTests {
			failedTests := make(map[string]struct{})
			for _, test := range outputs.TestSummary.Tests {
				if test.Status != runtests.TestSuccess {
					failedTests[test.Name] = struct{}{}
				} else if c.IgnoreFlakes {
					// If a later run of a failed test passed,
					// remove it from the list of failed tests.
					if _, ok := failedTests[test.Name]; ok {
						delete(failedTests, test.Name)
					}
				}
			}
			if len(failedTests) == 0 {
				return false
			}
		}
	}

	if c.Type == swarmingOutputType && c.SkipPassedTest {
		type testdata struct {
			name    string
			isFlake bool
			index   int
		}
		failedTestsMap := make(map[string]testdata)
		for i, testLog := range outputs.SwarmingOutputPerTest {
			var testStatus runtests.TestStatus
			if outputs.TestSummary != nil && i < len(outputs.TestSummary.Tests) {
				testStatus = outputs.TestSummary.Tests[i].Status
			}
			if testStatus == runtests.TestSuccess {
				if c.IgnoreFlakes {
					if test, ok := failedTestsMap[testLog.TestName]; ok {
						test.isFlake = true
						failedTestsMap[testLog.TestName] = test
					}
				}
				continue
			}
			if c.findNearbyStrings(testLog.Bytes) {
				failedTestsMap[testLog.TestName] = testdata{testLog.TestName, c.AlwaysFlake, i}
			}
		}
		var failedTests []testdata
		var flakedTests []testdata
		for _, data := range failedTestsMap {
			if data.isFlake {
				flakedTests = append(flakedTests, data)
			} else {
				failedTests = append(failedTests, data)
			}
		}
		if len(failedTests) > 0 {
			return true
		}
		if len(flakedTests) > 0 {
			c.isFlake = true
			return true
		}
		return false
	}

	var logs [][]byte
	switch c.Type {
	case serialLogType:
		logs = outputs.SerialLogs
	case swarmingOutputType:
		logs = [][]byte{outputs.SwarmingOutput}
	case syslogType:
		logs = outputs.Syslogs
	default:
		return false
	}

	if slices.ContainsFunc(logs, c.findNearbyStrings) {
		if c.AlwaysFlake {
			c.isFlake = true
		}
		return true
	}
	return false
}

func (c *NearbyStringCheck) Name() string {
	return path.Join("nearby_string",
		string(c.Type),
		strings.ReplaceAll(c.String1, " ", "_"),
		strings.ReplaceAll(c.String2, " ", "_"))
}

func (c *NearbyStringCheck) IsInfraFailure() bool {
	return c.InfraFailure
}

func (c *NearbyStringCheck) IsFlake() bool {
	return c.isFlake
}

func (c *NearbyStringCheck) FailureReason() string {
	return "Found lines nearby:\n" + c.line1 + "\n" + c.line2
}

func NearbyStringsChecks() []FailureModeCheck {
	return []FailureModeCheck{
		// For https://fxbug.dev/486231198
		// LINT.IfChange(suspend_failed_tefmo)
		&NearbyStringCheck{
			String1:          "Suspend failed due to \"external wake source: nanohub-datachannel\".",
			String2:          "Format: Log Type - Time(microsec) - Message - Optional Info",
			MaxDistanceLines: 10,
			Type:             serialLogType,
		},
		// LINT.ThenChange(//src/starnix/kernel/core/power/manager.rs:suspend_failed_tefmo)
	}
}
