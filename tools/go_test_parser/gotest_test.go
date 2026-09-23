// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package go_test_parser

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

func TestParseGo(t *testing.T) {
	stdout := `
2020/06/17 18:15:06.096179 testrunner DEBUG: starting: [host_x64/fake_tests --test.timeout 5m -test.v]
=== RUN   TestParseEmpty
--- PASS: TestParseEmpty (0.01s)
=== RUN   TestParseInvalid
--- PASS: TestParseInvalid (0.02s)
=== RUN   TestParseGoogleTest
    TestParseGoogleTest: experimental/users/shayba/testparser/testparser_test.go:15: Parse(invalid).Parse() = [{SynonymDictTest.IsInitializedEmpty Pass 4} {SynonymDictTest.ReadingEmptyFileReturnsFalse Pass 3} {SynonymDictTest.ReadingNonexistentFileReturnsFalse Pass 4} {SynonymDictTest.LoadDictionary Pass 4} {SynonymDictTest.GetSynonymsReturnsListOfWords Pass 4} {SynonymDictTest.GetSynonymsWhenNoSynonymsAreAvailable Pass 4} {SynonymDictTest.AllWordsAreSynonymsOfEachOther Pass 4} {SynonymDictTest.GetSynonymsReturnsListOfWordsWithStubs Fail 4} {SynonymDictTest.CompoundWordBug Skip 4}]; want []
--- FAIL: TestParseGoogleTest (3.00s)
=== RUN   TestFail
    TestFail: experimental/users/shayba/testparser/testparser_test.go:68: Oops!
--- FAIL: TestFail (0.00s)
=== RUN   TestSkip
    TestSkip: experimental/users/shayba/testparser/testparser_test.go:72: Huh?
--- SKIP: TestSkip (0.00s)
=== RUN   TestAdd
=== RUN   TestAdd/add_foo
=== RUN   TestAdd/add_bar
=== RUN   TestAdd/add_baz
--- PASS: TestAdd (0.00s)
    --- PASS: TestAdd/add_foo (0.00s)
    --- PASS: TestAdd/add_bar (0.00s)
		--- PASS: TestAdd/add_baz (0.00s)
ok 8 host_x64/fake_tests (4.378744489s)
`
	want := []runtests.TestCaseResult{
		{
			DisplayName: "TestParseEmpty",
			CaseName:    "TestParseEmpty",
			Status:      runtests.TestSuccess,
			Duration:    10000000,
			Format:      "Go",
		}, {
			DisplayName: "TestParseInvalid",
			CaseName:    "TestParseInvalid",
			Status:      runtests.TestSuccess,
			Duration:    20000000,
			Format:      "Go",
		}, {
			DisplayName:   "TestParseGoogleTest",
			CaseName:      "TestParseGoogleTest",
			Status:        runtests.TestFailure,
			FailureReason: runtests.FailureReasonFromMessage("TestParseGoogleTest: experimental/users/shayba/testparser/testparser_test.go:15: Parse(invalid).Parse() = [{SynonymDictTest.IsInitializedEmpty Pass 4} {SynonymDictTest.ReadingEmptyFileReturnsFalse Pass 3} {SynonymDictTest.ReadingNonexistentFileReturnsFalse Pass 4} {SynonymDictTest.LoadDictionary Pass 4} {SynonymDictTest.GetSynonymsReturnsListOfWords Pass 4} {SynonymDictTest.GetSynonymsWhenNoSynonymsAreAvailable Pass 4} {SynonymDictTest.AllWordsAreSynonymsOfEachOther Pass 4} {SynonymDictTest.GetSynonymsReturnsListOfWordsWithStubs Fail 4} {SynonymDictTest.CompoundWordBug Skip 4}]; want []"),
			Duration:      3000000000,
			Format:        "Go",
		}, {
			DisplayName:   "TestFail",
			CaseName:      "TestFail",
			Status:        runtests.TestFailure,
			FailureReason: runtests.FailureReasonFromMessage("TestFail: experimental/users/shayba/testparser/testparser_test.go:68: Oops!"),
			Format:        "Go",
		}, {
			DisplayName: "TestSkip",
			CaseName:    "TestSkip",
			Status:      runtests.TestSkipped,
			Format:      "Go",
		}, {
			DisplayName: "TestAdd",
			CaseName:    "TestAdd",
			Status:      runtests.TestSuccess,
			Format:      "Go",
		}, {
			DisplayName: "TestAdd/add_foo",
			SuiteName:   "TestAdd",
			CaseName:    "add_foo",
			Status:      runtests.TestSuccess,
			Format:      "Go",
		}, {
			DisplayName: "TestAdd/add_bar",
			SuiteName:   "TestAdd",
			CaseName:    "add_bar",
			Status:      runtests.TestSuccess,
			Format:      "Go",
		}, {
			DisplayName: "TestAdd/add_baz",
			SuiteName:   "TestAdd",
			CaseName:    "add_baz",
			Status:      runtests.TestSuccess,
			Format:      "Go",
		},
	}
	testCaseCmp(t, stdout, want)
}

func TestParseGoPanic(t *testing.T) {
	stdout := `
=== RUN   TestReboot
Running /tmp/qemu-distro868073415/bin/qemu-system-x86_64 [-initrd /usr/local/google/home/curtisgalloway/src/fuchsia/out/core.x64/fuchsia.zbi -kernel /usr/local/google/home/curtisgalloway/src/fuchsia/out/core.x64/host_x64/test_data/qemu/multiboot.bin -nographic -smp 4,threads=2 -trace enable=vm_state_notify -machine q35 -device isa-debug-exit,iobase=0xf4,iosize=0x04 -cpu Haswell,+smap,-check,-fsgsbase -m 8192 -net none -append kernel.serial=legacy kernel.entropy-mixin=1420bb81dc0396b37cc2d0aa31bb2785dadaf9473d0780ecee1751afb5867564 kernel.halt-on-panic=true]
Checking for QEMU boot...
9234@1592444858.702846:vm_state_notify running 1 reason 9
SeaBIOS (version rel-1.11.1-0-g0551a4be2c-prebuilt.qemu-project.org)
panic: test timed out after 1s

goroutine 52 [running]:
testing.(*M).startAlarm.func1()
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/testing/testing.go:1459 +0xdf
created by time.goFunc
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/time/sleep.go:168 +0x44

goroutine 1 [chan receive]:
testing.(*T).Run(0xc00014c120, 0x570c48, 0xa, 0x57a1a8, 0x47ebc6)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/testing/testing.go:1043 +0x37e
testing.runTests.func1(0xc00014c000)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/testing/testing.go:1284 +0x78
testing.tRunner(0xc00014c000, 0xc000088e10)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/testing/testing.go:991 +0xdc
testing.runTests(0xc000134020, 0x6896d0, 0x1, 0x1, 0x0)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/testing/testing.go:1282 +0x2a7
testing.(*M).Run(0xc000148000, 0x0)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/testing/testing.go:1199 +0x15f
main.main()
	_testmain.go:44 +0x135

goroutine 19 [IO wait]:
internal/poll.runtime_pollWait(0x7f1d4a735d88, 0x72, 0xffffffffffffffff)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/runtime/netpoll.go:203 +0x55
internal/poll.(*pollDesc).wait(0xc0004f6258, 0x72, 0xf01, 0xfee, 0xffffffffffffffff)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/internal/poll/fd_poll_runtime.go:87 +0x45
internal/poll.(*pollDesc).waitRead(...)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/internal/poll/fd_poll_runtime.go:92
internal/poll.(*FD).Read(0xc0004f6240, 0xc0005ca012, 0xfee, 0xfee, 0x0, 0x0, 0x0)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/internal/poll/fd_unix.go:169 +0x19b
os.(*File).read(...)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/os/file_unix.go:263
os.(*File).Read(0xc000010040, 0xc0005ca012, 0xfee, 0xfee, 0x1, 0x0, 0x0)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/os/file.go:116 +0x71
bufio.(*Reader).fill(0xc0004f63c0)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/bufio/bufio.go:100 +0x103
bufio.(*Reader).ReadSlice(0xc0004f63c0, 0x47040a, 0x689dc0, 0xc0005c4080, 0x1, 0xc00015bd40, 0x4b2ee1)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/bufio/bufio.go:359 +0x3d
bufio.(*Reader).ReadBytes(0xc0004f63c0, 0xa, 0x5734e6, 0x15, 0xffffffffffffffff, 0x46, 0x0)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/bufio/bufio.go:438 +0x7a
bufio.(*Reader).ReadString(...)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/bufio/bufio.go:475
go.fuchsia.dev/fuchsia/src/testing/qemu.(*Instance).checkForLogMessage(0xc000134460, 0xc0004f63c0, 0x5734e6, 0x15, 0x573893, 0x16)
	/usr/local/google/home/curtisgalloway/src/fuchsia/out/core.x64/host_x64/gen/gopaths/reboot_tests/src/go.fuchsia.dev/fuchsia/src/testing/qemu/qemu.go:497 +0x46
go.fuchsia.dev/fuchsia/src/testing/qemu.(*Instance).WaitForLogMessage(...)
	/usr/local/google/home/curtisgalloway/src/fuchsia/out/core.x64/host_x64/gen/gopaths/reboot_tests/src/go.fuchsia.dev/fuchsia/src/testing/qemu/qemu.go:432
go.fuchsia.dev/fuchsia/src/tests/reboot.TestReboot(0xc00014c120)
	/usr/local/google/home/curtisgalloway/src/fuchsia/out/core.x64/host_x64/gen/gopaths/reboot_tests/src/go.fuchsia.dev/fuchsia/src/tests/reboot/reboot_test.go:52 +0x36c
testing.tRunner(0xc00014c120, 0x57a1a8)
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/testing/testing.go:991 +0xdc
created by testing.(*T).Run
	/usr/local/google/home/curtisgalloway/src/fuchsia/prebuilt/third_party/go/linux-x64/src/testing/testing.go:1042 +0x357
	`
	want := []runtests.TestCaseResult{
		{
			DisplayName:   "TestReboot",
			CaseName:      "TestReboot",
			Status:        runtests.TestFailure,
			FailureReason: runtests.FailureReasonFromMessage("panic: test timed out after 1s"),
			Duration:      1000000000,
			Format:        "Go",
		},
	}
	testCaseCmp(t, stdout, want)
}
