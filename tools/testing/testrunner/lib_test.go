// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testrunner

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"

	"go.fuchsia.dev/fuchsia/tools/botanist"
	botanistconstants "go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/botanist/targets"
	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder"
	"go.fuchsia.dev/fuchsia/tools/lib/clock"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
	"go.fuchsia.dev/fuchsia/tools/testing/tap"
	"go.fuchsia.dev/fuchsia/tools/testing/testrunner/constants"
)

const (
	testFunc        = "Test"
	copySinksFunc   = "EnsureSinks"
	runSnapshotFunc = "RunSnapshot"
	closeFunc       = "Close"
)

type fakeTester struct {
	runTest     func(context.Context, testsharder.Test, io.Writer, io.Writer, string) (*runtests.TestDetails, error)
	reconnect   func(context.Context, testsharder.Test) error
	setupTest   func(context.Context, testsharder.Test) error
	funcCalls   []string
	outDirs     map[string]bool
	lastTestRun testsharder.Test
}

func testResult(test testsharder.Test, result runtests.TestStatus) *runtests.TestDetails {
	return &runtests.TestDetails{
		Name:    test.Name,
		GNLabel: test.Label,
		Status:  result,
	}
}

func (t *fakeTester) SetupTest(ctx context.Context, test testsharder.Test) error {
	if t.setupTest != nil {
		return t.setupTest(ctx, test)
	}
	return nil
}

func (t *fakeTester) Test(ctx context.Context, test testsharder.Test, stdout, stderr io.Writer, outDir string) (*runtests.TestDetails, error) {
	t.funcCalls = append(t.funcCalls, testFunc)
	if t.outDirs == nil {
		t.outDirs = make(map[string]bool)
	}
	t.outDirs[outDir] = true
	result := runtests.TestSuccess
	t.lastTestRun = test
	if t.runTest != nil {
		return t.runTest(ctx, test, stdout, stderr, outDir)
	}

	return testResult(test, result), nil
}

func (t *fakeTester) ProcessResult(ctx context.Context, test testsharder.Test, outDir string, testResult *runtests.TestDetails, err error) (*runtests.TestDetails, error) {
	return testResult, err
}

func (t *fakeTester) Close() error {
	t.funcCalls = append(t.funcCalls, closeFunc)
	return nil
}

func (t *fakeTester) EnsureSinks(_ context.Context, _ []runtests.DataSinkMap, _ *TestOutputs) error {
	t.funcCalls = append(t.funcCalls, copySinksFunc)
	return nil
}

func (t *fakeTester) RunSnapshot(_ context.Context, _ string) error {
	t.funcCalls = append(t.funcCalls, runSnapshotFunc)
	return nil
}

func (t *fakeTester) Reconnect(ctx context.Context) error {
	if t.reconnect != nil {
		return t.reconnect(ctx, t.lastTestRun)
	}
	return nil
}

func TestValidateTest(t *testing.T) {
	cases := []struct {
		name      string
		test      testsharder.Test
		expectErr bool
	}{
		{
			name: "missing name",
			test: testsharder.Test{
				Test: build.Test{
					OS:   "linux",
					Path: "/foo/bar",
				},
				Runs: 1,
			},
			expectErr: true,
		},
		{
			name: "missing OS",
			test: testsharder.Test{
				Test: build.Test{
					Name: "test1",
					Path: "/foo/bar",
				},
				Runs: 1,
			},
			expectErr: true,
		},
		{
			name: "spurious package URL",
			test: testsharder.Test{
				Test: build.Test{
					Name:       "test1",
					OS:         "linux",
					PackageURL: "fuchsia-pkg://test1",
				},
				Runs: 1,
			},
			expectErr: true,
		},
		{
			name: "missing required path",
			test: testsharder.Test{
				Test: build.Test{
					Name: "test1",
					OS:   "linux",
				},
				Runs: 1,
			},
			expectErr: true,
		},
		{
			name: "missing required package_url or path",
			test: testsharder.Test{
				Test: build.Test{
					Name: "test1",
					OS:   "fuchsia",
				},
				Runs: 1,
			},
			expectErr: true,
		},
		{
			name: "missing runs",
			test: testsharder.Test{
				Test: build.Test{
					Name: "test1",
					OS:   "linux",
					Path: "/foo/bar",
				},
			},
			expectErr: true,
		},
		{
			name: "missing run algorithm",
			test: testsharder.Test{
				Test: build.Test{
					Name: "test1",
					OS:   "linux",
					Path: "/foo/bar",
				},
				Runs: 2,
			},
			expectErr: true,
		},
		{
			name: "valid test with path",
			test: testsharder.Test{
				Test: build.Test{
					Name: "test1",
					OS:   "linux",
					Path: "/foo/bar",
				},
				Runs: 1,
			},
			expectErr: false,
		},
		{
			name: "valid test with packageurl",
			test: testsharder.Test{
				Test: build.Test{
					Name:       "test1",
					OS:         "fuchsia",
					PackageURL: "fuchsia-pkg://test1",
				},
				Runs:         5,
				RunAlgorithm: testsharder.KeepGoing,
			},
			expectErr: false,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			err := validateTest(c.test)
			if c.expectErr != (err != nil) {
				t.Errorf("got error: %q, expectErr: %t", err, c.expectErr)
			}
		})
	}
}

func stdioPath(testName string, runIndex int) string {
	return filepath.Join(testOutDir(testName, runIndex), runtests.TestOutputFilename)
}

func testOutDir(testName string, runIndex int) string {
	return filepath.Join(testName, strconv.Itoa(runIndex))
}

func testDetails(name string, runIndex int, duration time.Duration, result runtests.TestStatus) runtests.TestDetails {
	return runtests.TestDetails{
		Name:   name,
		Status: result,
		TestResult: runtests.TestResult{
			OutputFiles: []string{runtests.TestOutputFilename},
			OutputDir:   testOutDir(name, runIndex),
		},
		DurationMillis: duration.Milliseconds(),
	}
}

func succeededTest(name string, runIndex int, duration time.Duration) runtests.TestDetails {
	return testDetails(name, runIndex, duration, runtests.TestSuccess)
}

func failedTest(name string, runIndex int, duration time.Duration) runtests.TestDetails {
	return testDetails(name, runIndex, duration, runtests.TestFailure)
}

func timedOutTest(name string, runIndex int, duration time.Duration) runtests.TestDetails {
	return testDetails(name, runIndex, duration, runtests.TestAborted)
}

func TestFilterTests(t *testing.T) {
	testCases := []struct {
		name                string
		tests               []testsharder.Test
		testAllowlistLength string
		testAllowlist       []string
		expectedResults     []testsharder.Test
		expectErr           bool
	}{
		{
			name:            "empty tests",
			tests:           []testsharder.Test{},
			expectedResults: []testsharder.Test{},
			expectErr:       false,
		},
		{
			name: "no test allowlist",
			tests: []testsharder.Test{
				{
					Test: build.Test{
						Name: "test1",
					},
				},
			},
			expectedResults: []testsharder.Test{
				{
					Test: build.Test{
						Name: "test1",
					},
				},
			},
		},
		{
			name: "valid test allowlist",
			tests: []testsharder.Test{
				{
					Test: build.Test{Name: "foo"},
				},
				{
					Test: build.Test{Name: "bar"},
				},
				{
					Test: build.Test{Name: "baz"},
				},
				{
					Test: build.Test{Name: "quux"},
				},
			},
			testAllowlistLength: "2",
			testAllowlist:       []string{"bar", "quux"},
			expectedResults: []testsharder.Test{
				{
					Test: build.Test{Name: "bar"},
				},
				{
					Test: build.Test{Name: "quux"},
				},
			},
		},
		{
			name: "test allowlist length is not an integer",
			tests: []testsharder.Test{
				{
					Test: build.Test{Name: "foo"},
				},
			},
			testAllowlistLength: "not an integer",
			testAllowlist:       []string{"bar", "quux"},
			expectErr:           true,
		},
		{
			name: "test allowlist length mismatch",
			tests: []testsharder.Test{
				{
					Test: build.Test{Name: "foo"},
				},
			},
			testAllowlistLength: "2",
			testAllowlist:       []string{"bar"},
			expectErr:           true,
		},
		{
			name: "test allowlist entry is an empty string",
			tests: []testsharder.Test{
				{
					Test: build.Test{Name: "foo"},
				},
			},
			testAllowlistLength: "1",
			testAllowlist:       []string{""},
			expectErr:           true,
		},
		{
			name: "test allowlist entry is not found in the test list",
			tests: []testsharder.Test{
				{
					Test: build.Test{Name: "foo"},
				},
			},
			testAllowlistLength: "1",
			testAllowlist:       []string{"bar"},
			expectErr:           true,
		},
	}
	for _, c := range testCases {
		t.Run(c.name, func(t *testing.T) {
			// Set the environment variables for the test.
			if c.testAllowlistLength != "" {
				t.Setenv(botanistconstants.TestAllowlistLengthEnvKey, c.testAllowlistLength)
			}
			for i, testName := range c.testAllowlist {
				t.Setenv(fmt.Sprintf(botanistconstants.TestAllowlistIndexEnvKeyTemplate, i), testName)
			}
			filteredTests, err := filterTests(c.tests)
			if c.expectErr != (err != nil) {
				t.Errorf("got error: %q, expectErr: %t", err, c.expectErr)
			}
			if !c.expectErr {
				if diff := cmp.Diff(c.expectedResults, filteredTests); diff != "" {
					t.Errorf("Wrong tests (-want +got):\n%s", diff)
				}
			}
		})
	}
}

func multipliedRun(details runtests.TestDetails) runtests.TestDetails {
	details.Tags = []build.TestTag{{Key: "is_multiplied_run", Value: "true"}}
	return details
}

func TestRunAndOutputTests(t *testing.T) {
	defaultDuration := time.Second
	perTestTimeout := 3 * time.Minute
	// A duration that slightly exceeds the total allowed runtime for each test.
	tooLong := perTestTimeout + testTimeoutGracePeriod + time.Second

	// Defines how the fake tester's Test() method should behave when running a
	// specified test.
	type testBehavior struct {
		// Whether running the test should return a (non-fatal) error.
		fail bool
		// Whether the test should hang indefinitely. Takes precedent over `fail`.
		hang bool
		// Whether the test should emit a fatal error.
		fatal bool
		// Whether the test should return a connection error.
		connErr bool
		// Whether the tester should fail to reconnect.
		failReconnect bool
		// The duration that the test will take to run.
		duration time.Duration
		// Data that the test should emit to stdout and stderr.
		stdout, stderr string
		// A file to write to the test out dir.
		outputfile string
		// Whether package resolution failed.
		pkgResolveFailed bool
		// The duration that package resolution will take.
		pkgResolutionDuration time.Duration
	}

	testCases := []struct {
		name string
		// The input tests for testrunner to run.
		tests []testsharder.Test
		// How the fake tester should behave when running each test, where keys
		// are of the form "test_name/run_index". Default is for the test to
		// pass with a duration of `defaultDuration`.
		behavior map[string]testBehavior
		// Don't impose a per-test timeout when running these tests.
		noTimeout bool
		// The test results that should be output.
		expectedResults []runtests.TestDetails
		// Mapping from relative filepath within the results dir to expected contents.
		expectedOutputs map[string]string
		// The error value that the function should return, as determined by errors.Is().
		wantErr bool
	}{
		{
			name: "no tests",
		},
		{
			name: "passed test",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			expectedResults: []runtests.TestDetails{
				succeededTest("foo", 0, defaultDuration),
			},
		},
		{
			name: "failed test",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {fail: true},
			},
			expectedResults: []runtests.TestDetails{
				failedTest("foo", 0, defaultDuration),
			},
		},
		{
			name: "timed out test",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {duration: tooLong},
			},
			expectedResults: []runtests.TestDetails{
				timedOutTest("foo", 0, tooLong),
			},
		},
		{
			name: "passed test with separate package resolution timeout",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo", PackageResolutionTimeoutSecs: 60},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {pkgResolutionDuration: tooLong},
			},
			expectedResults: []runtests.TestDetails{
				succeededTest("foo", 0, tooLong+defaultDuration),
			},
		},
		{
			name: "timed out test with separate package resolution timeout",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo", PackageResolutionTimeoutSecs: 60},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {duration: tooLong, pkgResolutionDuration: 60 * time.Second},
			},
			expectedResults: []runtests.TestDetails{
				timedOutTest("foo", 0, tooLong+60*time.Second),
			},
		},
		{
			name: "package resolution failed",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo", PackageResolutionTimeoutSecs: 60},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {pkgResolutionDuration: 60 * time.Second, pkgResolveFailed: true},
			},
			expectedResults: []runtests.TestDetails{
				func() runtests.TestDetails {
					d := failedTest("foo", 0, 60*time.Second)
					d.FailureReason = runtests.FailureReasonFromMessage("failed to setup test: package resolution failed")
					return d
				}(),
			},
		},
		{
			name: "many tests",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
				{
					Test:         build.Test{Name: "bar"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
				{
					Test:         build.Test{Name: "baz"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
				{
					Test:         build.Test{Name: "quux"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			expectedResults: []runtests.TestDetails{
				succeededTest("foo", 0, defaultDuration),
				succeededTest("bar", 0, defaultDuration),
				succeededTest("baz", 0, defaultDuration),
				succeededTest("quux", 0, defaultDuration),
			},
		},
		{
			name:      "no timeout set",
			noTimeout: true,
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
					Timeout:      -1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {duration: tooLong},
			},
			expectedResults: []runtests.TestDetails{
				// As long as they complete successfully within a finite
				// duration, tests should never be considered failures if
				// there's no timeout set.
				succeededTest("foo", 0, tooLong),
			},
		},
		{
			name: "stop on success",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					// The test should stop as soon as it passes, even if it has
					// not yet reached `runs`.
					Runs: 10,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {fail: true},
				"foo/1": {fail: true},
			},
			expectedResults: []runtests.TestDetails{
				failedTest("foo", 0, defaultDuration),
				failedTest("foo", 1, defaultDuration),
				succeededTest("foo", 2, defaultDuration),
			},
		},
		{
			name: "stop on failure",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					// The test should stop as soon as it fails, even if is has
					// not yet reached `runs`.
					Runs: 10,
				},
			},
			behavior: map[string]testBehavior{
				"foo/5": {fail: true},
			},
			expectedResults: []runtests.TestDetails{
				succeededTest("foo", 0, defaultDuration),
				multipliedRun(succeededTest("foo", 1, defaultDuration)),
				multipliedRun(succeededTest("foo", 2, defaultDuration)),
				multipliedRun(succeededTest("foo", 3, defaultDuration)),
				multipliedRun(succeededTest("foo", 4, defaultDuration)),
				failedTest("foo", 5, defaultDuration),
			},
		},
		{
			name: "stop on success, runs exceeded",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {fail: true},
				"foo/1": {fail: true},
				// Throw in a timeout too just to be sure it's handled the same
				// as non-timeout failures.
				"foo/2": {duration: tooLong},
				"foo/3": {fail: true},
				"foo/4": {fail: true},
			},
			expectedResults: []runtests.TestDetails{
				failedTest("foo", 0, defaultDuration),
				failedTest("foo", 1, defaultDuration),
				timedOutTest("foo", 2, tooLong),
				failedTest("foo", 3, defaultDuration),
				failedTest("foo", 4, defaultDuration),
			},
		},
		{
			name: "stop on failure, runs exceeded",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         5,
				},
			},
			expectedResults: []runtests.TestDetails{
				succeededTest("foo", 0, defaultDuration),
				multipliedRun(succeededTest("foo", 1, defaultDuration)),
				multipliedRun(succeededTest("foo", 2, defaultDuration)),
				multipliedRun(succeededTest("foo", 3, defaultDuration)),
				multipliedRun(succeededTest("foo", 4, defaultDuration)),
			},
		},
		{
			name: "keep going with mixed passes and fails",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.KeepGoing,
					Runs:         5,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {fail: true},
				"foo/3": {fail: true},
			},
			expectedResults: []runtests.TestDetails{
				failedTest("foo", 0, defaultDuration),
				succeededTest("foo", 1, defaultDuration),
				multipliedRun(succeededTest("foo", 2, defaultDuration)),
				failedTest("foo", 3, defaultDuration),
				multipliedRun(succeededTest("foo", 4, defaultDuration)),
			},
		},
		{
			name: "multiple tests fail",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         1,
				},
				{
					Test:         build.Test{Name: "bar"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {fail: true},
				"bar/0": {fail: true},
			},
			expectedResults: []runtests.TestDetails{
				failedTest("foo", 0, defaultDuration),
				failedTest("bar", 0, defaultDuration),
			},
		},
		{
			name: "multiple tests with one failing until the last attempt",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         10,
				},
				{
					Test:         build.Test{Name: "bar"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         10,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {fail: true},
				"foo/1": {fail: true},
				"foo/2": {fail: true},
			},
			expectedResults: []runtests.TestDetails{
				failedTest("foo", 0, defaultDuration),
				succeededTest("bar", 0, defaultDuration),
				failedTest("foo", 1, defaultDuration),
				failedTest("foo", 2, defaultDuration),
				succeededTest("foo", 3, defaultDuration),
			},
		},
		{
			name: "multiple tests with one timing out then passing",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         2,
				},
				{
					Test:         build.Test{Name: "bar"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         2,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {duration: tooLong},
			},
			expectedResults: []runtests.TestDetails{
				timedOutTest("foo", 0, tooLong),
				succeededTest("bar", 0, defaultDuration),
				succeededTest("foo", 1, defaultDuration),
			},
		},
		{
			name: "multiple tests fail with retries",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
				{
					Test:         build.Test{Name: "bar"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {fail: true},
				"foo/1": {fail: true},
				"bar/0": {fail: true},
				"bar/1": {fail: true},
				"bar/2": {fail: true},
				"bar/3": {fail: true},
			},
			expectedResults: []runtests.TestDetails{
				failedTest("foo", 0, defaultDuration),
				failedTest("bar", 0, defaultDuration),
				failedTest("foo", 1, defaultDuration),
				failedTest("bar", 1, defaultDuration),
				succeededTest("foo", 2, defaultDuration),
				failedTest("bar", 2, defaultDuration),
				failedTest("bar", 3, defaultDuration),
				succeededTest("bar", 4, defaultDuration),
			},
		},
		{
			name: "stop repeating after secs",
			tests: []testsharder.Test{
				{
					Test:                   build.Test{Name: "foo"},
					RunAlgorithm:           testsharder.StopOnFailure,
					StopRepeatingAfterSecs: int((5 * defaultDuration).Seconds()),
					Runs:                   1000,
				},
			},
			expectedResults: []runtests.TestDetails{
				succeededTest("foo", 0, defaultDuration),
				multipliedRun(succeededTest("foo", 1, defaultDuration)),
				multipliedRun(succeededTest("foo", 2, defaultDuration)),
				multipliedRun(succeededTest("foo", 3, defaultDuration)),
				multipliedRun(succeededTest("foo", 4, defaultDuration)),
			},
		},
		{
			name: "stop repeating early on failure",
			tests: []testsharder.Test{
				{
					Test:                   build.Test{Name: "foo"},
					RunAlgorithm:           testsharder.StopOnFailure,
					StopRepeatingAfterSecs: int((5 * defaultDuration).Seconds()),
					Runs:                   1000,
				},
			},
			behavior: map[string]testBehavior{
				"foo/1": {fail: true},
			},
			expectedResults: []runtests.TestDetails{
				succeededTest("foo", 0, defaultDuration),
				failedTest("foo", 1, defaultDuration),
			},
		},
		{
			name: "test hangs on first attempt",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
				{
					Test:         build.Test{Name: "bar"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {hang: true, duration: tooLong},
			},
			expectedResults: []runtests.TestDetails{
				timedOutTest("foo", 0, tooLong),
				succeededTest("bar", 0, defaultDuration),
				succeededTest("foo", 1, defaultDuration),
			},
		},
		{
			name: "retries on connection error",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {connErr: true},
				"foo/1": {connErr: true},
			},
			expectedResults: []runtests.TestDetails{
				succeededTest("foo", 2, defaultDuration),
			},
		},
		{
			name: "reconnect fails",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {connErr: true, failReconnect: true},
			},
			expectedResults: []runtests.TestDetails{},
			wantErr:         true,
		},
		{
			name: "reconnect succeeds, then fails",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {connErr: true},
				"foo/1": {connErr: true, failReconnect: true},
			},
			expectedResults: []runtests.TestDetails{},
			wantErr:         true,
		},
		{
			name: "fatal error running test",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
				{
					Test:         build.Test{Name: "bar"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         5,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {fail: true},
				"bar/1": {fatal: true},
			},
			// A fatal error should not be reported as a test failure since
			// fatal errors are generally not the fault of any one specific
			// test, but all previous results should still be reported.
			expectedResults: []runtests.TestDetails{
				failedTest("foo", 0, defaultDuration),
				succeededTest("bar", 0, defaultDuration),
				succeededTest("foo", 1, defaultDuration),
			},
			wantErr: true,
		},
		{
			name: "collects stdio",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
				{
					Test:         build.Test{Name: "bar"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {fail: true, stdout: "stdout0\n", stderr: "stderr0\n"},
				"foo/1": {duration: tooLong, stdout: "stdout1\n", stderr: "stderr1\n"},
				"foo/2": {stdout: "stdout2\n", stderr: "stderr2\n"},
				"bar/0": {stdout: "bar-stdout0\n", stderr: "bar-stderr0\n"},
			},
			expectedResults: []runtests.TestDetails{
				failedTest("foo", 0, defaultDuration),
				succeededTest("bar", 0, defaultDuration),
				timedOutTest("foo", 1, tooLong),
				succeededTest("foo", 2, defaultDuration),
			},
			expectedOutputs: map[string]string{
				// The fake tester writes to stdout before stderr, so stdout
				// always comes first.
				stdioPath("foo", 0): "stdout0\nstderr0\n",
				stdioPath("foo", 1): "stdout1\nstderr1\n",
				stdioPath("foo", 2): "stdout2\nstderr2\n",
				stdioPath("bar", 0): "bar-stdout0\nbar-stderr0\n",
			},
		},
		{
			name: "collects outputfiles",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
				{
					Test:         build.Test{Name: "bar"},
					RunAlgorithm: testsharder.StopOnSuccess,
					Runs:         5,
				},
			},
			behavior: map[string]testBehavior{
				"foo/0": {stdout: "stdout0\n", stderr: "stderr0\n", outputfile: "outputfile"},
				"bar/0": {stdout: "bar-stdout0\n", stderr: "bar-stderr0\n"},
			},
			expectedResults: []runtests.TestDetails{
				func() runtests.TestDetails {
					d := succeededTest("foo", 0, defaultDuration)
					// The output files found in the test out dir will
					// appear first in the list.
					d.OutputFiles = append([]string{"outputfile"}, d.OutputFiles...)
					return d
				}(),
				succeededTest("bar", 0, defaultDuration),
			},
			expectedOutputs: map[string]string{
				stdioPath("foo", 0): "stdout0\nstderr0\n",
				stdioPath("bar", 0): "bar-stdout0\nbar-stderr0\n",
				filepath.Join(testOutDir("foo", 0), "outputfile"): "outputs",
			},
		},
		{
			name: "affected test",
			tests: []testsharder.Test{
				{
					Test:         build.Test{Name: "foo"},
					RunAlgorithm: testsharder.StopOnFailure,
					Runs:         1,
					Affected:     true,
				},
			},
			expectedResults: []runtests.TestDetails{
				func() runtests.TestDetails {
					d := succeededTest("foo", 0, defaultDuration)
					d.Affected = true
					return d
				}(),
			},
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			fakeClock := clock.NewFakeClock()
			ctx := clock.NewContext(context.Background(), fakeClock)

			// Make sure the test data is realistic and could actually pass
			// validation; there's not much point in testing bogus inputs since
			// it would fail validation before testrunner even started running
			// any tests.
			for i, test := range tc.tests {
				if test.Timeout == 0 {
					test.Timeout = perTestTimeout
				}
				// These fields aren't important for the purpose of this
				// function so we don't require that they be set by each test
				// case.
				test.OS = "linux"
				test.Path = filepath.Join("path", "to", test.Name)
				if err := validateTest(test); err != nil {
					t.Fatal(err)
				}
				tc.tests[i] = test
			}

			timeout := perTestTimeout
			if tc.noTimeout {
				timeout = 0
			}

			runCounts := make(map[string]int)
			testerForTest := func(testsharder.Test) (Tester, *[]runtests.DataSinkMap, error) {
				return &fakeTester{runTest: func(ctx context.Context, test testsharder.Test, stdout, stderr io.Writer, outdir string) (*runtests.TestDetails, error) {
					runIndex := runCounts[test.Name]
					runCounts[test.Name]++
					behavior := tc.behavior[fmt.Sprintf("%s/%d", test.Name, runIndex)]

					stdout.Write([]byte(behavior.stdout))
					stderr.Write([]byte(behavior.stderr))

					result := testResult(test, runtests.TestSuccess)

					if behavior.outputfile != "" {
						os.MkdirAll(outdir, os.ModePerm)
						os.WriteFile(filepath.Join(outdir, behavior.outputfile), []byte("outputs"), os.ModePerm)
						result.OutputDir = outdir
						result.OutputFiles = []string{behavior.outputfile}
					}

					if behavior.duration == 0 {
						behavior.duration = defaultDuration
					}
					fakeClock.Advance(behavior.duration)

					if behavior.hang {
						// Block forever (well, technically just until the test
						// ends and the Cleanup callback executes, since we
						// don't want to leak goroutines).
						c := make(chan struct{})
						t.Cleanup(func() { close(c) })
						<-c
						result.Status = runtests.TestAborted
						return result, nil
					} else if behavior.fatal {
						result.Status = ""
						return result, fmt.Errorf("fatal error")
					} else if behavior.fail {
						result.Status = runtests.TestFailure
						return result, nil
					} else if behavior.connErr {
						result.Status = runtests.TestFailure
						return result, connectionError{fmt.Errorf("conn err")}
					}

					if timeout > 0 && behavior.duration >= timeout {
						// If the test is expected to time out, make sure to
						// wait until the context gets canceled before exiting
						// so we know that we've already hit the timeout
						// handler. Otherwise there's a race condition between
						// this function exiting and the timeout handler
						// triggering.
						<-ctx.Done()
					}
					return result, nil
				}, reconnect: func(ctx context.Context, test testsharder.Test) error {
					runIndex := runCounts[test.Name] - 1
					behavior := tc.behavior[fmt.Sprintf("%s/%d", test.Name, runIndex)]
					if behavior.failReconnect {
						return fmt.Errorf("reconnect failed")
					}
					return nil
				}, setupTest: func(ctx context.Context, test testsharder.Test) error {
					runIndex := runCounts[test.Name]
					behavior := tc.behavior[fmt.Sprintf("%s/%d", test.Name, runIndex)]
					if behavior.pkgResolutionDuration > 0 {
						fakeClock.Advance(behavior.pkgResolutionDuration)
					}
					if behavior.pkgResolveFailed {
						return fmt.Errorf("package resolution failed")
					}
					return nil
				}}, &[]runtests.DataSinkMap{}, nil
			}

			resultsDir := mkdtemp(t, "results")
			outputs, err := CreateTestOutputs(tap.NewProducer(io.Discard), resultsDir)
			if err != nil {
				t.Fatal(err)
			}

			origRetryBackoff := connectionErrorRetryBackoff
			defer func() {
				connectionErrorRetryBackoff = origRetryBackoff
			}()
			connectionErrorRetryBackoff = &retry.ZeroBackoff{}

			cleanup, err := runAndOutputTests(ctx, tc.tests, testerForTest, outputs, resultsDir, nil, nil, nil, botanist.Experiments{})
			cleanup()
			if tc.wantErr != (err != nil) {
				t.Errorf("want err: %t, got %s", tc.wantErr, err)
			}
			opts := cmp.Options{
				cmpopts.EquateEmpty(),
				cmpopts.IgnoreFields(runtests.TestDetails{}, "StartTime"),
			}
			if diff := cmp.Diff(tc.expectedResults, outputs.Summary.Tests, opts...); diff != "" {
				t.Errorf("test results diff (-want +got): %s", diff)
			}
			for path, want := range tc.expectedOutputs {
				got, err := os.ReadFile(filepath.Join(resultsDir, path))
				if err != nil {
					t.Errorf("Error reading expected output file %q: %s", path, err)
					continue
				}
				if diff := cmp.Diff(want, string(got)); diff != "" {
					t.Errorf("File contents diff (-want +got): %s", diff)
				}
			}
		})
	}
}

// mkdtemp creates a new temporary directory within t.TempDir.
func mkdtemp(t *testing.T, pattern string) string {
	t.Helper()
	dir, err := os.MkdirTemp(t.TempDir(), pattern)
	if err != nil {
		t.Fatal(err)
	}
	return dir
}

type fuchsiaTarget struct {
	targets.Emulator
	sshControlMasterRunning bool
}

func (t *fuchsiaTarget) SSHControlMasterRunning() bool {
	return t.sshControlMasterRunning
}

func (t *fuchsiaTarget) SetupSSHControlMaster(_ context.Context, _, _ string) (func(), error) {
	t.sshControlMasterRunning = true
	return func() {}, nil
}

func (t *fuchsiaTarget) ResolveIP() error {
	return nil
}

func (t *fuchsiaTarget) IPv4() (net.IP, error) {
	return nil, nil
}

func (t *fuchsiaTarget) IPv6() (*net.IPAddr, error) {
	return nil, nil
}

func TestExecute(t *testing.T) {
	tests := []testsharder.Test{
		{
			Test: build.Test{
				Name:       "bar",
				OS:         "fuchsia",
				PackageURL: "fuchsia-pkg://foo/bar.cm",
			},
			RunAlgorithm: testsharder.StopOnFailure,
			Runs:         2,
		}, {
			Test: build.Test{
				Name:       "baz",
				Path:       "/foo/baz",
				OS:         "fuchsia",
				PackageURL: "fuchsia-pkg://foo/baz.cm",
			},
			RunAlgorithm: testsharder.StopOnSuccess,
			Runs:         2,
		},
	}
	cases := []struct {
		name             string
		sshKeyFile       string
		serialSocketPath string
		wantErr          bool
		useFFX           bool
	}{
		{
			name:             "serial tester",
			serialSocketPath: "socketpath",
		},
		{
			name:             "nil serial tester",
			serialSocketPath: "socketpath",
			wantErr:          true,
		},
		{
			name:    "missing socket path",
			wantErr: true,
		},
		{
			name:       "use ffx",
			sshKeyFile: "sshkey",
			useFFX:     true,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			// Revert fakes.
			oldSerialTester := serialTester
			oldFFXInstance := ffxInstance
			defer func() {
				serialTester = oldSerialTester
				ffxInstance = oldFFXInstance
			}()
			fuchsiaTester := &fakeTester{}
			serialTester = func(_ context.Context, _ string) (Tester, error) {
				if c.wantErr {
					return nil, fmt.Errorf("failed to get tester")
				}
				return fuchsiaTester, nil
			}
			ffx := &ffxutil.MockFFXInstance{}
			ffxInstance = func(_ context.Context, _ *ffxutil.FFXInstance, _ botanist.Experiments, _ targets.FuchsiaTarget) (FFXInstance, error) {
				if c.useFFX {
					return ffx, nil
				}
				return nil, nil
			}

			var buf bytes.Buffer
			producer := tap.NewProducer(&buf)
			o, err := CreateTestOutputs(producer, mkdtemp(t, "outputs"))
			if err != nil {
				t.Fatal(err)
			}
			defer o.Close()
			target := &fuchsiaTarget{}
			testbedConfig := []any{
				targets.FuchsiaTestbedConfig{Nodename: "target1", IPv4: "0.0.0.0", IPv6: "::"},
				"non-fuchsia-target",
				targets.FuchsiaTestbedConfig{Nodename: targets.DefaultEmulatorNodename, IPv4: "0.0.0.0", IPv6: "::"},
			}
			oldReconnect := ffxTesterReconnect
			defer func() { ffxTesterReconnect = oldReconnect }()
			reconnect := false
			ffxTesterReconnect = func(t *FFXTester, ctx context.Context) error {
				if !reconnect {
					reconnect = true
					return fmt.Errorf("failed to reconnect")
				}
				return nil
			}
			expectedTestbedConfig := make([]any, 3)
			copy(expectedTestbedConfig, testbedConfig)
			testrunnerOpts := Options{SnapshotFile: "snapshot.zip", FuchsiaTarget: target, TestbedConfig: testbedConfig}
			if c.useFFX {
				if _, err := ffx.WriteRunResult(build.TestList{}, filepath.Join(o.OutDir, "early-boot-profiles")); err != nil {
					t.Errorf("failed to write early-boot profiles: %s", err)
				}
			}
			err = execute(context.Background(), tests, o, net.IPAddr{}, c.sshKeyFile, c.serialSocketPath, t.TempDir(), testrunnerOpts)
			if c.wantErr {
				if err == nil {
					t.Errorf("got nil error, want an error for failing to initialize a tester")
				}
				return
			}
			if err != nil {
				t.Errorf("got error: %v", err)
			}

			if c.sshKeyFile != "" && !target.SSHControlMasterRunning() {
				t.Errorf("ssh controlmaster not running when it should be")
			} else if c.sshKeyFile == "" && target.SSHControlMasterRunning() {
				t.Errorf("ssh controlmaster running when it shouldn't be")
			}
			if c.useFFX {
				// The fake target returns nil IP addresses so we should expect
				// the testbed config to have the addresses cleared for the emulator
				// target.
				expectedTestbedConfig[2] = targets.FuchsiaTestbedConfig{Nodename: targets.DefaultEmulatorNodename}
			}
			if diff := cmp.Diff(expectedTestbedConfig, testbedConfig); diff != "" {
				t.Errorf("Unexpected testbed config (-want +got):\n%s", diff)
			}

			funcCalls := strings.Join(fuchsiaTester.funcCalls, ",")
			testCount := strings.Count(funcCalls, testFunc)
			expectedTestCount := 3
			if c.useFFX {
				testCount = strings.Count(strings.Join(ffx.CmdsCalled, ","), "test")
			}
			copySinksCount := strings.Count(funcCalls, copySinksFunc)
			snapshotCount := strings.Count(funcCalls, runSnapshotFunc)
			expectedCopySinksCount := 1
			if c.useFFX {
				snapshotCount = strings.Count(strings.Join(ffx.CmdsCalled, ","), "snapshot")
				expectedCopySinksCount = 0
			}
			closeCount := strings.Count(funcCalls, closeFunc)
			expectedCloseCount := 1
			if c.useFFX {
				expectedCloseCount = 0
			}
			if testCount != expectedTestCount {
				t.Errorf("ran %d tests, want: %d", testCount, expectedTestCount)
			}
			if copySinksCount != expectedCopySinksCount {
				t.Errorf("ran CopySinks %d times, want: %d", copySinksCount, expectedCopySinksCount)
			}
			if snapshotCount != 1 {
				t.Errorf("ran RunSnapshot %d times, want: 1", snapshotCount)
			}
			if closeCount != expectedCloseCount {
				t.Errorf("ran Close %d times, want: %d", closeCount, expectedCloseCount)
			}
			// Ensure CopySinks, RunSnapshot, and Close are run after all calls to Test.
			if !c.useFFX {
				numLastCalls := 3
				lastCalls := fuchsiaTester.funcCalls[len(fuchsiaTester.funcCalls)-numLastCalls:]
				expectedLastCalls := []string{runSnapshotFunc, copySinksFunc, closeFunc}
				if diff := cmp.Diff(expectedLastCalls, lastCalls); diff != "" {
					t.Errorf("Unexpected command run (-want +got):\n%s", diff)
				}
			}
		})
	}
}

func TestScaleTestTimeout(t *testing.T) {
	testcases := []struct {
		name            string
		scaleFactor     string
		timeout         time.Duration
		expectedTimeout time.Duration
	}{
		{
			name:            "no scale factor",
			timeout:         time.Minute,
			expectedTimeout: time.Minute,
		}, {
			name:            "scale by 10",
			scaleFactor:     "10",
			timeout:         time.Minute,
			expectedTimeout: 10 * time.Minute,
		}, {
			name:            "invalid scale factor just returns orig timeout",
			scaleFactor:     "not a number",
			timeout:         time.Minute,
			expectedTimeout: time.Minute,
		},
	}

	for _, tc := range testcases {
		t.Run(tc.name, func(t *testing.T) {
			origScaleFactor := os.Getenv(constants.TestTimeoutScaleFactor)
			defer func() {
				if err := os.Setenv(constants.TestTimeoutScaleFactor, origScaleFactor); err != nil {
					t.Logf("failed to reset %s to %s", constants.TestTimeoutScaleFactor, origScaleFactor)
				}
			}()
			os.Setenv(constants.TestTimeoutScaleFactor, tc.scaleFactor)
			got := ScaleTestTimeout(tc.timeout)
			if got != tc.expectedTimeout {
				t.Errorf("got: %v, want: %v", got, tc.expectedTimeout)
			}
		})
	}
}
