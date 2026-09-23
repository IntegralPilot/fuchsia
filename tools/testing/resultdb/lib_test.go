// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package resultdb

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	resultpb "go.chromium.org/luci/resultdb/proto/v1"
	sinkpb "go.chromium.org/luci/resultdb/sink/proto/v1"
	"google.golang.org/protobuf/proto"

	"github.com/google/go-cmp/cmp"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder/metadata"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

func TestParseSummary(t *testing.T) {
	const testCount = 10
	summary := createTestSummary(testCount)
	testResults, _, _ := SummaryToResultSink(summary, []*resultpb.StringPair{}, "")
	if len(testResults) != testCount {
		t.Errorf(
			"Parsed incorrect number of resultdb tests in TestSummary, got %d, want %d",
			len(testResults), testCount)
	}
	requests := createTestResultsRequests(testResults, testCount)
	if len(requests) != 1 {
		t.Errorf(
			"Grouped incorrect chunks of ResultDB sink requests, got %d, want 1",
			len(requests))
	}
	if len(requests[0].TestResults) != testCount {
		t.Errorf(
			"Incorrect number of TestResult in the first chunk, got %d, want %d",
			len(requests[0].TestResults), testCount)
	}
	if requests[0].TestResults[0].TestId != "test_0" {
		t.Errorf("Incorrect TestId parsed for first suite. got %s, want test_0", requests[0].TestResults[0].TestId)
	}
}

func TestSetTestDetailsToResultSink(t *testing.T) {
	outputRoot := t.TempDir()
	detail := createTestDetailWithPassedAndFailedTestCase(5, 2, outputRoot)
	expectedFailureReason := "some failure reason reported by test runner"
	detail.FailureReason = runtests.FailureReasonFromMessage(expectedFailureReason)
	// include 7 owners to test truncation of owner list
	detail.Metadata = metadata.TestMetadata{
		Owners: []string{
			"testgoogler1@google.com",
			"testgoogler2@google.com",
			"testgoogler3@google.com",
			"testgoogler4@google.com",
			"testgoogler5@google.com",
			"testgoogler6@google.com",
			"testgoogler7@google.com",
		},
		ComponentID: 1478143,
	}
	extraTags := []*resultpb.StringPair{
		{Key: "key1", Value: "value1"},
	}
	result, _, _, err := testDetailsToResultSink(extraTags, detail, outputRoot)
	if err != nil {
		t.Fatalf("Cannot parse test detail. got %s", err)
	}

	var gotErr string
	if result.FailureReason != nil && len(result.FailureReason.Errors) > 0 {
		gotErr = result.FailureReason.Errors[0].Message
	}
	if !(result.StatusV2 == resultpb.TestResult_FAILED && gotErr == expectedFailureReason) {
		t.Errorf("If a test failed, the top level test should have the reported failure reason.\n The error message is %q.\n The expected failure reason is %q.", gotErr, expectedFailureReason)
	}

	tags := make(map[string]string)
	for _, tag := range result.Tags {
		tags[tag.Key] = tag.Value
	}

	expectedTags := map[string]string{
		"key1":              "value1",
		"gn_label":          detail.GNLabel,
		"source_label":      detail.SourceLabel,
		"test_case_count":   "7",
		"affected":          "false",
		"is_top_level_test": "true",
		"owners":            "testgoogler1@google.com,testgoogler2@google.com,testgoogler3@google.com,testgoogler4@google.com,testgoogler5@google.com",
	}
	if diff := cmp.Diff(tags, expectedTags); diff != "" {
		t.Errorf("tags differ (-got +want):\n%s", diff)
	}

	if len(result.Artifacts) != 2 {
		t.Errorf("Got %d artifacts, want 2", len(result.Artifacts))
	}
	artifactNames := []string{}
	for name := range result.Artifacts {
		artifactNames = append(artifactNames, name)
	}
	sort.Strings(artifactNames)
	if diff := cmp.Diff(artifactNames, []string{"dir-1/outputfile", "dir_2/outputfile"}); diff != "" {
		t.Errorf("Diff in output files (-got +want):\n%s", diff)
	}
	expectedMetadata := resultpb.TestMetadata{
		BugComponent: &resultpb.BugComponent{
			System: &resultpb.BugComponent_IssueTracker{
				IssueTracker: &resultpb.IssueTrackerComponent{
					ComponentId: 1478143,
				},
			},
		},
	}
	if diff := cmp.Diff(result.TestMetadata.Name, expectedMetadata.Name); diff != "" {
		t.Errorf("Diff in metadata name (-got +want):\n%s", diff)
	}
	if diff := cmp.Diff(result.TestMetadata.BugComponent.GetIssueTracker().ComponentId, expectedMetadata.BugComponent.GetIssueTracker().ComponentId); diff != "" {
		t.Errorf("Diff in the bug component's component id (-got +want):\n%s", diff)
	}
}

func TestSetTestDetailsToResultSink_FailureReason_ExceedsMaxSize(t *testing.T) {
	outputRoot := t.TempDir()
	detail := createTestDetailWithPassedAndFailedTestCase(5, 200, outputRoot)
	detail.FailureReason = runtests.FailureReasonFromMessage(strings.Repeat("a", 2000))
	expectedFailureReason := strings.Repeat("a", MaxFailureReasonLength-3) + "..."
	extraTags := []*resultpb.StringPair{
		{Key: "key1", Value: "value1"},
	}
	result, _, _, err := testDetailsToResultSink(extraTags, detail, outputRoot)
	if err != nil {
		t.Fatalf("Cannot parse test detail. got %s", err)
	}

	var gotErr string
	if result.FailureReason != nil && len(result.FailureReason.Errors) > 0 {
		gotErr = result.FailureReason.Errors[0].Message
	}
	if !(result.StatusV2 == resultpb.TestResult_FAILED && gotErr == expectedFailureReason) {
		t.Errorf("If a test failed, the top level test should have the reported failure reason truncated.\n The error message is %q.\n The expected failure reason is %q.", gotErr, expectedFailureReason)
	}

	tags := make(map[string]string)
	for _, tag := range result.Tags {
		tags[tag.Key] = tag.Value
	}

	expectedTags := map[string]string{
		"key1":              "value1",
		"gn_label":          detail.GNLabel,
		"source_label":      detail.SourceLabel,
		"test_case_count":   "205",
		"affected":          "false",
		"is_top_level_test": "true",
	}
	if diff := cmp.Diff(tags, expectedTags); diff != "" {
		t.Errorf("tags differ (-got +want):\n%s", diff)
	}

	if len(result.Artifacts) != 2 {
		t.Errorf("Got %d artifacts, want 2", len(result.Artifacts))
	}
	artifactNames := []string{}
	for name := range result.Artifacts {
		artifactNames = append(artifactNames, name)
	}
	sort.Strings(artifactNames)
	if diff := cmp.Diff(artifactNames, []string{"dir-1/outputfile", "dir_2/outputfile"}); diff != "" {
		t.Errorf("Diff in output files (-got +want):\n%s", diff)
	}
}

func TestSetTestDetailsToResultSink_NonSuccessCases(t *testing.T) {
	outputRoot := t.TempDir()

	tests := []struct {
		name                  string
		detail                runtests.TestDetails
		expectedFailureReason string
	}{
		{
			name: "mixed_failure_and_skipped_with_reason",
			detail: runtests.TestDetails{
				Name:        "foo",
				GNLabel:     "some label",
				SourceLabel: "some source label",
				Status:      runtests.TestFailure,
				TestResult: runtests.TestResult{
					OutputDir:     "foo",
					FailureReason: runtests.FailureReasonFromMessage("some failure reason reported by test runner"),
					Cases: []runtests.TestCaseResult{
						{CaseName: "failed_case", Status: runtests.TestFailure},
						{CaseName: "skipped_with_reason", Status: runtests.TestSkipped},
						{CaseName: "skipped_without_reason", Status: runtests.TestSkipped},
					},
				},
			},
			expectedFailureReason: "some failure reason reported by test runner",
		},
		{
			name: "all_skipped_some_with_reason",
			detail: runtests.TestDetails{
				Name:        "foo",
				GNLabel:     "some label",
				SourceLabel: "some source label",
				Status:      runtests.TestSkipped,
				TestResult: runtests.TestResult{
					OutputDir: "foo",
					Cases: []runtests.TestCaseResult{
						{CaseName: "skipped_1", Status: runtests.TestSkipped},
						{CaseName: "skipped_2", Status: runtests.TestSkipped},
					},
				},
			},
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			extraTags := []*resultpb.StringPair{}
			result, _, _, err := testDetailsToResultSink(extraTags, &tc.detail, outputRoot)
			if err != nil {
				t.Fatalf("Cannot parse test detail: %s", err)
			}

			if tc.expectedFailureReason == "" {
				if result.FailureReason != nil {
					t.Errorf("got FailureReason %+v, want nil", result.FailureReason)
				}
			} else {
				if result.FailureReason == nil {
					t.Fatalf("Expected FailureReason to be non-nil")
				}
				var gotErr string
				if len(result.FailureReason.Errors) > 0 {
					gotErr = result.FailureReason.Errors[0].Message
				}
				if gotErr != tc.expectedFailureReason {
					t.Errorf("got failure reason %q, want %q", gotErr, tc.expectedFailureReason)
				}
			}
		})
	}
}

func TestSetTestCaseToResultSink(t *testing.T) {
	outputRoot := t.TempDir()
	detail := createTestDetailWithTestCase(5, outputRoot)
	detail.Metadata = metadata.TestMetadata{
		Owners: []string{
			"testgoogler1@google.com",
			"testgoogler2@google.com",
			"testgoogler3@google.com",
			"testgoogler4@google.com",
			"testgoogler5@google.com",
			"testgoogler6@google.com",
			"testgoogler7@google.com",
		},
		ComponentID: 1478143,
	}
	results, _, _ := testCaseToResultSink(detail.Cases, []*resultpb.StringPair{}, detail, outputRoot)
	if len(results) != 5 {
		t.Errorf("Got %d test case results, want 5", len(results))
	}

	for i, result := range results {
		tags := make(map[string]string)
		for _, tag := range result.Tags {
			tags[tag.Key] = tag.Value
		}
		expectedTags := map[string]string{
			"format":       detail.Cases[i].Format,
			"is_test_case": "true",
			"key1":         "value1",
			"owners":       "testgoogler1@google.com,testgoogler2@google.com,testgoogler3@google.com,testgoogler4@google.com,testgoogler5@google.com",
		}
		if diff := cmp.Diff(tags, expectedTags); diff != "" {
			t.Errorf("tags differ (-got +want):\n%s", diff)
		}
		if len(result.Artifacts) != 2 {
			t.Errorf("Got %d artifacts for test case %d, want 2", len(result.Artifacts), i+1)
		}
		artifactNames := []string{}
		for name := range result.Artifacts {
			artifactNames = append(artifactNames, name)
		}
		sort.Strings(artifactNames)
		if diff := cmp.Diff(artifactNames, []string{"case/outputfile1", "case/outputfile2"}); diff != "" {
			t.Errorf("Diff in output files (-got +want):\n%s", diff)
		}
		expectedMetadata := resultpb.TestMetadata{
			Name: detail.Cases[i].DisplayName,
			BugComponent: &resultpb.BugComponent{
				System: &resultpb.BugComponent_IssueTracker{
					IssueTracker: &resultpb.IssueTrackerComponent{
						ComponentId: 1478143,
					},
				},
			},
		}
		if diff := cmp.Diff(result.TestMetadata.Name, expectedMetadata.Name); diff != "" {
			t.Errorf("Diff in metadata name (-got +want):\n%s", diff)
		}
		if diff := cmp.Diff(result.TestMetadata.BugComponent.GetIssueTracker().ComponentId, expectedMetadata.BugComponent.GetIssueTracker().ComponentId); diff != "" {
			t.Errorf("Diff in the bug component's component id (-got +want):\n%s", diff)
		}
	}
}

func TestSetTestCaseToResultSink_WithFailureReason(t *testing.T) {
	outputRoot := t.TempDir()
	detail := &runtests.TestDetails{
		Name:      "foo",
		Status:    runtests.TestFailure,
		StartTime: time.Now(),
		TestResult: runtests.TestResult{
			Cases: []runtests.TestCaseResult{
				{
					DisplayName: "foo/bar_failed",
					SuiteName:   "foo",
					CaseName:    "bar_failed",
					Status:      runtests.TestFailure,
					Format:      "Rust",
					FailureReason: &runtests.FailureReason{
						Errors: []*runtests.FailureReasonError{
							{Message: "error message 1"},
							{Message: "error message 2"},
						},
					},
				},
			},
		},
	}

	results, _, _ := testCaseToResultSink(detail.Cases, []*resultpb.StringPair{}, detail, outputRoot)
	if len(results) != 1 {
		t.Fatalf("Got %d test case results, want 1", len(results))
	}

	res := results[0]
	if res.FailureReason == nil {
		t.Fatalf("Expected FailureReason to be non-nil")
	}
	if len(res.FailureReason.Errors) != 2 {
		t.Fatalf("got %d Errors, want 2", len(res.FailureReason.Errors))
	}
	if res.FailureReason.Errors[0].Message != "error message 1" || res.FailureReason.Errors[1].Message != "error message 2" {
		t.Errorf("unexpected error messages: %v", res.FailureReason.Errors)
	}
}

func TestSetTestCaseToResultSink_WithFailureReason_DefaultKind(t *testing.T) {
	outputRoot := t.TempDir()
	detail := &runtests.TestDetails{
		Name:      "foo",
		Status:    runtests.TestFailure,
		StartTime: time.Now(),
		TestResult: runtests.TestResult{
			Cases: []runtests.TestCaseResult{
				{
					DisplayName: "foo/bar_failed",
					SuiteName:   "foo",
					CaseName:    "bar_failed",
					Status:      runtests.TestAborted, // Should default kind to TIMEOUT
					Format:      "Rust",
					FailureReason: &runtests.FailureReason{
						Errors: []*runtests.FailureReasonError{
							{Message: "timeout error message"},
						},
					},
				},
				{
					DisplayName:   "foo/bar_crash",
					SuiteName:     "foo",
					CaseName:      "bar_crash",
					Status:        runtests.TestInfraFailure, // Should default kind to CRASH
					Format:        "Rust",
					FailureReason: runtests.FailureReasonFromMessage("infra failure message"),
				},
			},
		},
	}

	results, _, _ := testCaseToResultSink(detail.Cases, []*resultpb.StringPair{}, detail, outputRoot)
	if len(results) != 2 {
		t.Fatalf("Got %d test case results, want 2", len(results))
	}

	res0 := results[0]
	if res0.FailureReason == nil {
		t.Fatalf("Expected res0 FailureReason to be non-nil")
	}
	if res0.FailureReason.Kind != resultpb.FailureReason_TIMEOUT {
		t.Errorf("got Kind = %v, want %v", res0.FailureReason.Kind, resultpb.FailureReason_TIMEOUT)
	}
	if len(res0.FailureReason.Errors) != 1 || res0.FailureReason.Errors[0].Message != "timeout error message" {
		t.Errorf("unexpected res0 errors: %v", res0.FailureReason.Errors)
	}

	res1 := results[1]
	if res1.FailureReason == nil {
		t.Fatalf("Expected res1 FailureReason to be non-nil")
	}
	if res1.FailureReason.Kind != resultpb.FailureReason_CRASH {
		t.Errorf("got Kind = %v, want %v", res1.FailureReason.Kind, resultpb.FailureReason_CRASH)
	}
	if len(res1.FailureReason.Errors) != 1 || res1.FailureReason.Errors[0].Message != "infra failure message" {
		t.Errorf("unexpected res1 errors: %v", res1.FailureReason.Errors)
	}
}

func TestTestDetailsToResultSink_PanicsOnNilErrorsInCases(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Errorf("expected panic when error in slice is nil")
		}
	}()

	outputRoot := t.TempDir()
	detail := &runtests.TestDetails{
		Name:      "foo",
		Status:    runtests.TestFailure,
		StartTime: time.Now(),
		TestResult: runtests.TestResult{
			FailureReason: &runtests.FailureReason{
				Errors: []*runtests.FailureReasonError{
					nil,
					{Message: "first valid error"},
				},
			},
		},
	}

	_, _, _, _ = testDetailsToResultSink([]*resultpb.StringPair{}, detail, outputRoot)
}

func TestToResultDBFailureReason_PanicsOnNilError(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Errorf("expected panic when error in slice is nil")
		}
	}()

	fr := &runtests.FailureReason{
		Errors: []*runtests.FailureReasonError{
			{Message: "valid error 1"},
			nil,
		},
	}

	_ = toResultDBFailureReason(fr, resultpb.FailureReason_ORDINARY)
}

func TestToResultDBFailureReason_Truncation(t *testing.T) {
	// 1. Test message (>1024B) truncation
	fr := &runtests.FailureReason{
		Errors: []*runtests.FailureReasonError{
			{
				Message: strings.Repeat("m", 1500),
			},
		},
	}
	res := toResultDBFailureReason(fr, resultpb.FailureReason_ORDINARY)
	if len(res.Errors) != 1 {
		t.Fatalf("got %d errors, want 1", len(res.Errors))
	}
	if len(res.Errors[0].Message) != MaxFailureReasonLength {
		t.Errorf("got message len %d, want %d", len(res.Errors[0].Message), MaxFailureReasonLength)
	}
	if !strings.HasSuffix(res.Errors[0].Message, "...") {
		t.Errorf("expected message to end with '...'")
	}

	// 2. Test total error list size (>16KB) truncation
	frList := &runtests.FailureReason{}
	for i := 0; i < 20; i++ {
		frList.Errors = append(frList.Errors, &runtests.FailureReasonError{
			Message: strings.Repeat("e", 1000),
		})
	}
	resList := toResultDBFailureReason(frList, resultpb.FailureReason_ORDINARY)
	if proto.Size(resList) > MaxFailureReasonTotalSize {
		t.Errorf("got proto size %d, want <= %d", proto.Size(resList), MaxFailureReasonTotalSize)
	}
	if resList.TruncatedErrorsCount == 0 {
		t.Errorf("expected TruncatedErrorsCount > 0 when exceeding 16KB limit")
	}
	if len(resList.Errors)+int(resList.TruncatedErrorsCount) != 20 {
		t.Errorf("got %d kept errors + %d truncated errors, want 20 total", len(resList.Errors), resList.TruncatedErrorsCount)
	}
}

func createTestSummary(testCount int) *runtests.TestSummary {
	t := []runtests.TestDetails{}
	for i := 0; i < testCount; i++ {
		t = append(t, runtests.TestDetails{
			Name:                 fmt.Sprintf("test_%d", i),
			GNLabel:              "some label",
			SourceLabel:          "some source label",
			TestResult:           runtests.TestResult{OutputFiles: []string{"some file path"}},
			Status:               runtests.TestSuccess,
			StartTime:            time.Now(),
			DurationMillis:       39797,
			IsTestingFailureMode: false,
		})
	}
	return &runtests.TestSummary{Tests: t}
}

func createTestDetailWithTestCase(testCase int, outputRoot string) *runtests.TestDetails {
	t := []runtests.TestCaseResult{}
	if outputRoot != "" {
		for _, f := range []string{"foo/dir-1/outputfile", "foo/dir#2/outputfile", "foo/case/outputfile1", "foo/case/outputfile2"} {
			outputfile := filepath.Join(outputRoot, f)
			os.MkdirAll(filepath.Dir(outputfile), os.ModePerm)
			os.WriteFile(outputfile, []byte("output"), os.ModePerm)
		}
	}
	for i := 0; i < testCase; i++ {
		t = append(t, runtests.TestCaseResult{
			DisplayName: fmt.Sprintf("foo/bar_%d", i),
			SuiteName:   "foo",
			CaseName:    fmt.Sprintf("bar_%d", i),
			Status:      runtests.TestSuccess,
			Format:      "Rust",
			OutputFiles: []string{"case/outputfile1", "case/outputfile2"},
			Tags:        []build.TestTag{{"key1", "value1"}},
		})
	}
	return &runtests.TestDetails{
		Name:        "foo",
		GNLabel:     "some label",
		SourceLabel: "some source label",
		TestResult: runtests.TestResult{
			OutputFiles: []string{"dir-1/outputfile", "dir#2/outputfile"},
			OutputDir:   "foo",
			Cases:       t,
		},
		Status:               runtests.TestSuccess,
		StartTime:            time.Now(),
		DurationMillis:       39797,
		IsTestingFailureMode: false,
	}
}

func createTestDetailWithPassedAndFailedTestCase(passedTestCase int, failedTestCase int, outputRoot string) *runtests.TestDetails {
	t := []runtests.TestCaseResult{}
	if outputRoot != "" {
		for _, f := range []string{"dir-1/outputfile", "dir#2/outputfile", "case/outputfile1", "case/outputfile2"} {
			outputfile := filepath.Join(outputRoot, f)
			os.MkdirAll(filepath.Dir(outputfile), os.ModePerm)
			os.WriteFile(outputfile, []byte("output"), os.ModePerm)
		}
	}
	for i := 0; i < passedTestCase; i++ {
		t = append(t, runtests.TestCaseResult{
			DisplayName: fmt.Sprintf("foo/bar_%d", i),
			SuiteName:   "foo",
			CaseName:    fmt.Sprintf("bar_%d", i),
			Status:      runtests.TestSuccess,
			Format:      "Rust",
			OutputFiles: []string{"case/outputfile1", "case/outputfile2"},
			Tags:        []build.TestTag{{"key1", "value1"}},
		})
	}
	for i := 0; i < failedTestCase; i++ {
		t = append(t, runtests.TestCaseResult{
			DisplayName: fmt.Sprintf("foo/bar_%d", i),
			SuiteName:   "foo",
			CaseName:    fmt.Sprintf("bar_%d", i),
			Status:      runtests.TestFailure,
			Format:      "Rust",
			OutputFiles: []string{"case/outputfile1", "case/outputfile2"},
			Tags:        []build.TestTag{{"key1", "value1"}},
		})
	}
	finalResult := runtests.TestSuccess
	if failedTestCase > 0 {
		finalResult = runtests.TestFailure
	}
	return &runtests.TestDetails{
		Name:        "foo",
		GNLabel:     "some label",
		SourceLabel: "some source label",
		TestResult: runtests.TestResult{
			OutputFiles: []string{"dir-1/outputfile", "dir#2/outputfile"},
			Cases:       t,
		},
		Status:               finalResult,
		StartTime:            time.Now(),
		DurationMillis:       39797,
		IsTestingFailureMode: false,
	}
}

func TestIsReadable(t *testing.T) {
	if r := isReadable(""); r {
		t.Errorf("Empty string cannot be readable. got %t, want false", r)
	}
	if r := isReadable(*testDataDir); r {
		t.Errorf("Directory should not be readable. got %t, want false", r)
	}
	luciCtx := filepath.Join(*testDataDir, "lucictx.json")
	if r := isReadable(luciCtx); !r {
		t.Errorf("File %v should be readable. got %t, want true", luciCtx, r)
	}
}

func TestInvocationLevelArtifacts(t *testing.T) {
	invocationLogs := []string{"syslog.txt", "serial_log.txt", "nonexistent_log.txt"}
	artifacts := InvocationLevelArtifacts(*testDataDir, invocationLogs)
	foundSyslog := false
	foundSerial := false
	for logName := range artifacts {
		switch logName {
		case "syslog.txt":
			foundSyslog = true
		case "serial_log.txt":
			foundSerial = true
		default:
			t.Errorf("Found unexpected log (%s), expect only syslog.txt or serial_log.txt", logName)
		}
	}
	if !foundSyslog {
		t.Errorf("Did not find syslog.txt in output")
	}
	if !foundSerial {
		t.Errorf("Did not find serial_log.txt in output")
	}
}

func TestTruncateString(t *testing.T) {
	testCases := []struct {
		testStr string
		want    string
		limit   int // bytes
	}{
		{
			testStr: "ab£cdefg",
			want:    "",
			limit:   1,
		}, {
			testStr: "ab£cdefg",
			want:    "ab...",
			limit:   5,
		}, {
			testStr: "ab£cdefg",
			want:    "ab...",
			limit:   6,
		}, {
			testStr: "ab£cdefg",
			want:    "ab£...",
			limit:   7,
		}, {
			testStr: "♥LoveFuchsia",
			want:    "",
			limit:   3,
		}, {
			testStr: "♥LoveFuchsia",
			want:    "",
			limit:   4,
		}, {
			testStr: "♥LoveFuchsia",
			want:    "",
			limit:   5,
		}, {
			testStr: "♥LoveFuchsia",
			want:    "♥...",
			limit:   6,
		}, {
			testStr: "♥LoveFuchsia",
			want:    "♥L...",
			limit:   7,
		}, {
			testStr: "♥LoveFuchsia",
			want:    "♥LoveFuc...",
			limit:   13,
		}, {
			testStr: "♥LoveFuchsia",
			want:    "♥LoveFuchsia",
			limit:   14,
		}, {
			testStr: "♥LoveFuchsia",
			want:    "♥LoveFuchsia",
			limit:   100,
		},
	}
	for _, tc := range testCases {
		r := truncateString(tc.testStr, tc.limit)
		if r != tc.want {
			t.Errorf("TestTruncateString failed for input: %q(%d), got %q, want %q",
				tc.testStr, tc.limit, r, tc.want)
		}
	}
}

func TestExoneratedTestCase(t *testing.T) {
	outputRoot := t.TempDir()
	detail := &runtests.TestDetails{
		Name:      "foo",
		Status:    runtests.TestFailure,
		StartTime: time.Now(),
		TestResult: runtests.TestResult{
			Cases: []runtests.TestCaseResult{
				{
					DisplayName:   "foo/bar_exonerated",
					SuiteName:     "foo",
					CaseName:      "bar_exonerated",
					Status:        runtests.TestExonerated,
					Format:        "Rust",
					FailureReason: runtests.FailureReasonFromMessage("Flaky test instance"),
				},
				{
					DisplayName: "foo/bar_passed",
					SuiteName:   "foo",
					CaseName:    "bar_passed",
					Status:      runtests.TestSuccess,
					Format:      "Rust",
				},
			},
		},
	}

	results, exonerations, skipped := testCaseToResultSink(detail.Cases, []*resultpb.StringPair{}, detail, outputRoot)

	if len(results) != 2 {
		t.Fatalf("Got %d test case results, want 2", len(results))
	}
	if len(skipped) != 0 {
		t.Errorf("Got skipped tests %v, want 0", skipped)
	}
	if len(exonerations) != 1 {
		t.Fatalf("Got %d exonerations, want 1", len(exonerations))
	}

	// Verify the first result is the exonerated one (FAILED)
	exoneratedCaseResult := results[0]
	if exoneratedCaseResult.TestId != "foo/foo:bar_exonerated" {
		t.Errorf("Unexpected TestId for exonerated case: %s", exoneratedCaseResult.TestId)
	}
	if exoneratedCaseResult.StatusV2 != resultpb.TestResult_FAILED {
		t.Errorf("Exonerated case result status should be FAILED, got: %s", exoneratedCaseResult.StatusV2)
	}
	if exoneratedCaseResult.FailureReason == nil || len(exoneratedCaseResult.FailureReason.Errors) == 0 {
		t.Fatalf("Exonerated case result should have FailureReason")
	}
	if exoneratedCaseResult.FailureReason.Errors[0].Message != "Flaky test instance" {
		t.Errorf("Unexpected FailureReason message: %s", exoneratedCaseResult.FailureReason.Errors[0].Message)
	}

	// Verify the second result is the passed one (PASSED)
	passedCaseResult := results[1]
	if passedCaseResult.TestId != "foo/foo:bar_passed" {
		t.Errorf("Unexpected TestId for passed case: %s", passedCaseResult.TestId)
	}
	if passedCaseResult.StatusV2 != resultpb.TestResult_PASSED {
		t.Errorf("Passed case result status should be PASSED, got: %s", passedCaseResult.StatusV2)
	}
	if passedCaseResult.FailureReason != nil {
		t.Errorf("Passed case result should not have FailureReason, got: %+v", passedCaseResult.FailureReason)
	}

	// Verify that the exoneration maps to the exact test case ID
	exoneration := exonerations[0]
	if exoneration.TestId != "foo/foo:bar_exonerated" {
		t.Errorf("Exoneration maps to incorrect TestId: got %s, want foo/foo:bar_exonerated", exoneration.TestId)
	}
	if exoneration.Reason != resultpb.ExonerationReason_NOT_CRITICAL {
		t.Errorf("Unexpected Exoneration reason: got %v, want NOT_CRITICAL", exoneration.Reason)
	}
	if !strings.Contains(exoneration.ExplanationHtml, "bar_exonerated") {
		t.Errorf("ExplanationHtml does not name the testcase: got %q", exoneration.ExplanationHtml)
	}
}

func TestExoneratedTestDetail(t *testing.T) {
	outputRoot := t.TempDir()
	detail := &runtests.TestDetails{
		Name:           "foo_exonerated_target",
		Status:         runtests.TestExonerated,
		StartTime:      time.Now(),
		DurationMillis: 500,
	}

	result, exoneration, skipped, err := testDetailsToResultSink([]*resultpb.StringPair{}, detail, outputRoot)
	if err != nil {
		t.Fatalf("Unexpected error mapping details: %v", err)
	}
	if skipped != "" {
		t.Errorf("Got skipped test %q, want empty", skipped)
	}
	if result == nil {
		t.Fatalf("Result is nil")
	}
	if exoneration == nil {
		t.Fatalf("Exoneration is nil")
	}

	// Verify that the exonerated target has its result mapped as FAILED
	if result.TestId != "foo_exonerated_target" {
		t.Errorf("Unexpected TestId: got %s", result.TestId)
	}
	if result.StatusV2 != resultpb.TestResult_FAILED {
		t.Errorf("Exonerated target status should be FAILED, got: %s", result.StatusV2)
	}

	// Verify the target's exoneration maps to the target's flat name
	if exoneration.TestId != "foo_exonerated_target" {
		t.Errorf("Exoneration maps to incorrect TestId: got %s", exoneration.TestId)
	}
	if exoneration.Reason != resultpb.ExonerationReason_NOT_CRITICAL {
		t.Errorf("Unexpected Exoneration reason: got %v", exoneration.Reason)
	}
	if !strings.Contains(exoneration.ExplanationHtml, "foo_exonerated_target") {
		t.Errorf("ExplanationHtml does not name the test target: got %q", exoneration.ExplanationHtml)
	}
}

func TestExoneratedSummaryToResultSink(t *testing.T) {
	summary := &runtests.TestSummary{
		Tests: []runtests.TestDetails{
			{
				Name:      "test_exonerated_target",
				Status:    runtests.TestExonerated,
				StartTime: time.Now(),
			},
			{
				Name:      "test_exonerated_case",
				Status:    runtests.TestFailure,
				StartTime: time.Now(),
				TestResult: runtests.TestResult{
					Cases: []runtests.TestCaseResult{
						{
							DisplayName: "test_exonerated_case/suite:case",
							SuiteName:   "suite",
							CaseName:    "case",
							Status:      runtests.TestExonerated,
						},
					},
				},
			},
			{
				Name:      "test_passed",
				Status:    runtests.TestSuccess,
				StartTime: time.Now(),
			},
		},
	}

	results, exonerations, skipped := SummaryToResultSink(summary, []*resultpb.StringPair{}, "")

	// test_exonerated_target reports 1 result (itself) and 1 target exoneration.
	// test_exonerated_case reports 2 results (the case and the target details) and 1 case exoneration.
	// test_passed reports 1 result (itself).
	// Total expected results: 1 (target) + 1 (case) + 1 (details of target) + 1 (passed target) = 4
	if len(results) != 4 {
		t.Errorf("Got %d results, want 4", len(results))
	}
	if len(exonerations) != 2 {
		t.Fatalf("Got %d exonerations, want 2", len(exonerations))
	}
	if len(skipped) != 0 {
		t.Errorf("Got skipped tests %v, want 0", skipped)
	}

	// Verify the two exonerations map to their respective targets
	exonerationIDs := []string{exonerations[0].TestId, exonerations[1].TestId}
	sort.Strings(exonerationIDs)

	expectedIDs := []string{"test_exonerated_case/suite:case", "test_exonerated_target"}
	if diff := cmp.Diff(exonerationIDs, expectedIDs); diff != "" {
		t.Errorf("Exoneration IDs differ (-got +want):\n%s", diff)
	}
}

func TestRequestsChunking(t *testing.T) {
	t.Run("TestResults_Chunking", func(t *testing.T) {
		const resultCount = MaxBatchSize*2 + 1
		results := make([]*sinkpb.TestResult, resultCount)
		for i := 0; i < resultCount; i++ {
			results[i] = &sinkpb.TestResult{
				TestId: fmt.Sprintf("test_result_%d", i),
			}
		}

		resultRequests := createTestResultsRequests(results, MaxBatchSize)

		// Expected partition chunks: MaxBatchSize, MaxBatchSize, 1 (total 3 chunks)
		if len(resultRequests) != 3 {
			t.Fatalf("Expected 3 TestResults request chunks, got: %d", len(resultRequests))
		}
		if len(resultRequests[0].TestResults) != MaxBatchSize {
			t.Errorf("First chunk should have %d results, got: %d", MaxBatchSize, len(resultRequests[0].TestResults))
		}
		if len(resultRequests[1].TestResults) != MaxBatchSize {
			t.Errorf("Second chunk should have %d results, got: %d", MaxBatchSize, len(resultRequests[1].TestResults))
		}
		if len(resultRequests[2].TestResults) != 1 {
			t.Errorf("Third chunk should have 1 result, got: %d", len(resultRequests[2].TestResults))
		}
		expectedLastResultId := fmt.Sprintf("test_result_%d", resultCount-1)
		if resultRequests[2].TestResults[0].TestId != expectedLastResultId {
			t.Errorf("Unexpected TestId in last chunk: got %s, want %s", resultRequests[2].TestResults[0].TestId, expectedLastResultId)
		}
	})

	t.Run("TestResults_Empty", func(t *testing.T) {
		emptyResults := createTestResultsRequests(nil, MaxBatchSize)
		if emptyResults != nil {
			t.Errorf("Expected nil for empty results chunking, got: %+v", emptyResults)
		}
	})

	t.Run("TestExonerations_Chunking", func(t *testing.T) {
		const exonerationCount = MaxBatchSize*2 + 1
		exonerations := make([]*sinkpb.TestExoneration, exonerationCount)
		for i := 0; i < exonerationCount; i++ {
			exonerations[i] = &sinkpb.TestExoneration{
				TestId: fmt.Sprintf("test_exoneration_%d", i),
			}
		}

		exonerationRequests := createTestExonerationsRequests(exonerations, MaxBatchSize)

		// Expected partition chunks: MaxBatchSize, MaxBatchSize, 1 (total 3 chunks)
		if len(exonerationRequests) != 3 {
			t.Fatalf("Expected 3 TestExonerations request chunks, got: %d", len(exonerationRequests))
		}
		if len(exonerationRequests[0].TestExonerations) != MaxBatchSize {
			t.Errorf("First chunk should have %d exonerations, got: %d", MaxBatchSize, len(exonerationRequests[0].TestExonerations))
		}
		if len(exonerationRequests[1].TestExonerations) != MaxBatchSize {
			t.Errorf("Second chunk should have %d exonerations, got: %d", MaxBatchSize, len(exonerationRequests[1].TestExonerations))
		}
		if len(exonerationRequests[2].TestExonerations) != 1 {
			t.Errorf("Third chunk should have 1 exoneration, got: %d", len(exonerationRequests[2].TestExonerations))
		}
		expectedLastExonerationId := fmt.Sprintf("test_exoneration_%d", exonerationCount-1)
		if exonerationRequests[2].TestExonerations[0].TestId != expectedLastExonerationId {
			t.Errorf("Unexpected TestId in last chunk: got %s, want %s", exonerationRequests[2].TestExonerations[0].TestId, expectedLastExonerationId)
		}
	})

	t.Run("TestExonerations_Empty", func(t *testing.T) {
		emptyExonerations := createTestExonerationsRequests(nil, MaxBatchSize)
		if emptyExonerations != nil {
			t.Errorf("Expected nil for empty exonerations chunking, got: %+v", emptyExonerations)
		}
	})
}

func TestParseTestLocation(t *testing.T) {
	testCases := []struct {
		name  string
		label string
		want  *resultpb.TestLocation
	}{
		{
			name:  "standard GN label with target",
			label: "//src/sys/test_manager:test-manager-unittests",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/sys/test_manager",
			},
		},
		{
			name:  "GN label with toolchain",
			label: "//src/sys/test_manager:test-manager-unittests(//build/toolchain:x64)",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/sys/test_manager",
			},
		},
		{
			name:  "GN label with trailing slash in dir",
			label: "//src/sys/test_manager/:test-manager-unittests",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/sys/test_manager",
			},
		},
		{
			name:  "Bazel single at label",
			label: "@//src/sys/test_manager:test-manager-unittests",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/sys/test_manager",
			},
		},
		{
			name:  "Bazel double at label",
			label: "@@//src/sys/test_manager:test-manager-unittests",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/sys/test_manager",
			},
		},
		{
			name:  "Label with triple slashes",
			label: "///src:test",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src",
			},
		},
		{
			name:  "Label with triple slashes and file target",
			label: "///src/tests:test_runner.py",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/tests/test_runner.py",
			},
		},
		{
			name:  "Target with python script extension",
			label: "//src/tests:test_runner.py",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/tests/test_runner.py",
			},
		},
		{
			name:  "Target with cc extension",
			label: "//zircon/system/ulib/c/test:libc-test.cc",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//zircon/system/ulib/c/test/libc-test.cc",
			},
		},
		{
			name:  "Target with .cm component manifest",
			label: "//src/sys/test_manager:test-manager.cm",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/sys/test_manager",
			},
		},
		{
			name:  "Root package target",
			label: "//:tests",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//",
			},
		},
		{
			name:  "Root package target with file extension",
			label: "//:test.py",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//test.py",
			},
		},
		{
			name:  "Package directory only without colon",
			label: "//src/sys/test_manager",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/sys/test_manager",
			},
		},
		{
			name:  "Vendor google subsystem label",
			label: "//vendor/google/subsystem/tests:my_test",
			want: &resultpb.TestLocation{
				Repo:     VendorGoogleRepo,
				FileName: "//subsystem/tests",
			},
		},
		{
			name:  "Vendor google root target",
			label: "//vendor/google:my_test",
			want: &resultpb.TestLocation{
				Repo:     VendorGoogleRepo,
				FileName: "//",
			},
		},
		{
			name:  "Vendor google file target",
			label: "//vendor/google:test.py",
			want: &resultpb.TestLocation{
				Repo:     VendorGoogleRepo,
				FileName: "//test.py",
			},
		},
		{
			name:  "Vendor google nested file target",
			label: "//vendor/google/pkg:test.py",
			want: &resultpb.TestLocation{
				Repo:     VendorGoogleRepo,
				FileName: "//pkg/test.py",
			},
		},
		{
			name:  "Empty label",
			label: "",
			want:  nil,
		},
		{
			name:  "Whitespace label",
			label: "   ",
			want:  nil,
		},
		{
			name:  "Label with spaces",
			label: "some source label",
			want:  nil,
		},
		{
			name:  "Label without leading slashes",
			label: "src/sys:test",
			want:  nil,
		},
		{
			name:  "Path escaping root with ..",
			label: "//../../outside:test",
			want:  nil,
		},
		{
			name:  "Label with triple slashes escaping root with ..",
			label: "///../../outside:test",
			want:  nil,
		},
		{
			name:  "Toolchain missing closing parenthesis",
			label: "//src/sys:test(//build/toolchain:x64",
			want:  nil,
		},
		{
			name:  "Toolchain only without label",
			label: "(//build/toolchain:x64)",
			want:  nil,
		},
		{
			name:  "Unmatched closing parenthesis",
			label: "//src/sys:test)",
			want:  nil,
		},
		{
			name:  "Label with parentheses in path without toolchain",
			label: "//src/foo(bar):baz",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/foo(bar)",
			},
		},
		{
			name:  "Label with parentheses in path and toolchain",
			label: "//src/foo(bar):baz(//build/toolchain:x64)",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/foo(bar)",
			},
		},
		{
			name:  "Label with parentheses in directory path",
			label: "//src/foo(bar)/tests:my_test",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/foo(bar)/tests",
			},
		},
		{
			name:  "Toolchain with @// prefix",
			label: "//src/sys:test(@//build/toolchain:x64)",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/sys",
			},
		},
		{
			name:  "Toolchain with @@// prefix",
			label: "//src/sys:test(@@//build/toolchain:x64)",
			want: &resultpb.TestLocation{
				Repo:     DefaultRepo,
				FileName: "//src/sys",
			},
		},
		{
			name:  "Invalid toolchain ending in parenthesis",
			label: "//src/sys:test(invalid)",
			want:  nil,
		},
		{
			name:  "Filename exceeds max length",
			label: "//" + strings.Repeat("a/", MaxLocationFileNameLength/2+1) + ":my_test",
			want:  nil,
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			got, err := parseTestLocation(tc.label)
			if tc.want == nil {
				if err == nil {
					t.Errorf("parseTestLocation(%q) expected error, got nil", tc.label)
				}
			} else {
				if err != nil {
					t.Errorf("parseTestLocation(%q) unexpected error: %v", tc.label, err)
				}
				if !proto.Equal(got, tc.want) {
					t.Errorf("parseTestLocation(%q) = %+v, want %+v", tc.label, got, tc.want)
				}
			}
		})
	}
}

func TestSetTestMetadata_Location(t *testing.T) {
	t.Run("SourceLabel takes precedence", func(t *testing.T) {
		detail := runtests.TestDetails{
			SourceLabel: "//src/sys/foo:foo_test",
			GNLabel:     "//src/gn/bar:bar_test",
		}
		r := &sinkpb.TestResult{}
		setTestMetadata(r, detail, "foo")
		if r.TestMetadata == nil || r.TestMetadata.Location == nil {
			t.Fatalf("expected Location to be populated")
		}
		want := &resultpb.TestLocation{
			Repo:     DefaultRepo,
			FileName: "//src/sys/foo",
		}
		if !proto.Equal(r.TestMetadata.Location, want) {
			t.Errorf("Location mismatch: got %+v, want %+v", r.TestMetadata.Location, want)
		}
	})

	t.Run("GNLabel is used as fallback when SourceLabel is empty", func(t *testing.T) {
		detail := runtests.TestDetails{
			GNLabel: "//src/gn/bar:bar_test(//build/toolchain:x64)",
		}
		r := &sinkpb.TestResult{}
		setTestMetadata(r, detail, "bar")
		if r.TestMetadata == nil || r.TestMetadata.Location == nil {
			t.Fatalf("expected Location to be populated from GNLabel fallback")
		}
		want := &resultpb.TestLocation{
			Repo:     DefaultRepo,
			FileName: "//src/gn/bar",
		}
		if !proto.Equal(r.TestMetadata.Location, want) {
			t.Errorf("Location mismatch: got %+v, want %+v", r.TestMetadata.Location, want)
		}
	})

	t.Run("invalid label leaves location nil", func(t *testing.T) {
		detail := runtests.TestDetails{
			SourceLabel: "invalid label with spaces",
		}
		r := &sinkpb.TestResult{}
		setTestMetadata(r, detail, "invalid")
		if r.TestMetadata == nil {
			t.Fatalf("expected TestMetadata to not be nil")
		}
		if r.TestMetadata.Location != nil {
			t.Errorf("expected Location to be nil, got %+v", r.TestMetadata.Location)
		}
	})
}

func TestTestDetailsToResultSink_Location(t *testing.T) {
	outputRoot := t.TempDir()
	detail := &runtests.TestDetails{
		Name:        "foo_test",
		SourceLabel: "//src/sys/foo:foo_test",
		Status:      runtests.TestSuccess,
		StartTime:   time.Now(),
	}
	result, _, _, err := testDetailsToResultSink([]*resultpb.StringPair{}, detail, outputRoot)
	if err != nil {
		t.Fatalf("testDetailsToResultSink failed: %v", err)
	}
	if result.TestMetadata == nil || result.TestMetadata.Location == nil {
		t.Fatalf("expected TestMetadata.Location to be set")
	}
	want := &resultpb.TestLocation{
		Repo:     DefaultRepo,
		FileName: "//src/sys/foo",
	}
	if !proto.Equal(result.TestMetadata.Location, want) {
		t.Errorf("Location diff: got %+v, want %+v", result.TestMetadata.Location, want)
	}
}

func TestTestCaseToResultSink_Location(t *testing.T) {
	outputRoot := t.TempDir()
	detail := &runtests.TestDetails{
		Name:        "vendor_test",
		SourceLabel: "//vendor/google/tests:vendor_test",
		Status:      runtests.TestSuccess,
		StartTime:   time.Now(),
		TestResult: runtests.TestResult{
			Cases: []runtests.TestCaseResult{
				{
					DisplayName: "vendor_test/case_1",
					SuiteName:   "suite",
					CaseName:    "case_1",
					Status:      runtests.TestSuccess,
				},
			},
		},
	}
	results, _, _ := testCaseToResultSink(detail.Cases, []*resultpb.StringPair{}, detail, outputRoot)
	if len(results) != 1 {
		t.Fatalf("expected 1 result, got %d", len(results))
	}
	res := results[0]
	if res.TestMetadata == nil || res.TestMetadata.Location == nil {
		t.Fatalf("expected TestMetadata.Location to be set")
	}
	want := &resultpb.TestLocation{
		Repo:     VendorGoogleRepo,
		FileName: "//tests",
	}
	if !proto.Equal(res.TestMetadata.Location, want) {
		t.Errorf("Location diff: got %+v, want %+v", res.TestMetadata.Location, want)
	}
}
