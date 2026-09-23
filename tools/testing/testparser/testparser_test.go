// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testparser

import (
	"bytes"
	"encoding/json"
	"testing"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

func compactJSON(jsonBytes []byte) []byte {
	buffer := bytes.NewBuffer([]byte{})
	json.Compact(buffer, jsonBytes)
	return buffer.Bytes()
}

func indentJSON(jsonBytes []byte) []byte {
	buffer := bytes.NewBuffer([]byte{})
	json.Indent(buffer, jsonBytes, "", "\t")
	return buffer.Bytes()
}

func testCase(t *testing.T, stdout string, want string) {
	t.Helper()
	r, _ := Parse([]byte(stdout))
	actual, _ := json.Marshal(r)
	if !bytes.Equal(actual, compactJSON([]byte(want))) {
		actualIndented := string(indentJSON(actual))
		wantIndented := string(indentJSON([]byte(want)))
		t.Errorf(
			"Parse(stdout) = `\n%v\n`; want `\n%v\n`",
			actualIndented,
			wantIndented,
		)
	}
}

func testCaseCmp(t *testing.T, stdout string, want []runtests.TestCaseResult) {
	r, _ := Parse([]byte(stdout))
	if diff := cmp.Diff(want, r, cmpopts.SortSlices(func(a, b runtests.TestCaseResult) bool { return a.DisplayName < b.DisplayName })); diff != "" {
		t.Errorf("Found mismatch in %s (-want +got):\n%s", stdout, diff)
	}
}

func TestParseEmpty(t *testing.T) {
	testCaseCmp(t, "", []runtests.TestCaseResult{})
}

func TestParseInvalid(t *testing.T) {
	stdout := `
Mary had a little lamb
Its fleece was white as snow
And everywhere that Mary went
The lamb was sure to go
`
	testCaseCmp(t, stdout, []runtests.TestCaseResult{})
}

// If no test cases can be parsed, the output should be an empty slice, not a
// nil slice, so it gets serialized as an empty JSON array instead of as null.
func TestParseNoTestCases(t *testing.T) {
	testCase(t, "non-test output", "[]")
}

func TestParseTrfTest(t *testing.T) {
	stdout := `
Running test 'fuchsia-pkg://fuchsia.com/f2fs-fs-tests#meta/f2fs-unittest.cm'
[RUNNING]	BCacheTest.Trim
[PASSED]	BCacheTest.Trim
[RUNNING]	BCacheTest.Exception
[00371.417610][942028][942030][<root>] ERROR: [src/storage/f2fs/bcache.cc(45)] Invalid device size
[00371.417732][942028][942030][<root>] ERROR: [src/storage/f2fs/bcache.cc(52)] Block count overflow
[PASSED]	BCacheTest.Exception
[RUNNING]	CheckpointTest.Version
[PASSED]	CheckpointTest.Version
[stdout - BCacheTest.Trim]
magic [0xf2f52010 : 4076150800]
[stdout - BCacheTest.Trim]
major_ver [0x1 : 1]
[stdout - BCacheTest.Trim]
minor_ver [0x0 : 0]
[RUNNING]	FormatFilesystemTest.MkfsOptionsLabel
[00402.723886][969765][969767][<root>] INFO: [mkfs.cc(894)] This device doesn't support TRIM
[00402.743120][969765][969767][<root>] INFO: [mkfs.cc(894)] This device doesn't support TRIM
[stderr - FormatFilesystemTest.MkfsOptionsLabel]
ERROR: label length should be less than 16.
[PASSED]	FormatFilesystemTest.MkfsOptionsLabel
[RUNNING]	NodeManagerTest.TruncateExceptionCase
[stderr - NodeManagerTest.TruncateExceptionCase]
Error reading test result:File(read call failed: A FIDL client's channel to the service fuchsia.io.File was closed: PEER_CLOSED
[stderr - NodeManagerTest.TruncateExceptionCase]
[stderr - NodeManagerTest.TruncateExceptionCase]
Caused by:
[stderr - NodeManagerTest.TruncateExceptionCase]
    0: A FIDL client's channel to the service fuchsia.io.File was closed: PEER_CLOSED
[stderr - NodeManagerTest.TruncateExceptionCase]
    1: PEER_CLOSED)
[FAILED]	NodeManagerTest.TruncateExceptionCase
[RUNNING]	VnodeTest.TruncateExceptionCase
[stderr - VnodeTest.TruncateExceptionCase]
Test exited abnormally
[FAILED]	VnodeTest.TruncateExceptionCase
Failed tests: NodeManagerTest.TruncateExceptionCase, VnodeTest.TruncateExceptionCase
[duration - virtualization::virtualization_netdevice::remove_network]:	Still running after 60 seconds
[TIMED_OUT]	virtualization::virtualization_netdevice::remove_network
100 out of 102 tests passed...
fuchsia-pkg://fuchsia.com/f2fs-fs-tests#meta/f2fs-unittest.cm completed with result: FAILED
One or more test runs failed.
`
	want := []runtests.TestCaseResult{
		{
			DisplayName: "BCacheTest.Trim",
			CaseName:    "BCacheTest.Trim",
			Status:      runtests.TestSuccess,
			Format:      "FTF",
		}, {
			DisplayName: "BCacheTest.Exception",
			CaseName:    "BCacheTest.Exception",
			Status:      runtests.TestSuccess,
			Format:      "FTF",
		}, {
			DisplayName: "CheckpointTest.Version",
			CaseName:    "CheckpointTest.Version",
			Status:      runtests.TestSuccess,
			Format:      "FTF",
		}, {
			DisplayName: "FormatFilesystemTest.MkfsOptionsLabel",
			CaseName:    "FormatFilesystemTest.MkfsOptionsLabel",
			Status:      runtests.TestSuccess,
			Format:      "FTF",
		}, {
			DisplayName: "NodeManagerTest.TruncateExceptionCase",
			CaseName:    "NodeManagerTest.TruncateExceptionCase",
			Status:      runtests.TestFailure,
			Format:      "FTF",
			FailureReason: runtests.FailureReasonFromMessage("Error reading test result:File(read call failed: A FIDL client's channel to the service fuchsia.io.File was closed: PEER_CLOSED\n" +
				"Caused by:\n" +
				"    0: A FIDL client's channel to the service fuchsia.io.File was closed: PEER_CLOSED"),
		}, {
			DisplayName:   "VnodeTest.TruncateExceptionCase",
			CaseName:      "VnodeTest.TruncateExceptionCase",
			Status:        runtests.TestFailure,
			Format:        "FTF",
			FailureReason: runtests.FailureReasonFromMessage("Test exited abnormally"),
		}, {
			DisplayName: "virtualization::virtualization_netdevice::remove_network",
			CaseName:    "virtualization::virtualization_netdevice::remove_network",
			Status:      runtests.TestAborted,
			Format:      "FTF",
		},
	}
	testCaseCmp(t, stdout, want)
}

func TestParseTrfTestWithExpectations(t *testing.T) {
	stdout := `
Running test 'fuchsia-pkg://fuchsia.com/starnix_gvisor_tests?hash=c7c79a3408c5c0c89f61beb738f2dd71ca1c724f8d9c9d4a72e52631c8cbaacb#meta/chroot_test.cm'
[RUNNING]	ChrootTest.ProcMemSelfMapsNoEscapeProcOpen
[SKIPPED]	ChrootTest.ProcMemSelfMapsNoEscapeProcOpen
[RUNNING]	ChrootTest.Success
[RUNNING]	ChrootTest.ProcMemSelfFdsNoEscapeProcOpen
[stdout - fuchsia-pkg://fuchsia.com/starnix_gvisor_tests?hash=c7c79a3408c5c0c89f61beb738f2dd71ca1c724f8d9c9d4a72e52631c8cbaacb#meta/chroot_test.cm]
Note: Google Test filter = ChrootTest.Success:ChrootTest.ProcMemSelfFdsNoEscapeProcOpen:
[==========] Running 2 tests from 1 test suite.
[----------] Global test environment set-up.
[----------] 2 tests from ChrootTest
[ RUN      ] ChrootTest.Success
[       OK ] ChrootTest.Success (4 ms)
[ RUN      ] ChrootTest.ProcMemSelfFdsNoEscapeProcOpen
third_party/gvisor/test/syscalls/linux/chroot.cc:306: Failure
Value of: InForkedProcess(rest)
Expected: is OK and has a value that is equal to 0
	Actual: No Error (of type N12cloud_gvisor7testing12PosixErrorOrIiEE), has a value 256

[  FAILED  ] ChrootTest.ProcMemSelfFdsNoEscapeProcOpen (17 ms)
[----------] 2 tests from ChrootTest (21 ms total)

[----------] Global test environment tear-down
[==========] 2 tests from 1 test suite ran. (21 ms total)
[  PASSED  ] 1 tests.
[  FAILED  ] 1 tests, listed below:
[  FAILED  ] ChrootTest.ProcMemSelfFdsNoEscapeProcOpen

	1 FAILED TESTS
[stderr - fuchsia-pkg://fuchsia.com/starnix_gvisor_tests?hash=c7c79a3408c5c0c89f61beb738f2dd71ca1c724f8d9c9d4a72e52631c8cbaacb#meta/chroot_test.cm]
Check failed: strcmp(buf, "/foo") == 0
libc: Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE) in tid 9 (chroot_test), pid 9 (<unknown>)
[01447.782299][expectation-comparer] INFO: ChrootTest.ProcMemSelfFdsNoEscapeProcOpen failure is expected, so it will be reported to the test runner as having passed.
[PASSED]	ChrootTest.Success
[01447.649121][expectation-comparer] INFO: ChrootTest.ProcMemSelfMapsNoEscapeProcOpen skip is expected.
[PASSED]	ChrootTest.ProcMemSelfFdsNoEscapeProcOpen
[01447.596892][runners:chroot_test.cm_Z3b2DH8][starnix] INFO: start_component: fuchsia-pkg://fuchsia.com/starnix_gvisor_tests?hash=c7c79a3408c5c0c89f61beb738f2dd71ca1c724f8d9c9d4a72e52631c8cbaacb#meta/chroot_test.cm
arguments: Some([])
manifest: Some(Dictionary { entries: Some([DictionaryEntry { key: "binary", value: Some(Str("data/tests/chroot_test")) }, DictionaryEntry { key: "environ", value: Some(StrVec(["TEST_TMPDIR=/data/tmp", "TEST_ON_GVISOR=1", "TEST_SRCDIR={pkg_path}/data/tests", "BENCHMARK_FORMAT=json", "BENCHMARK_OUT=/test_data/benchmark.json"])) }, DictionaryEntry { key: "user", value: Some(Str("fuchsia:x:0:0")) }, DictionaryEntry { key: "apex_hack", value: Some(StrVec(["com.android.runtime"])) }, DictionaryEntry { key: "features", value: Some(StrVec(["binder", "test_data"])) }, DictionaryEntry { key: "init", value: Some(StrVec([])) }, DictionaryEntry { key: "init_user", value: Some(Str("root:x:0:0")) }, DictionaryEntry { key: "kernel_cmdline", value: Some(Str("androidboot.hardware=starnix")) }, DictionaryEntry { key: "mounts", value: Some(StrVec(["/:ext4:data/system.img", "/vendor:ext4:data/testcases.img", "/data:remotefs:data", "/dev:devtmpfs", "/dev/shm:tmpfs", "/data/tmp:tmpfs", "/dev/shm:tmpfs", "/dev/pts:devpts", "/proc:proc", "/sys:sysfs", "/sys/fs/selinux:selinuxfs"])) }, DictionaryEntry { key: "name", value: Some(Str("gvisor_test")) }, DictionaryEntry { key: "startup_file_path", value: Some(Str("")) }, DictionaryEntry { key: "test_type", value: Some(Str("gunit")) }, DictionaryEntry { key: "args", value: Some(StrVec(["--gunit_list_tests", "--gunit_output=json:/test_data/test_result-b2be0d31-81ff-40c4-bdb9-0cfc9e93649d.json"])) }]), __source_breaking: _ })

[01447.598078][runners:chroot_test.cm_Z3b2DH8][starnix] INFO: start_component environment: ["TEST_TMPDIR=/data/tmp", "TEST_ON_GVISOR=1", "TEST_SRCDIR=/container/component/f169czoi4l/pkg/data/tests", "BENCHMARK_FORMAT=json", "BENCHMARK_OUT=/test_data/benchmark.json"]

[01447.641356][runners:chroot_test.cm_Z3b2DH8][stdio,starnix] INFO: ChrootTest.
	Success
	ProcMemSelfFdsNoEscapeProcOpen
	ProcMemSelfMapsNoEscapeProcOpen
[01447.780791][expectation-comparer] INFO: ChrootTest.Success success is expected.

2 out of 2 attempted tests passed, 1 tests skipped...
fuchsia-pkg://fuchsia.com/starnix_gvisor_tests?hash=c7c79a3408c5c0c89f61beb738f2dd71ca1c724f8d9c9d4a72e52631c8cbaacb#meta/chroot_test.cm completed with result: PASSED

PASS: 1 FAIL: 0 00:01 √  fuchsia-pkg://fuchsia.com/starnix_gvisor_tests#meta/chroot_test.cm
	`
	want := []runtests.TestCaseResult{
		{
			DisplayName: "ChrootTest.Success",
			CaseName:    "ChrootTest.Success",
			Status:      runtests.TestSuccess,
			Format:      "FTF",
			Tags: []build.TestTag{
				{
					Key:   "expectation",
					Value: "success",
				},
			},
		},
		{
			DisplayName: "ChrootTest.ProcMemSelfFdsNoEscapeProcOpen",
			CaseName:    "ChrootTest.ProcMemSelfFdsNoEscapeProcOpen",
			Status:      runtests.TestSuccess,
			Format:      "FTF",
			Tags: []build.TestTag{
				{
					Key:   "expectation",
					Value: "failure",
				},
			},
		},
		{
			DisplayName: "ChrootTest.ProcMemSelfMapsNoEscapeProcOpen",
			CaseName:    "ChrootTest.ProcMemSelfMapsNoEscapeProcOpen",
			Status:      runtests.TestSkipped,
			Format:      "FTF",
			Tags: []build.TestTag{
				{
					Key:   "expectation",
					Value: "skip",
				},
			},
		},
	}

	testCaseCmp(t, stdout, want)
}

func TestParseGoogleTest(t *testing.T) {
	stdout := `
Some times there is weird stuff in stdout.
[==========] Running 9 tests from 1 test suite.
[----------] Global test environment set-up.
[----------] 9 tests from SynonymDictTest
[ RUN      ] SynonymDictTest.IsInitializedEmpty
[       OK ] SynonymDictTest.IsInitializedEmpty (4 ms)
[ RUN      ] SynonymDictTest.ReadingEmptyFileReturnsFalse
[       OK ] SynonymDictTest.ReadingEmptyFileReturnsFalse (3 ms)
[ RUN      ] SynonymDictTest.ReadingNonexistentFileReturnsFalse
Some times tests print to stdout.
Their prints get interleaved with the results.
[       OK ] SynonymDictTest.ReadingNonexistentFileReturnsFalse (4 ms)
[ RUN      ] SynonymDictTest.LoadDictionary
[       OK ] SynonymDictTest.LoadDictionary (4 ms)
[ RUN      ] SynonymDictTest.GetSynonymsReturnsListOfWords
[       OK ] SynonymDictTest.GetSynonymsReturnsListOfWords (4 ms)
[ RUN      ] SynonymDictTest.GetSynonymsWhenNoSynonymsAreAvailable
[       OK ] SynonymDictTest.GetSynonymsWhenNoSynonymsAreAvailable (4 ms)
[ RUN      ] SynonymDictTest.AllWordsAreSynonymsOfEachOther
[       OK ] SynonymDictTest.AllWordsAreSynonymsOfEachOther (4 ms)
[ RUN      ] SynonymDictTest.GetSynonymsReturnsListOfWordsWithStubs
[  FAILED  ] SynonymDictTest.GetSynonymsReturnsListOfWordsWithStubs (4 ms)
[ RUN      ] SynonymDictTest.CompoundWordBug
[  SKIPPED ] SynonymDictTest.CompoundWordBug (4 ms)
[----------] 9 tests from SynonymDictTest (36 ms total)
[----------] Global test environment tear-down
[==========] 9 tests from 1 test suite ran. (38 ms total)
[  PASSED  ] 9 tests.
`
	want := []runtests.TestCaseResult{
		{
			DisplayName: "SynonymDictTest.IsInitializedEmpty",
			SuiteName:   "SynonymDictTest",
			CaseName:    "IsInitializedEmpty",
			Status:      runtests.TestSuccess,
			Duration:    4000000,
			Format:      "GoogleTest",
		}, {
			DisplayName: "SynonymDictTest.ReadingEmptyFileReturnsFalse",
			SuiteName:   "SynonymDictTest",
			CaseName:    "ReadingEmptyFileReturnsFalse",
			Status:      runtests.TestSuccess,
			Duration:    3000000,
			Format:      "GoogleTest",
		}, {
			DisplayName: "SynonymDictTest.ReadingNonexistentFileReturnsFalse",
			SuiteName:   "SynonymDictTest",
			CaseName:    "ReadingNonexistentFileReturnsFalse",
			Status:      runtests.TestSuccess,
			Duration:    4000000,
			Format:      "GoogleTest",
		}, {
			DisplayName: "SynonymDictTest.LoadDictionary",
			SuiteName:   "SynonymDictTest",
			CaseName:    "LoadDictionary",
			Status:      runtests.TestSuccess,
			Duration:    4000000,
			Format:      "GoogleTest",
		}, {
			DisplayName: "SynonymDictTest.GetSynonymsReturnsListOfWords",
			SuiteName:   "SynonymDictTest",
			CaseName:    "GetSynonymsReturnsListOfWords",
			Status:      runtests.TestSuccess,
			Duration:    4000000,
			Format:      "GoogleTest",
		}, {
			DisplayName: "SynonymDictTest.GetSynonymsWhenNoSynonymsAreAvailable",
			SuiteName:   "SynonymDictTest",
			CaseName:    "GetSynonymsWhenNoSynonymsAreAvailable",
			Status:      runtests.TestSuccess,
			Duration:    4000000,
			Format:      "GoogleTest",
		}, {
			DisplayName: "SynonymDictTest.AllWordsAreSynonymsOfEachOther",
			SuiteName:   "SynonymDictTest",
			CaseName:    "AllWordsAreSynonymsOfEachOther",
			Status:      runtests.TestSuccess,
			Duration:    4000000,
			Format:      "GoogleTest",
		}, {
			DisplayName: "SynonymDictTest.GetSynonymsReturnsListOfWordsWithStubs",
			SuiteName:   "SynonymDictTest",
			CaseName:    "GetSynonymsReturnsListOfWordsWithStubs",
			Status:      runtests.TestFailure,
			Duration:    4000000,
			Format:      "GoogleTest",
		}, {
			DisplayName: "SynonymDictTest.CompoundWordBug",
			SuiteName:   "SynonymDictTest",
			CaseName:    "CompoundWordBug",
			Status:      runtests.TestSkipped,
			Duration:    4000000,
			Format:      "GoogleTest",
		},
	}
	testCaseCmp(t, stdout, want)
}

func TestParseZircon(t *testing.T) {
	stdout := `
CASE minfs_truncate_tests                               [STARTED]
    TestTruncateSmall                                   [RUNNING] [PASSED] (1 ms)
    (TestTruncateLarge<1 << 10, 1000>)                  [RUNNING] [PASSED] (20414 ms)
    (TestTruncateLarge<1 << 15, 500>)                   [RUNNING] [PASSED] (10012 ms)
    (TestTruncateLarge<1 << 20, 500>)                   [RUNNING] [FAILED] (10973 ms)
    (TestTruncateLarge<1 << 25, 500>)                   [IGNORED]
CASE minfs_truncate_tests                               [PASSED]

CASE minfs_sparse_tests                                 [STARTED]
    (test_sparse<0, 0, kBlockSize>)                     [RUNNING] [PASSED] (19 ms)
    (test_sparse<kBlockSize / 2, 0, kBlockSize>)        [RUNNING] [PASSED] (20 ms)
    (test_sparse<kBlockSize / 2, kBlockSize, kBlockSize>) [RUNNING] [PASSED] (19 ms)
    (test_sparse<kBlockSize, 0, kBlockSize>)            [RUNNING] [PASSED] (19 ms)
    (test_sparse<kBlockSize, kBlockSize / 2, kBlockSize>) [RUNNING] [PASSED] (19 ms)
    (test_sparse<kBlockSize * kDirectBlocks, kBlockSize * kDirectBlocks - kBlockSize, kBlockSize * 2>) [RUNNING] [PASSED] (20 ms)
    (test_sparse<kBlockSize * kDirectBlocks, kBlockSize * kDirectBlocks - kBlockSize, kBlockSize * 32>) [RUNNING] [PASSED] (24 ms)
    (test_sparse<kBlockSize * kDirectBlocks + kBlockSize, kBlockSize * kDirectBlocks - kBlockSize, kBlockSize * 32>) [RUNNING] [PASSED] (24 ms)
    (test_sparse<kBlockSize * kDirectBlocks + kBlockSize, kBlockSize * kDirectBlocks + 2 * kBlockSize, kBlockSize * 32>) [RUNNING] [PASSED] (25 ms)
CASE minfs_sparse_tests                                 [PASSED]

CASE minfs_rw_workers_test                              [STARTED]
    TestWorkSingleThread                                [RUNNING] [PASSED] (40920 ms)
CASE minfs_rw_workers_test                              [PASSED]

CASE minfs_maxfile_tests                                [STARTED]
    test_maxfile                                        [RUNNING] [PASSED] (62243 ms)
CASE minfs_maxfile_tests                                [PASSED]

CASE minfs_directory_tests                              [STARTED]
    TestDirectoryLarge                                  [RUNNING] [PASSED] (3251 ms)
    TestDirectoryReaddir                                [RUNNING] [PASSED] (69 ms)
    TestDirectoryReaddirLarge                           [RUNNING] [PASSED] (6414 ms)
CASE minfs_directory_tests                              [PASSED]

CASE minfs_basic_tests                                  [STARTED]
    test_basic                                          [RUNNING] [PASSED] (21 ms)
CASE minfs_basic_tests                                  [PASSED]
====================================================
Results for test binary "host_x64-asan/fs-host":
    SUCCESS!  All test cases passed!
    CASES:  6     SUCCESS:  5     FAILED:  1
====================================================
`

	want := []runtests.TestCaseResult{
		{
			DisplayName: "minfs_truncate_tests.TestTruncateSmall",
			SuiteName:   "minfs_truncate_tests",
			CaseName:    "TestTruncateSmall",
			Duration:    1000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_truncate_tests.(TestTruncateLarge\u003c1 \u003c\u003c 10, 1000\u003e)",
			SuiteName:   "minfs_truncate_tests",
			CaseName:    "(TestTruncateLarge\u003c1 \u003c\u003c 10, 1000\u003e)",
			Duration:    20414000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_truncate_tests.(TestTruncateLarge\u003c1 \u003c\u003c 15, 500\u003e)",
			SuiteName:   "minfs_truncate_tests",
			CaseName:    "(TestTruncateLarge\u003c1 \u003c\u003c 15, 500\u003e)",
			Duration:    10012000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_truncate_tests.(TestTruncateLarge\u003c1 \u003c\u003c 20, 500\u003e)",
			SuiteName:   "minfs_truncate_tests",
			CaseName:    "(TestTruncateLarge\u003c1 \u003c\u003c 20, 500\u003e)",
			Duration:    10973000000,
			Status:      runtests.TestFailure,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_truncate_tests.(TestTruncateLarge\u003c1 \u003c\u003c 25, 500\u003e)",
			SuiteName:   "minfs_truncate_tests",
			CaseName:    "(TestTruncateLarge\u003c1 \u003c\u003c 25, 500\u003e)",
			Duration:    0,
			Status:      runtests.TestSkipped,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_sparse_tests.(test_sparse\u003c0, 0, kBlockSize\u003e)",
			SuiteName:   "minfs_sparse_tests",
			CaseName:    "(test_sparse\u003c0, 0, kBlockSize\u003e)",
			Duration:    19000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_sparse_tests.(test_sparse\u003ckBlockSize / 2, 0, kBlockSize\u003e)",
			SuiteName:   "minfs_sparse_tests",
			CaseName:    "(test_sparse\u003ckBlockSize / 2, 0, kBlockSize\u003e)",
			Duration:    20000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_sparse_tests.(test_sparse\u003ckBlockSize / 2, kBlockSize, kBlockSize\u003e)",
			SuiteName:   "minfs_sparse_tests",
			CaseName:    "(test_sparse\u003ckBlockSize / 2, kBlockSize, kBlockSize\u003e)",
			Duration:    19000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_sparse_tests.(test_sparse\u003ckBlockSize, 0, kBlockSize\u003e)",
			SuiteName:   "minfs_sparse_tests",
			CaseName:    "(test_sparse\u003ckBlockSize, 0, kBlockSize\u003e)",
			Duration:    19000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_sparse_tests.(test_sparse\u003ckBlockSize, kBlockSize / 2, kBlockSize\u003e)",
			SuiteName:   "minfs_sparse_tests",
			CaseName:    "(test_sparse\u003ckBlockSize, kBlockSize / 2, kBlockSize\u003e)",
			Duration:    19000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_sparse_tests.(test_sparse\u003ckBlockSize * kDirectBlocks, kBlockSize * kDirectBlocks - kBlockSize, kBlockSize * 2\u003e)",
			SuiteName:   "minfs_sparse_tests",
			CaseName:    "(test_sparse\u003ckBlockSize * kDirectBlocks, kBlockSize * kDirectBlocks - kBlockSize, kBlockSize * 2\u003e)",
			Duration:    20000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_sparse_tests.(test_sparse\u003ckBlockSize * kDirectBlocks, kBlockSize * kDirectBlocks - kBlockSize, kBlockSize * 32\u003e)",
			SuiteName:   "minfs_sparse_tests",
			CaseName:    "(test_sparse\u003ckBlockSize * kDirectBlocks, kBlockSize * kDirectBlocks - kBlockSize, kBlockSize * 32\u003e)",
			Duration:    24000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_sparse_tests.(test_sparse\u003ckBlockSize * kDirectBlocks + kBlockSize, kBlockSize * kDirectBlocks - kBlockSize, kBlockSize * 32\u003e)",
			SuiteName:   "minfs_sparse_tests",
			CaseName:    "(test_sparse\u003ckBlockSize * kDirectBlocks + kBlockSize, kBlockSize * kDirectBlocks - kBlockSize, kBlockSize * 32\u003e)",
			Duration:    24000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_sparse_tests.(test_sparse\u003ckBlockSize * kDirectBlocks + kBlockSize, kBlockSize * kDirectBlocks + 2 * kBlockSize, kBlockSize * 32\u003e)",
			SuiteName:   "minfs_sparse_tests",
			CaseName:    "(test_sparse\u003ckBlockSize * kDirectBlocks + kBlockSize, kBlockSize * kDirectBlocks + 2 * kBlockSize, kBlockSize * 32\u003e)",
			Duration:    25000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_rw_workers_test.TestWorkSingleThread",
			SuiteName:   "minfs_rw_workers_test",
			CaseName:    "TestWorkSingleThread",
			Duration:    40920000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_maxfile_tests.test_maxfile",
			SuiteName:   "minfs_maxfile_tests",
			CaseName:    "test_maxfile",
			Duration:    62243000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_directory_tests.TestDirectoryLarge",
			SuiteName:   "minfs_directory_tests",
			CaseName:    "TestDirectoryLarge",
			Duration:    3251000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_directory_tests.TestDirectoryReaddir",
			SuiteName:   "minfs_directory_tests",
			CaseName:    "TestDirectoryReaddir",
			Duration:    69000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_directory_tests.TestDirectoryReaddirLarge",
			SuiteName:   "minfs_directory_tests",
			CaseName:    "TestDirectoryReaddirLarge",
			Duration:    6414000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		}, {
			DisplayName: "minfs_basic_tests.test_basic",
			SuiteName:   "minfs_basic_tests",
			CaseName:    "test_basic",
			Duration:    21000000,
			Status:      runtests.TestSuccess,
			Format:      "Zircon utest",
		},
	}
	testCaseCmp(t, stdout, want)
}

// Regression test for https://fxbug.dev/42128506
func TestFxb51327(t *testing.T) {
	stdout := `
[==========] Running 13 tests from 3 test suites.
[----------] Global test environment set-up.
[----------] 10 tests from UltrasoundTest
[ RUN      ] UltrasoundTest.CreateRenderer
[00392.026593][276880][276883][test-devmgr] INFO:
[00392.026655][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.026783][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.026793][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00391.063831][276429][279657][audio_pipeline_test] INFO: [hermetic_audio_environment.cc(40)] Using path '/pkg/data/ultrasound' for /config/data directory for fuchsia-pkg://fuchsia.com/audio_core#meta/audio_core_nodevfs_noconfigdata.cm.
[00391.064291][276429][279657][audio_pipeline_test] INFO: [hermetic_audio_environment.cc(54)] No config_data provided for fuchsia-pkg://fuchsia.com/virtual_audio_service#meta/virtual_audio_service_nodevfs.cm
[00391.820503][280027][280029][audio_core] INFO: [main.cc(36)] AudioCore starting up
[00391.891903][280027][280029][audio_core] INFO: [policy_loader.cc(244)] No policy found; using default.
[00392.007930][280027][280375][audio_core] ERROR: [src/media/audio/audio_core/driver_output.cc(166)] OUTPUT UNDERFLOW: Missed mix target by (worst-case, expected) = (49, 99) ms. Cooling down for 1000 milliseconds.
[00392.007972][280027][280375][audio_core] ERROR: [src/media/audio/audio_core/reporter.cc(428)] UNDERFLOW: Failed to obtain the Cobalt logger
[00392.026545][280027][280375][audio_core] INFO: [audio_driver_v2.cc(585)] Output shutting down 'Stream channel closed unexpectedly', status:-24
[00392.026600][280027][280375][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[       OK ] UltrasoundTest.CreateRenderer (964 ms)
[ RUN      ] UltrasoundTest.RendererDoesNotSupportSetPcmStreamType
[00392.169044][276880][276883][test-devmgr] INFO:
[00392.169124][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.169299][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.169321][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00392.026628][280027][280375][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.140514][280027][280436][audio_core] ERROR: [src/media/audio/audio_core/driver_output.cc(166)] OUTPUT UNDERFLOW: Missed mix target by (worst-case, expected) = (48.95, 98) ms. Cooling down for 1000 milliseconds.
[00392.140549][280027][280436][audio_core] ERROR: [src/media/audio/audio_core/reporter.cc(428)] UNDERFLOW: Failed to obtain the Cobalt logger
[00392.158798][280027][280029][audio_core] ERROR: [src/media/audio/audio_core/ultrasound_renderer.cc(55)] Unsupported method SetPcmStreamType on ultrasound renderer
[00392.168963][280027][280436][audio_core] INFO: [audio_driver_v2.cc(585)] Output shutting down 'Stream channel closed unexpectedly', status:-24
[       OK ] UltrasoundTest.RendererDoesNotSupportSetPcmStreamType (142 ms)
[ RUN      ] UltrasoundTest.RendererDoesNotSupportSetUsage
[00392.281231][276880][276883][test-devmgr] INFO:
[00392.281289][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.281417][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.281428][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00392.169001][280027][280436][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.169021][280027][280436][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.253474][280027][280498][audio_core] ERROR: [src/media/audio/audio_core/driver_output.cc(166)] OUTPUT UNDERFLOW: Missed mix target by (worst-case, expected) = (20.51, 70) ms. Cooling down for 1000 milliseconds.
[00392.253501][280027][280498][audio_core] ERROR: [src/media/audio/audio_core/reporter.cc(428)] UNDERFLOW: Failed to obtain the Cobalt logger
[00392.270892][280027][280029][audio_core] ERROR: [src/media/audio/audio_core/ultrasound_renderer.cc(59)] Unsupported method SetUsage on ultrasound renderer
[00392.281120][280027][280498][audio_core] INFO: [audio_driver_v2.cc(585)] Output shutting down 'Stream channel closed unexpectedly', status:-24
[       OK ] UltrasoundTest.RendererDoesNotSupportSetUsage (112 ms)
[ RUN      ] UltrasoundTest.RendererDoesNotSupportBindGainControl
[00392.423415][276880][276883][test-devmgr] INFO:
[00392.423473][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.423785][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.423800][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00392.281148][280027][280498][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.281169][280027][280498][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.394476][280027][280562][audio_core] ERROR: [src/media/audio/audio_core/driver_output.cc(166)] OUTPUT UNDERFLOW: Missed mix target by (worst-case, expected) = (49, 99) ms. Cooling down for 1000 milliseconds.
[00392.394513][280027][280562][audio_core] ERROR: [src/media/audio/audio_core/reporter.cc(428)] UNDERFLOW: Failed to obtain the Cobalt logger
[00392.413146][280027][280029][audio_core] ERROR: [src/media/audio/audio_core/ultrasound_renderer.cc(64)] Unsupported method BindGainControl on ultrasound renderer
[00392.423325][280027][280562][audio_core] INFO: [audio_driver_v2.cc(585)] Output shutting down 'Stream channel closed unexpectedly', status:-24
[00392.423387][280027][280562][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.423417][280027][280562][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[       OK ] UltrasoundTest.RendererDoesNotSupportBindGainControl (151 ms)
[ RUN      ] UltrasoundTest.RendererDoesNotSupportSetReferenceClock
[00392.575423][276880][276883][test-devmgr] INFO:
[00392.575484][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.575618][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.575627][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00392.546406][280027][280625][audio_core] ERROR: [src/media/audio/audio_core/driver_output.cc(166)] OUTPUT UNDERFLOW: Missed mix target by (worst-case, expected) = (48.99, 98) ms. Cooling down for 1000 milliseconds.
[00392.546445][280027][280625][audio_core] ERROR: [src/media/audio/audio_core/reporter.cc(428)] UNDERFLOW: Failed to obtain the Cobalt logger
[00392.565178][280027][280029][audio_core] ERROR: [src/media/audio/audio_core/ultrasound_renderer.cc(68)] Unsupported method SetReferenceClock on ultrasound renderer
[00392.575361][280027][280625][audio_core] INFO: [audio_driver_v2.cc(585)] Output shutting down 'Stream channel closed unexpectedly', status:-24
[       OK ] UltrasoundTest.RendererDoesNotSupportSetReferenceClock (143 ms)
[ RUN      ] UltrasoundTest.CreateCapturer
[00392.607433][276880][276883][test-devmgr] INFO:
[00392.607488][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.607609][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.607620][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00392.575406][280027][280625][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.575427][280027][280625][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.607377][280027][280686][audio_core] INFO: [audio_driver_v2.cc(585)]  Input shutting down 'Stream channel closed unexpectedly', status:-24
[       OK ] UltrasoundTest.CreateCapturer (32 ms)
[ RUN      ] UltrasoundTest.CapturerDoesNotSupportSetPcmStreamType
[00392.649557][276880][276883][test-devmgr] INFO:
[00392.649636][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.649796][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.649812][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00392.607415][280027][280686][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.607436][280027][280686][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.639303][280027][280029][audio_core] ERROR: [src/media/audio/audio_core/ultrasound_capturer.cc(61)] Unsupported method SetPcmStreamType on ultrasound capturer
[00392.649486][280027][280755][audio_core] INFO: [audio_driver_v2.cc(585)]  Input shutting down 'Stream channel closed unexpectedly', status:-24
[00392.649540][280027][280755][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.649569][280027][280755][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[       OK ] UltrasoundTest.CapturerDoesNotSupportSetPcmStreamType (51 ms)
[ RUN      ] UltrasoundTest.CapturerDoesNotSupportSetUsage
[00392.701181][276880][276883][test-devmgr] INFO:
[00392.701258][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.701417][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.701431][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00392.690860][280027][280029][audio_core] ERROR: [src/media/audio/audio_core/ultrasound_capturer.cc(57)] Unsupported method SetUsage on ultrasound capturer
[00392.701096][280027][280824][audio_core] INFO: [audio_driver_v2.cc(585)]  Input shutting down 'Stream channel closed unexpectedly', status:-24
[00392.701154][280027][280824][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.701185][280027][280824][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[       OK ] UltrasoundTest.CapturerDoesNotSupportSetUsage (42 ms)
[ RUN      ] UltrasoundTest.CapturerDoesNotSupportBindGainControl
[00392.743724][276880][276883][test-devmgr] INFO:
[00392.743803][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.743971][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.743989][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00392.733408][280027][280029][audio_core] ERROR: [src/media/audio/audio_core/ultrasound_capturer.cc(66)] Unsupported method BindGainControl on ultrasound capturer
[00392.743640][280027][280891][audio_core] INFO: [audio_driver_v2.cc(585)]  Input shutting down 'Stream channel closed unexpectedly', status:-24
[00392.743708][280027][280891][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.743738][280027][280891][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[       OK ] UltrasoundTest.CapturerDoesNotSupportBindGainControl (52 ms)
[ RUN      ] UltrasoundTest.CapturerDoesNotSupportSetReferenceClock
[00392.795276][276880][276883][test-devmgr] INFO:
[00392.795333][276880][276883][test-devmgr] INFO: Running remove task for device 0xc64fcfe910 'Virtual_Audio_Device_(default)'
[00392.795496][276880][276883][test-devmgr] INFO: Removed device 0xc64fcfe910 'Virtual_Audio_Device_(default)': ZX_OK
[00392.795530][276880][276883][test-devmgr] INFO: Removing device 0xc64fcfe910 'Virtual_Audio_Device_(default)' parent=0xc64fcfe610
[00392.784991][280027][280029][audio_core] ERROR: [src/media/audio/audio_core/ultrasound_capturer.cc(70)] Unsupported method SetReferenceClock on ultrasound capturer
[00392.795201][280027][280962][audio_core] INFO: [audio_driver_v2.cc(585)]  Input shutting down 'Stream channel closed unexpectedly', status:-24
[       OK ] UltrasoundTest.CapturerDoesNotSupportSetReferenceClock (41 ms)
[00392.795237][280027][280962][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(75)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[00392.795258][280027][280962][audio_core] ERROR: [src/media/audio/audio_core/audio_driver_v2.cc(305)] AudioDriver failed with error: -24: -24 (ZX_ERR_PEER_CLOSED)
[----------] 10 tests from UltrasoundTest (1733 ms total)
[----------] Global test environment tear-down
[==========] 13 tests from 3 test suites ran. (10934 ms total)
[  PASSED  ] 13 tests.
ok 9 fuchsia-pkg://fuchsia.com/audio_pipeline_tests#meta/audio_pipeline_tests.cm (12.230948553s)
`
	want := []runtests.TestCaseResult{
		{
			DisplayName: "UltrasoundTest.CreateRenderer",
			SuiteName:   "UltrasoundTest",
			CaseName:    "CreateRenderer",
			Duration:    964000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		}, {
			DisplayName: "UltrasoundTest.RendererDoesNotSupportSetPcmStreamType",
			SuiteName:   "UltrasoundTest",
			CaseName:    "RendererDoesNotSupportSetPcmStreamType",
			Duration:    142000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		}, {
			DisplayName: "UltrasoundTest.RendererDoesNotSupportSetUsage",
			SuiteName:   "UltrasoundTest",
			CaseName:    "RendererDoesNotSupportSetUsage",
			Duration:    112000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		}, {
			DisplayName: "UltrasoundTest.RendererDoesNotSupportBindGainControl",
			SuiteName:   "UltrasoundTest",
			CaseName:    "RendererDoesNotSupportBindGainControl",
			Duration:    151000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		}, {
			DisplayName: "UltrasoundTest.RendererDoesNotSupportSetReferenceClock",
			SuiteName:   "UltrasoundTest",
			CaseName:    "RendererDoesNotSupportSetReferenceClock",
			Duration:    143000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		}, {
			DisplayName: "UltrasoundTest.CreateCapturer",
			SuiteName:   "UltrasoundTest",
			CaseName:    "CreateCapturer",
			Duration:    32000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		}, {
			DisplayName: "UltrasoundTest.CapturerDoesNotSupportSetPcmStreamType",
			SuiteName:   "UltrasoundTest",
			CaseName:    "CapturerDoesNotSupportSetPcmStreamType",
			Duration:    51000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		}, {
			DisplayName: "UltrasoundTest.CapturerDoesNotSupportSetUsage",
			SuiteName:   "UltrasoundTest",
			CaseName:    "CapturerDoesNotSupportSetUsage",
			Duration:    42000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		}, {
			DisplayName: "UltrasoundTest.CapturerDoesNotSupportBindGainControl",
			SuiteName:   "UltrasoundTest",
			CaseName:    "CapturerDoesNotSupportBindGainControl",
			Duration:    52000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		}, {
			DisplayName: "UltrasoundTest.CapturerDoesNotSupportSetReferenceClock",
			SuiteName:   "UltrasoundTest",
			CaseName:    "CapturerDoesNotSupportSetReferenceClock",
			Duration:    41000000,
			Status:      runtests.TestSuccess,
			Format:      "GoogleTest",
		},
	}
	testCaseCmp(t, stdout, want)
}

func TestTRFLegacyTest(t *testing.T) {
	stdout := `
Running test 'fuchsia-pkg://fuchsia.com/vkreadback#meta/vkreadback.cm'
[RUNNING]	legacy_test
[stdout - legacy_test]
[==========] Running 5 tests from 1 test suite.
[stdout - legacy_test]
[----------] Global test environment set-up.
[stdout - legacy_test]
[----------] 5 tests from Vulkan
[stdout - legacy_test]
[ RUN      ] Vulkan.Readback
[stdout - legacy_test]
****** Test Failed! 4096 mismatches
[stdout - legacy_test]
../../src/graphics/tests/vkreadback/main.cc:33: Failure
[stdout - legacy_test]
Value of: test.Readback()
[stdout - legacy_test]
	Actual: false
[stdout - legacy_test]
Expected: true
[stdout - legacy_test]
[  FAILED  ] Vulkan.Readback (132 ms)
[stdout - legacy_test]
[ RUN      ] Vulkan.ManyReadback
[stderr - legacy_test]
Clear Color Value Mismatch at index 0 - expected 0xbf8000ff, got 0xabababab
[stderr - legacy_test]
Clear Color Value Mismatch at index 1 - expected 0xbf8000ff, got 0xabababab
[stderr - legacy_test]
Clear Color Value Mismatch at index 2 - expected 0xbf8000ff, got 0xabababab
[stderr - legacy_test]
Clear Color Value Mismatch at index 3 - expected 0xbf8000ff, got 0xabababab
[stdout - legacy_test]
****** Test Failed! 4096 mismatches
[stdout - legacy_test]
../../src/graphics/tests/vkreadback/main.cc:45: Failure
[stdout - legacy_test]
Value of: test->Readback()
[stdout - legacy_test]
  Actual: false
[stdout - legacy_test]
Expected: true
[stdout - legacy_test]
[  FAILED  ] Vulkan.ManyReadback (9078 ms)
[stdout - legacy_test]
[ RUN      ] Vulkan.ReadbackLoopWithFenceWaitThread
[stderr - legacy_test]
Clear Color Value Mismatch at index 0 - expected 0xbf8000ff, got 0xabababab
[stderr - legacy_test]
Clear Color Value Mismatch at index 1 - expected 0xbf8000ff, got 0xabababab
[stdout - legacy_test]
****** Test Failed! 4096 mismatches
[stdout - legacy_test]
../../src/graphics/tests/vkreadback/main.cc:98: Failure
[stdout - legacy_test]
Value of: test.Readback()
[stdout - legacy_test]
  Actual: false
[stdout - legacy_test]
Expected: true
[stdout - legacy_test]
[  FAILED  ] Vulkan.ReadbackLoopWithFenceWaitThread (340 ms)
`
	want := []runtests.TestCaseResult{}
	testCaseCmp(t, stdout, want)
}

func TestParseVulkanCts(t *testing.T) {
	stdout := `
Writing test log into /data/TestResults.qpa
dEQP Core git-6c86ad6eade572a70482771aa1c4d466fe7106ef (0x6c86ad6e) starting..
  target implementation = 'Fuchsia'
Test case 'dEQP-VK.renderpass.suballocation.multisample.r32g32_uint.samples_8'..
Pass (Pass)
Test case 'dEQP-VK.renderpass.suballocation.multisample.separate_stencil_usage.d32_sfloat_s8_uint.samples_32.test_stencil'..
NotSupported (Image type not supported at vktRenderPassMultisampleTests.cpp:270)
Test case 'dEQP-VK.renderpass.suballocation.multisample.r32g32_uint.samples_9'..
Fail (bad)
Test case 'dEQP-VK.renderpass.suballocation.multisample.r32g32_uint.samples_10'..
`

	want := []runtests.TestCaseResult{
		{
			DisplayName: "dEQP-VK.renderpass.suballocation.multisample.r32g32_uint.samples_8",
			SuiteName:   "dEQP-VK.renderpass.suballocation.multisample.r32g32_uint",
			CaseName:    "samples_8",
			Status:      runtests.TestSuccess,
			Format:      "VulkanCtsTest",
		}, {
			DisplayName: "dEQP-VK.renderpass.suballocation.multisample.separate_stencil_usage.d32_sfloat_s8_uint.samples_32.test_stencil",
			SuiteName:   "dEQP-VK.renderpass.suballocation.multisample.separate_stencil_usage.d32_sfloat_s8_uint.samples_32",
			CaseName:    "test_stencil",
			Status:      runtests.TestSkipped,
			Format:      "VulkanCtsTest",
		}, {
			DisplayName: "dEQP-VK.renderpass.suballocation.multisample.r32g32_uint.samples_9",
			SuiteName:   "dEQP-VK.renderpass.suballocation.multisample.r32g32_uint",
			CaseName:    "samples_9",
			Status:      runtests.TestFailure,
			Format:      "VulkanCtsTest",
		}, {
			DisplayName: "dEQP-VK.renderpass.suballocation.multisample.r32g32_uint.samples_10",
			SuiteName:   "dEQP-VK.renderpass.suballocation.multisample.r32g32_uint",
			CaseName:    "samples_10",
			Status:      runtests.TestFailure,
			Format:      "VulkanCtsTest",
		},
	}
	testCaseCmp(t, stdout, want)
}
