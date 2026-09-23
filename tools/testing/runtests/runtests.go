// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Package runtests contains specifics related to the runtests command.
package runtests

import (
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder/metadata"
)

// TODO(olivernewman): Move the contents of this file into a separate library as
// it's no longer specific to runtests - in fact, runtests only implements a
// subset of the types and fields defined here.

const (
	// TestSummaryFilename is the summary file name expected by the fuchsia
	// recipe module.
	TestSummaryFilename = "summary.json"

	// TestOutputFilename is the default output file name for a test.
	TestOutputFilename = "stdout-and-stderr.txt"
)

// TestStatus is the exit status of a test.
type TestStatus string

// Possible test statuses, names chosen for consistency with ResultDB:
// https://chromium.googlesource.com/infra/luci/luci-go/+/15458901113063d2a0d95be2e27a245c4cfd5690/resultdb/proto/v1/test_result.proto#142
const (
	// TestSuccess represents a passed test.
	TestSuccess TestStatus = "PASS"

	// TestFailure represents a failed test.
	TestFailure TestStatus = "FAIL"

	// TestAborted represents an aborted test (likely a timeout).
	TestAborted TestStatus = "ABORT"

	// TestSkipped represents a skipped test.
	TestSkipped TestStatus = "SKIP"

	// TestInfraFailure means that the test failed because of infra related issue.
	TestInfraFailure TestStatus = "INFRA_FAIL"

	// TestExonerated represents an exonerated test.
	TestExonerated TestStatus = "EXONERATED"
)

// IsFailure returns whether a test result corresponds to any failure condition
// (failure, timeout, etc.).
func IsFailure(tr TestStatus) bool {
	return tr != TestSuccess && tr != TestSkipped
}

// TestSummary is a summary of a suite of test runs. It represents the output
// file format of a runtests invocation.
type TestSummary struct {
	// Tests is a list of the details of the test runs.
	Tests []TestDetails `json:"tests"`

	// Outputs gives the suite-wide outputs, mapping canonical name of the
	// output to its path.
	Outputs map[string]string `json:"outputs,omitempty"`
}

// DataSink is a data sink exported by the test.
type DataSink struct {
	// Name is the name of the sink.
	Name string `json:"name"`

	// File is the file containing the sink data.
	File string `json:"file"`
}

// DataSinkMap is mapping from a data sink name to a list of the corresponding
// data sink files.
type DataSinkMap map[string][]DataSink

// TestDetails contains the details of a test run.
type TestDetails struct {
	TestResult

	// Name is the name of the test.
	Name string `json:"name"`

	// GNLabel is label of the test target (with toolchain).
	GNLabel string `json:"gn_label"`

	// SourceLabel is the build-system agnostic (either GN or Bazel) label corresponding to the test.
	SourceLabel string `json:"source_label"`

	// Status is the status of the test.
	Status TestStatus `json:"result"`

	// DataSinks gives the data sinks attached to a test.
	DataSinks DataSinkMap `json:"data_sinks,omitempty"`

	// StartTime is the UTC time when the test was started.
	StartTime time.Time `json:"start_time"`

	// EndTime is the UTC time when the test completed. Used to calculate the DurationMillis.
	EndTime time.Time `json:"-"`

	// Duration is how long the test execution took.
	DurationMillis int64 `json:"duration_milliseconds"`

	// IsTestingFailureMode is true iff this test was produced by tefmocheck.
	IsTestingFailureMode bool `json:"is_testing_failure_mode"`

	// Affected indicates whether the test is affected by the change under test.
	// It will only be set for tests running within tryjobs.
	Affected bool `json:"affected"`

	// Tags contain test metadata.
	Tags []build.TestTag `json:"tags"`

	// Test metadata
	Metadata metadata.TestMetadata `json:"metadata,omitempty"`

	// RunIndex is the index of this test run among all the runs of the same test.
	RunIndex int `json:"-"`

	// The combined stdout and stderr from this test.
	Stdio []byte `json:"-"`

	IsMultipliedRun bool `json:"-"`
}

// Passed indicates whether the test completed successfully. This will be false
// if the test timed out or failed.
func (r TestDetails) Passed() bool {
	return r.Status == TestSuccess
}

func (r TestDetails) Duration() time.Duration {
	return r.EndTime.Sub(r.StartTime)
}

// TestResult contains the details of a test run. A test can structure its results
// in this format and write it to TEST_OUTPUT_SUMMARY_PATH.
type TestResult struct {
	// OutputFiles are paths to the test's output files relative to the OutputDir.
	OutputFiles []string `json:"output_files"`

	// OutputDir is the common dir that all the OutputFiles should be located in.
	OutputDir string `json:"output_dir"`

	// Cases is individual test case results.
	Cases []TestCaseResult `json:"cases"`

	// FailureReason is an optional structured error message or explanation of the failure.
	// This will be ignored if the test status is a success. Host tests must exit with
	// a non-zero exit code to be considered a failure.
	FailureReason *FailureReason `json:"failure_reason,omitempty"`

	// SetupSucceeded indicates whether test setup completed successfully.
	SetupSucceeded *bool `json:"setup_succeeded,omitempty"`

	// TeardownSucceeded indicates whether test teardown completed successfully.
	TeardownSucceeded *bool `json:"teardown_succeeded,omitempty"`

	// ExitCode is the process exit code of the test binary.
	ExitCode *int `json:"exit_code,omitempty"`
}

// FailureReasonError represents a problem that caused a test to fail, such as a crash
// or expectation failure.
// Mirrors third_party/luci-go/resultdb/proto/v1/FailureReason_Error.
type FailureReasonError struct {
	Message string `json:"message,omitempty"`
	Trace   string `json:"trace,omitempty"`
}

// Provides structured information about why a test failed. This information is helpful
// for developers debugging failures and is also used by systems like LUCI Analysis
// to cluster similar failures together.
// It typically contains one or more error messages and potentially stack traces.
// Note: The total combined size of all errors within this message (as measured
// by proto.Size()) must not exceed 16,384 bytes.
// Mirrors third_party/luci-go/resultdb/proto/v1/FailureReason.
type FailureReason struct {
	Errors               []*FailureReasonError `json:"errors,omitempty"`
	TruncatedErrorsCount int32                 `json:"truncated_errors_count,omitempty"`
}

// FailureReasonFromMessage does not truncate message length so that full diagnostic output
// is preserved in summary.json for debugging. Downstream consumers and uploaders (such as resultdb/lib.go)
// are responsible for truncating error messages to meet specific RPC payload constraints.
func FailureReasonFromMessage(message string) *FailureReason {
	msg := strings.TrimSpace(message)
	if msg == "" {
		return nil
	}
	return &FailureReason{
		Errors: []*FailureReasonError{
			{Message: msg},
		},
	}
}

// TestCaseResult contains the details of a single test case, nested within a
// top-level TestDetails.
type TestCaseResult struct {
	DisplayName string        `json:"display_name"`
	SuiteName   string        `json:"suite_name"`
	CaseName    string        `json:"case_name"`
	Status      TestStatus    `json:"status"`
	Duration    time.Duration `json:"duration_nanos"`
	// Format is the test runner used to execute the test.
	Format string `json:"format"`
	// FailureReason is structured information about why a test case failed, including repeatable errors.
	FailureReason *FailureReason `json:"failure_reason,omitempty"`
	OutputFiles   []string       `json:"output_files,omitempty"`
	// The directory where the OutputFiles live if given as relative paths.
	OutputDir string `json:"output_dir,omitempty"`
	// Tags contain test case metadata.
	Tags []build.TestTag `json:"tags"`
}
