// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package mobly_test_parser

import (
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"

	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

func testCaseCmp(t *testing.T, stdout string, want []runtests.TestCaseResult) {
	r := Parse([]byte(stdout))
	if diff := cmp.Diff(want, r, cmpopts.SortSlices(func(a, b runtests.TestCaseResult) bool { return a.DisplayName < b.DisplayName })); diff != "" {
		t.Errorf("Found mismatch in %s (-want +got):\n%s", stdout, diff)
	}
}

func TestParseEmpty(t *testing.T) {
	testCaseCmp(t, "", []runtests.TestCaseResult{
		{
			DisplayName:   "TestparserError",
			FailureReason: runtests.FailureReasonFromMessage("[TestparserError] Missing Mobly summary record - potental infra timeout."),
			SuiteName:     "Synthetic",
			CaseName:      "Synthetic",
			Status:        runtests.TestAborted,
			Format:        "Mobly",
		},
	})
}

func TestParseMoblyTest(t *testing.T) {
	stdout := `
Running [InfraDriver]
======== Mobly config content ========
MoblyParams:
  LogPath: /tmp
TestBeds:
- Controllers:
    FuchsiaDevice:
    - name: fuchsia-emulator
      transport: fuchsia-controller
  Name: InfraTestbed
  TestParams: {}

======================================

[=====MOBLY RESULTS=====]
---
Requested Tests:
- test_goodbye
- test_hello
- test_skipped
- test_error
Type: TestNameList
---
Begin Time: 1668122321142
Details: null
End Time: 1668122321143
Extra Errors: {}
Extras: null
Result: PASS
Retry Parent: null
Signature: test_goodbye-1668122321142
Stacktrace: null
Termination Signal Type: null
Test Class: GreetingsTest
Test Name: test_goodbye
Type: Record
UID: null
---
Begin Time: 1668122321143
Details: Real test failure
End Time: 1668122321149
Extra Errors: {}
Extras: null
Result: FAIL
Retry Parent: null
Signature: test_hello-1668122321143
Stacktrace: null
Termination Signal Type: TestFailure
Test Class: GreetingsTest
Test Name: test_hello
Type: Record
UID: null
---
Details: null
Extra Errors: {}
Extras: null
Result: SKIP
Retry Parent: null
Signature: test_skipped-1668122321143
Stacktrace: null
Termination Signal Type: null
Test Class: GreetingsTest
Test Name: test_skipped
Type: Record
UID: null
---
Begin Time: 1668122321143
Details: 'Some multi-line error:
    line-1,
    line-2'
End Time: 1668122321149
Extra Errors: {}
Extras: null
Result: ERROR
Retry Parent: null
Signature: test_error-1668122321143
Stacktrace: null
Termination Signal Type: FuchsiaDeviceError
Test Class: GreetingsTest
Test Name: test_error
Type: Record
UID: null
---
Error: 0
Executed: 2
Failed: 0
Passed: 2
Requested: 2
Skipped: 0
Type: Summary

`

	want := []runtests.TestCaseResult{
		{
			DisplayName: "GreetingsTest.test_goodbye",
			SuiteName:   "GreetingsTest",
			CaseName:    "test_goodbye",
			Status:      runtests.TestSuccess,
			Duration:    1 * time.Millisecond,
			Format:      "Mobly",
		},
		{
			DisplayName:   "GreetingsTest.test_hello",
			FailureReason: runtests.FailureReasonFromMessage("[TestFailure] Real test failure"),
			SuiteName:     "GreetingsTest",
			CaseName:      "test_hello",
			Status:        runtests.TestFailure,
			Duration:      6 * time.Millisecond,
			Format:        "Mobly",
		},
		{
			DisplayName: "GreetingsTest.test_skipped",
			SuiteName:   "GreetingsTest",
			CaseName:    "test_skipped",
			Status:      runtests.TestSkipped,
			Duration:    0,
			Format:      "Mobly",
		},
		{
			DisplayName:   "GreetingsTest.test_error",
			FailureReason: runtests.FailureReasonFromMessage("[FuchsiaDeviceError] Some multi-line error: line-1, line-2"),
			SuiteName:     "GreetingsTest",
			CaseName:      "test_error",
			Status:        runtests.TestFailure,
			Duration:      6 * time.Millisecond,
			Format:        "Mobly",
		},
	}

	testCaseCmp(t, stdout, want)
}

func TestParseMoblyTestSkipWithTerminationSignal(t *testing.T) {
	stdout := `
Running [InfraDriver]
======== Mobly config content ========
MoblyParams:
  LogPath: /tmp
TestBeds:
- Controllers:
    FuchsiaDevice:
    - name: fuchsia-emulator
      transport: fuchsia-controller
  Name: InfraTestbed
  TestParams: {}

======================================

[=====MOBLY RESULTS=====]
---
Requested Tests:
- test_skip_with_sig
Type: TestNameList
---
Begin Time: 1668122321142
Details: Earlier test failed
End Time: 1668122321143
Extra Errors: {}
Extras: null
Result: SKIP
Retry Parent: null
Signature: test_skip_with_sig-1668122321142
Stacktrace: null
Termination Signal Type: TestAbortAll
Test Class: GreetingsTest
Test Name: test_skip_with_sig
Type: Record
UID: null
---
Error: 0
Executed: 1
Failed: 0
Passed: 0
Requested: 1
Skipped: 1
Type: Summary

`

	want := []runtests.TestCaseResult{
		{
			DisplayName:   "GreetingsTest.test_skip_with_sig",
			FailureReason: runtests.FailureReasonFromMessage("[TestAbortAll] Earlier test failed"),
			SuiteName:     "GreetingsTest",
			CaseName:      "test_skip_with_sig",
			Status:        runtests.TestFailure,
			Duration:      1 * time.Millisecond,
			Format:        "Mobly",
		},
	}

	testCaseCmp(t, stdout, want)
}

func TestParseMoblyTestErrorNoSummary(t *testing.T) {
	stdout := `
Running [InfraDriver]
======== Mobly config content ========
MoblyParams:
  LogPath: /tmp
TestBeds:
- Controllers:
    FuchsiaDevice:
    - name: fuchsia-emulator
      transport: fuchsia-controller
  Name: InfraTestbed
  TestParams: {}

======================================

[=====MOBLY RESULTS=====]
---
Requested Tests:
- test_goodbye
- test_hello
- test_skipped
- test_error
Type: TestNameList
---
Begin Time: 1668122321142
Details: null
End Time: 1668122321143
Extra Errors: {}
Extras: null
Result: PASS
Retry Parent: null
Signature: test_goodbye-1668122321142
Stacktrace: null
Termination Signal Type: null
Test Class: GreetingsTest
Test Name: test_goodbye
Type: Record
UID: null

`

	want := []runtests.TestCaseResult{
		{
			DisplayName: "GreetingsTest.test_goodbye",
			SuiteName:   "GreetingsTest",
			CaseName:    "test_goodbye",
			Status:      runtests.TestSuccess,
			Duration:    1 * time.Millisecond,
			Format:      "Mobly",
		},
		{
			DisplayName:   "TestparserError",
			FailureReason: runtests.FailureReasonFromMessage("[TestparserError] Missing Mobly summary record - potental infra timeout."),
			SuiteName:     "Synthetic",
			CaseName:      "Synthetic",
			Status:        runtests.TestAborted,
			Format:        "Mobly",
		},
	}

	testCaseCmp(t, stdout, want)
}

func TestParseMoblyTestErrorMissingHeader(t *testing.T) {
	stdout := `
Running [InfraDriver]
======== Mobly config content ========
MoblyParams:
  LogPath: /tmp
TestBeds:
- Controllers:
    FuchsiaDevice:
    - name: fuchsia-emulator
      transport: fuchsia-controller
  Name: InfraTestbed
  TestParams: {}

======================================

`

	want := []runtests.TestCaseResult{
		{
			DisplayName:   "TestparserError",
			FailureReason: runtests.FailureReasonFromMessage("[TestparserError] Missing Mobly summary record - potental infra timeout."),
			SuiteName:     "Synthetic",
			CaseName:      "Synthetic",
			Status:        runtests.TestAborted,
			Format:        "Mobly",
		},
	}

	testCaseCmp(t, stdout, want)
}

func TestParseMoblyTestErrorMalformedYAML(t *testing.T) {
	stdout := `
Running [InfraDriver]
======== Mobly config content ========
MoblyParams:
  LogPath: /tmp
TestBeds:
- Controllers:
    FuchsiaDevice:
    - name: fuchsia-emulator
      transport: fuchsia-controller
  Name: InfraTestbed
  TestParams: {}

======================================

[=====MOBLY RESULTS=====]
---
Requested Tests:
- test_goodbye
  test_hello:
    malformed

`

	want := []runtests.TestCaseResult{
		{
			DisplayName:   "TestparserError",
			FailureReason: runtests.FailureReasonFromMessage("[TestparserError] Missing Mobly summary record - potental infra timeout."),
			SuiteName:     "Synthetic",
			CaseName:      "Synthetic",
			Status:        runtests.TestAborted,
			Format:        "Mobly",
		},
	}

	testCaseCmp(t, stdout, want)
}
