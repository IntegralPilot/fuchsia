// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package dart_test_parser

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
	testCaseCmp(t, "non-test output", []runtests.TestCaseResult{})
}

func TestParseDartSystemTest(t *testing.T) {
	stdout := `
[----------] Test results JSON:
{
  "bqTableName": "e2etest",
  "bqDatasetName": "e2e_test_data",
  "bqProjectName": "fuchsia-infra",
  "buildID": "8880180380045754528",
  "startTime": "2020-05-16 02:44:33.519488",
  "buildBucketInfo": {
    "user": null,
    "botID": "fuchsia-internal-try-n1-1-ssd0-us-central1-c-37-za5b",
    "builderName": "foo",
    "buildID": "8880180380045754528",
    "changeNumber": null,
    "gcsBucket": "paper-crank-rogue-raft",
    "reason": "",
    "repository": "foo",
    "startTime": "2020-05-16 02:44:33.519488"
  },
  "testGroups": [
    {
      "name": "foo_test/group1",
      "result": "PASSED",
      "startTime": "2020-05-16 03:17:20.987638",
      "loginMode": "NOT_RUN",
      "retries": 0,
      "durationInSeconds": 87,
      "testCases": [
        {
          "name": "test1",
          "result": "PASSED",
          "startTime": "2020-05-16 03:17:25.745931",
          "loginMode": "NOT_RUN",
          "retries": 0,
          "durationInSeconds": 52,
          "customFields": [
            {
              "key": "device_name",
              "value": "paper-crank-rogue-raft"
            },
            {
              "key": "transcript",
              "value": "foo"
            }
          ]
        },
        {
          "name": "test2",
          "result": "FAILED",
          "startTime": "2020-05-16 03:18:18.197664",
          "loginMode": "NOT_RUN",
          "retries": 0,
          "durationInSeconds": 30,
          "customFields": [
            {
              "key": "device_name",
              "value": "paper-crank-rogue-raft"
            },
            {
              "key": "transcript",
              "value": "foo"
            }
          ]
        }
      ]
    },
    {
      "name": "foo_test/group2",
      "result": "PASSED",
      "startTime": "2020-05-16 03:17:18.291768",
      "loginMode": "UNKNOWN",
      "retries": 0,
      "durationInSeconds": 90,
      "testCases": []
    }
  ]
}
`
	want := []runtests.TestCaseResult{
		{
			DisplayName: "foo_test/group1.test1",
			SuiteName:   "foo_test/group1",
			CaseName:    "test1",
			Duration:    52000000000,
			Status:      runtests.TestSuccess,
			Format:      "dart_system_test",
		}, {
			DisplayName: "foo_test/group1.test2",
			SuiteName:   "foo_test/group1",
			CaseName:    "test2",
			Duration:    30000000000,
			Status:      runtests.TestFailure,
			Format:      "dart_system_test",
		},
	}
	testCaseCmp(t, stdout, want)
}
