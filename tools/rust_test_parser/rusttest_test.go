// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package rust_test_parser

import (
	"testing"

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
	testCaseCmp(t, "", []runtests.TestCaseResult{})
}

// If no test cases can be parsed, the output should be an empty slice, not a
// nil slice, so it gets serialized as an empty JSON array instead of as null.
func TestParseNoTestCases(t *testing.T) {
	testCaseCmp(t, "non-test output", []runtests.TestCaseResult{})
}

func TestParseRust(t *testing.T) {
	stdout := `
running 3 tests
test tests::ignored_test ... ignored
[stdout - legacy_test]
test tests::test_add_hundred ... ok
[stdout - legacy_test]
test tests::test_add ... FAILED
[stdout - legacy_test]
test tests::test_substract ... FAILED
[stdout - legacy_test]
[stdout - legacy_test]
failures:
[stdout - legacy_test]
[stdout - legacy_test]
---- tests::test_add_hundred stdout ----
[stdout - legacy_test]
---- tests::test_add_hundred stderr ----
[stdout - legacy_test]
booooo I printed an error, but it doesn't count as fail reason
---- tests::test_add stdout ----
[stdout - legacy_test]
---- tests::test_add stderr ----
[stdout - legacy_test]
thread 'main' panicked at ../../src/lib/zircon/rust/src/channel.rs:761:9:
[stdout - legacy_test]
assertion failed: ` + "`(left != right)`" + `
[stdout - legacy_test]
  left: ` + "`ObjectType(PORT)`" + `,
[stdout - legacy_test]
  right: ` + "`ObjectType(PORT)`" + `
[stdout - legacy_test]
stack backtrace:
[stdout - legacy_test]
{{{reset}}}
[stdout - legacy_test]
{{{module:0x0::elf:cb02c721da2e5287}}}
[stdout - legacy_test]
{{{mmap:0x2de1be9a000:0x11a5c:load:0x0:r:0x0}}}
[stdout - legacy_test]
[stdout - legacy_test]
---- tests::test_substract stdout ----
[stdout - legacy_test]
---- tests::test_substract stderr ----
[stdout - legacy_test]
thread 'main' panicked at ../../src/lib/zircon/rust/src/channel.rs:783:9:
[stdout - legacy_test]
assertion failed: ` + "`(left != right)`" + `
[stdout - legacy_test]
  left: ` + "`Err((5, 0))`" + `,
[stdout - legacy_test]
  right: ` + "`Err((5, 0))`" + `
[stdout - legacy_test]
stack backtrace:
[stdout - legacy_test]
{{{reset}}}
[stdout - legacy_test]
{{{module:0x0::elf:cb02c721da2e5287}}}
[stdout - legacy_test]
{{{mmap:0x3441843f000:0x11a5c:load:0x0:r:0x0}}}
[stdout - legacy_test]
{{{mmap:0x34418451000:0x18f90:load:0x0:rx:0x12000}}}
[stdout - legacy_test]
failures:
[stdout - legacy_test]
    test tests::test_add
[stdout - legacy_test]
    test tests::test_substract
[stdout - legacy_test]
[stdout - legacy_test]
test result: FAILED. 1 passed; 2 failed; 1 ignored; 0 measured; 0 filtered out; finished in 5.30s
[stdout - legacy_test]
[FAILED]	legacy_test
Failed tests: legacy_test
0 out of 1 tests passed...
fuchsia-pkg://fuchsia.com/fuchsiatests#meta/some-tests.cm completed with result: FAILED
One or more test runs failed.`

	want := []runtests.TestCaseResult{
		{
			DisplayName: "tests::ignored_test",
			SuiteName:   "tests",
			CaseName:    "ignored_test",
			Status:      runtests.TestSkipped,
			Format:      "Rust",
		}, {
			DisplayName: "tests::test_add_hundred",
			SuiteName:   "tests",
			CaseName:    "test_add_hundred",
			Status:      runtests.TestSuccess,
			Format:      "Rust",
		}, {
			DisplayName:   "tests::test_add",
			SuiteName:     "tests",
			CaseName:      "test_add",
			Status:        runtests.TestFailure,
			Format:        "Rust",
			FailureReason: runtests.FailureReasonFromMessage("thread 'main' panicked at ../../src/lib/zircon/rust/src/channel.rs:761:9:\nassertion failed: `(left != right)`\n  left: `ObjectType(PORT)`,\n  right: `ObjectType(PORT)`"),
		}, {
			DisplayName:   "tests::test_substract",
			SuiteName:     "tests",
			CaseName:      "test_substract",
			Status:        runtests.TestFailure,
			Format:        "Rust",
			FailureReason: runtests.FailureReasonFromMessage("thread 'main' panicked at ../../src/lib/zircon/rust/src/channel.rs:783:9:\nassertion failed: `(left != right)`\n  left: `Err((5, 0))`,\n  right: `Err((5, 0))`"),
		},
	}
	testCaseCmp(t, stdout, want)
}

// Regression test for https://fxbug.dev/42129657
func TestFxb52363(t *testing.T) {
	stdout := `
Running test in realm: test_env_25300c08
running 4 tests
test listen_for_klog ... ok
test listen_for_syslog ... ok
test listen_for_klog_routed_stdio ... ok
test test_observer_stop_api ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
ok 61 fuchsia-pkg://fuchsia.com/archivist_integration_tests#meta/logs_integration_rust_tests.cm (1.04732004s)
`
	want := []runtests.TestCaseResult{
		{
			DisplayName: "listen_for_klog",
			CaseName:    "listen_for_klog",
			Status:      runtests.TestSuccess,
			Format:      "Rust",
		}, {
			DisplayName: "listen_for_syslog",
			CaseName:    "listen_for_syslog",
			Status:      runtests.TestSuccess,
			Format:      "Rust",
		}, {
			DisplayName: "listen_for_klog_routed_stdio",
			CaseName:    "listen_for_klog_routed_stdio",
			Status:      runtests.TestSuccess,
			Format:      "Rust",
		}, {
			DisplayName: "test_observer_stop_api",
			CaseName:    "test_observer_stop_api",
			Status:      runtests.TestSuccess,
			Format:      "Rust",
		},
	}
	testCaseCmp(t, stdout, want)
}
