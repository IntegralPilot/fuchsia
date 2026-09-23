// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"fmt"
	"testing"

	"github.com/google/go-cmp/cmp"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

func TestCheck(t *testing.T) {
	const killerString = "KILLER STRING"
	const exceptString = "Don't die!"
	const line = "KILLER STRING: Something failed."
	exceptBlock := &logBlock{
		startString: "skip_test",
		endString:   "end_test",
	}
	exceptBlock2 := &logBlock{
		startString: "block_start",
		endString:   "block_end",
	}
	c := stringInLogCheck{
		String:        killerString,
		OnlyOnStates:  []string{},
		ExceptStrings: []string{exceptString},
		ExceptBlocks:  []*logBlock{exceptBlock, exceptBlock2},
		Type:          swarmingOutputType,
	}
	gotName := c.Name()
	wantName := "string_in_log/infra_and_test_std_and_klog.txt/KILLER_STRING"
	if gotName != wantName {
		t.Errorf("c.Name() returned %q, want %q", gotName, wantName)
	}

	for _, tc := range []struct {
		name                string
		attributeToTest     bool
		addTag              bool
		testingOutputs      TestingOutputs
		typeToCheck         logType
		skipPassedTask      bool
		skipAllPassedTests  bool
		skipPassedTest      bool
		ignoreFlakes        bool
		alwaysFlake         bool
		states              []string
		swarmingResultState string
		shouldMatch         bool
		wantName            string
		wantFlake           bool
		wantLine            string
		wantTags            []build.TestTag
	}{
		{
			name: "should match simple",
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil, "PREFIX %s SUFFIX", killerString),
			},
			shouldMatch: true,
		},
		{
			name: "should match with failure reason",
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil, "1\n2\n%s\n4\n5\n", line),
			},
			shouldMatch: true,
			wantLine:    line,
		},
		{
			name: "should match if string before except_block and return failure reason",
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil, "%s\n%s\n%s", exceptBlock.startString, exceptBlock.endString, line),
			},
			shouldMatch: true,
			wantLine:    line,
		},
		{
			name: "exceptSuccessfulSwarmingResult",
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil, "PREFIX %s SUFFIX", killerString),
			},
			skipPassedTask:      true,
			swarmingResultState: "COMPLETED",
		},
		{
			name: "should not match if string in other log",
			testingOutputs: TestingOutputs{
				SerialLogs:     [][]byte{[]byte(killerString)},
				SwarmingOutput: []byte("gentle string"),
			},
		},
		{
			name: "should not match if except_string in log",
			testingOutputs: TestingOutputs{
				SwarmingOutput: []byte(killerString + exceptString),
			},
		},
		{
			name: "should match if string before except_block",
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil, "PREFIX %s ... %s output %s SUFFIX", killerString, exceptBlock.startString, exceptBlock.endString),
			},
			shouldMatch: true,
		},
		{
			name: "should match if string after except_block",
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil, "PREFIX %s output %s ... %s SUFFIX", exceptBlock.startString, exceptBlock.endString, killerString),
			},
			shouldMatch: true,
		},
		{
			name: "should not match if string in except_block",
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil,
					"PREFIX %s %s output %s SUFFIX %s %s %s",
					exceptBlock.startString, killerString, exceptBlock.endString,
					exceptBlock2.startString, killerString, exceptBlock2.endString),
			},
		},
		{
			name: "should match if string in both except_block and outside except_block",
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil,
					"PREFIX %s ... %s %s %s %s %s %s SUFFIX",
					killerString, exceptBlock.startString, killerString, exceptBlock.endString,
					exceptBlock2.startString, killerString, exceptBlock2.endString,
				),
			},
			shouldMatch: true,
		},
		{
			name: "should not match if string in except_block with missing end string",
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil,
					"PREFIX ... %s %s %s %s %s",
					exceptBlock.startString, killerString, exceptBlock.endString,
					exceptBlock2.startString, killerString,
				),
			},
		},
		{
			name: "should match if swarming task state is in expected states",
			testingOutputs: TestingOutputs{
				SwarmingOutput: []byte(killerString),
			},
			states:              []string{"STATE_1", "STATE_2"},
			swarmingResultState: "STATE_1",
			shouldMatch:         true,
		},
		{
			name: "should not match if swarming task state is not in expected states",
			testingOutputs: TestingOutputs{
				SwarmingOutput: []byte(killerString),
			},
			states:              []string{"STATE_1", "STATE_2"},
			swarmingResultState: "NO_STATE",
		},
		{
			name:            "should match per test swarming output",
			attributeToTest: true,
			testingOutputs: TestingOutputs{
				SwarmingOutput: []byte(killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "foo-test",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
				},
			},
			shouldMatch: true,
			wantName:    wantName + "/foo-test",
		},
		{
			name:            "should match per test swarming output with double slash in test name",
			attributeToTest: true,
			testingOutputs: TestingOutputs{
				SwarmingOutput: []byte(killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "@@//foo/bar:baz",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
				},
			},
			shouldMatch: true,
			wantName:    wantName + "/@@//foo/bar:baz",
		},
		{
			name:            "should add test name to tag",
			attributeToTest: true,
			addTag:          true,
			testingOutputs: TestingOutputs{
				SwarmingOutput: []byte(killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "foo-test",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
				},
			},
			shouldMatch: true,
			wantName:    wantName,
			wantTags:    []build.TestTag{{Key: "test_name", Value: "foo-test"}},
		},
		{
			name:            "should respect except block even if split across tests",
			attributeToTest: true,
			testingOutputs: TestingOutputs{
				SwarmingOutput: fmt.Appendf(nil, "%s %s %s %s", exceptBlock.startString, killerString, exceptBlock.endString, killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "foo-test",
						Bytes:    fmt.Appendf(nil, "%s %s", exceptBlock.startString, killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
					{
						TestName: "bar-test",
						Bytes:    fmt.Appendf(nil, "%s %s", exceptBlock.endString, killerString),
						FilePath: "bar/log.txt",
						Index:    len(exceptBlock.startString) + len(killerString) + 2,
					},
				},
			},
			shouldMatch: true,
			wantName:    wantName + "/bar-test",
		},
		{
			name:            "should match correct FailureReason for non-zero SwarmingOutputPerTest slice index",
			attributeToTest: true,
			testingOutputs: TestingOutputs{
				SwarmingOutput: []byte("first test line\nsecond test " + line + "\n"),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "foo-test",
						Bytes:    []byte("first test line\n"),
						FilePath: "foo/log.txt",
						Index:    0,
					},
					{
						TestName: "bar-test",
						Bytes:    []byte("second test " + line + "\n"),
						FilePath: "bar/log.txt",
						Index:    len("first test line\n"),
					},
				},
			},
			shouldMatch: true,
			wantName:    wantName + "/bar-test",
			wantLine:    "second test " + line,
		},
		{
			name:            "should check all syslogs",
			attributeToTest: true,
			testingOutputs: TestingOutputs{
				Syslogs: [][]byte{[]byte("gentle string"), []byte(killerString)},
			},
			shouldMatch: true,
			typeToCheck: syslogType,
			wantName:    "string_in_log/syslog.txt/KILLER_STRING",
		},
		{
			name: "should skip if all tests passed",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestSuccess,
						},
					},
				},
				SerialLogs: [][]byte{[]byte("gentle string"), []byte(killerString)},
			},
			skipAllPassedTests:  true,
			swarmingResultState: "COMPLETED",
			typeToCheck:         serialLogType,
		},
		{
			name: "should skip if ignore flakes",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
						{
							Name:   "test1",
							Status: runtests.TestSuccess,
						},
					},
				},
				SerialLogs: [][]byte{[]byte("gentle string"), []byte(killerString)},
			},
			skipAllPassedTests:  true,
			ignoreFlakes:        true,
			swarmingResultState: "COMPLETED",
			typeToCheck:         serialLogType,
		},
		{
			name: "should not skip if a test failed",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
					},
				},
				SerialLogs: [][]byte{[]byte("gentle string"), []byte(killerString)},
			},
			shouldMatch:         true,
			skipAllPassedTests:  true,
			swarmingResultState: "COMPLETED",
			typeToCheck:         serialLogType,
		},
		{
			name: "should not skip if not ignore flakes",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
						{
							Name:   "test1",
							Status: runtests.TestSuccess,
						},
					},
				},
				SerialLogs: [][]byte{[]byte("gentle string"), []byte(killerString)},
			},
			shouldMatch:         true,
			skipAllPassedTests:  true,
			swarmingResultState: "COMPLETED",
			typeToCheck:         serialLogType,
		},
		{
			name: "should skip if passed test",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
						{
							Name:   "test2",
							Status: runtests.TestSuccess,
						},
					},
				},
				SwarmingOutput: []byte(killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "test1",
						Bytes:    []byte{},
						FilePath: "foo/log.txt",
						Index:    0,
					},
					{
						TestName: "test2",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
				},
			},
			skipPassedTest: true,
		},
		{
			name: "should report flake if ignore flakes",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
						{
							Name:   "test1",
							Status: runtests.TestSuccess,
						},
						{
							Name:   "test2",
							Status: runtests.TestFailure,
						},
					},
				},
				SwarmingOutput: []byte(killerString + killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "test1",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
					{
						TestName: "test1",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    len(killerString),
					},

					{
						TestName: "test2",
						Bytes:    []byte{},
						FilePath: "foo/log.txt",
						Index:    len(killerString) * 2,
					},
				},
			},
			shouldMatch:    true,
			skipPassedTest: true,
			ignoreFlakes:   true,
			wantFlake:      true,
		},
		{
			name: "should report failure over flake",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
						{
							Name:   "test1",
							Status: runtests.TestSuccess,
						},
						{
							Name:   "test2",
							Status: runtests.TestFailure,
						},
					},
				},
				SwarmingOutput: []byte(killerString + killerString + killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "test1",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
					{
						TestName: "test1",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    len(killerString),
					},

					{
						TestName: "test2",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    len(killerString) * 2,
					},
				},
			},
			shouldMatch:    true,
			skipPassedTest: true,
			ignoreFlakes:   true,
		},
		{
			name: "should not skip if not ignore flakes and skipPassedTest",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
						{
							Name:   "test1",
							Status: runtests.TestSuccess,
						},
					},
				},
				SwarmingOutput: []byte(killerString + killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "test1",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
					{
						TestName: "test1",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    len(killerString),
					},
				},
			},
			shouldMatch:    true,
			skipPassedTest: true,
		},
		{
			name: "should skip if skipPassedTask and a test failed",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
					},
				},
				SerialLogs: [][]byte{[]byte("gentle string"), []byte(killerString)},
			},
			skipPassedTask:      true,
			swarmingResultState: "COMPLETED",
			typeToCheck:         serialLogType,
		},
		{
			name: "should always flake if not attributed to test",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
						{
							Name:   "test2",
							Status: runtests.TestSuccess,
						},
					},
				},
				SwarmingOutput: []byte(killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "test1",
						Bytes:    []byte{},
						FilePath: "foo/log.txt",
						Index:    0,
					},
					{
						TestName: "test2",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
				},
			},
			shouldMatch: true,
			alwaysFlake: true,
			wantFlake:   true,
		},
		{
			name: "should always flake if skip passed test and failed test",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestSuccess,
						},
						{
							Name:   "test2",
							Status: runtests.TestFailure,
						},
					},
				},
				SwarmingOutput: []byte(killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "test1",
						Bytes:    []byte{},
						FilePath: "foo/log.txt",
						Index:    0,
					},
					{
						TestName: "test2",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
				},
			},
			shouldMatch:    true,
			skipPassedTest: true,
			alwaysFlake:    true,
			wantFlake:      true,
		},
		{
			name: "should not flake if skip passed test",
			testingOutputs: TestingOutputs{
				TestSummary: &runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{
							Name:   "test1",
							Status: runtests.TestFailure,
						},
						{
							Name:   "test2",
							Status: runtests.TestSuccess,
						},
					},
				},
				SwarmingOutput: []byte(killerString),
				SwarmingOutputPerTest: []TestLog{
					{
						TestName: "test1",
						Bytes:    []byte{},
						FilePath: "foo/log.txt",
						Index:    0,
					},
					{
						TestName: "test2",
						Bytes:    []byte(killerString),
						FilePath: "foo/log.txt",
						Index:    0,
					},
				},
			},
			skipPassedTest: true,
			alwaysFlake:    true,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			c := c // Make a copy to avoid modifying shared state.
			c.AttributeToTest = tc.attributeToTest
			c.AddTag = tc.addTag
			c.SkipPassedTask = tc.skipPassedTask
			c.SkipAllPassedTests = tc.skipAllPassedTests
			c.SkipPassedTest = tc.skipPassedTest
			c.IgnoreFlakes = tc.ignoreFlakes
			c.AlwaysFlake = tc.alwaysFlake
			// It accesses this field for DebugText().
			tc.testingOutputs.SwarmingSummary = &SwarmingTaskSummary{
				Results: &SwarmingRpcsTaskResult{
					TaskId: "abc", State: tc.swarmingResultState,
				},
			}
			c.OnlyOnStates = tc.states
			if tc.typeToCheck != "" {
				c.Type = tc.typeToCheck
			}
			if c.Check(&tc.testingOutputs) != tc.shouldMatch {
				t.Errorf("c.Check(%q) returned %t, expected %t",
					tc.name, !tc.shouldMatch, tc.shouldMatch)
			}
			gotName := c.Name()
			if tc.wantName != "" && gotName != tc.wantName {
				t.Errorf("c.Name() returned %q, want %q", gotName, tc.wantName)
			}
			if tc.attributeToTest && tc.addTag && tc.shouldMatch {
				if diff := cmp.Diff(tc.wantTags, c.Tags()); diff != "" {
					t.Errorf("c.Tags() returned diff (-want +got): %s", diff)
				}
			}
			c.DebugText() // minimal coverage, check it doesn't crash.
			swarmingOutputPerTest := tc.testingOutputs.SwarmingOutputPerTest
			gotOutputFiles := c.OutputFiles()
			if len(swarmingOutputPerTest) == 0 && len(gotOutputFiles) != 0 {
				t.Errorf("c.OutputFiles() returned %s, want []", gotOutputFiles)
			}
			if len(swarmingOutputPerTest) == 1 && (len(gotOutputFiles) != 1 || swarmingOutputPerTest[0].FilePath != gotOutputFiles[0]) {
				t.Errorf("c.OutputFiles() returned %s, want %q", gotOutputFiles, swarmingOutputPerTest[0].FilePath)
			}
			if tc.wantFlake != c.IsFlake() {
				t.Errorf("c.IsFlake() returned %t, expected %t", c.IsFlake(), tc.wantFlake)
			}
			if tc.wantLine != "" && tc.wantLine != c.line {
				t.Errorf("Got line %s, expected %s", c.line, tc.wantLine)
			}
		})
	}
}
