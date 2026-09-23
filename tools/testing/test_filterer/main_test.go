// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"os"
	"testing"

	"github.com/google/go-cmp/cmp"
	swarmingpb "go.chromium.org/luci/swarming/proto/api_v2"
	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
	"google.golang.org/protobuf/testing/protocmp"
)

func TestCreateSkippedShards(t *testing.T) {
	testCases := []struct {
		name            string
		shards          []testsharder.Shard
		metadata        PresubmitRetryMetadata
		flags           flags
		expectedResults []testsharder.Shard
		expectErr       bool
	}{
		{
			name: "build not skipping building fuchsia on a presubmit retry",
			shards: []testsharder.Shard{
				{Name: "shard1", Skippable: false},
				{Name: "shard2", Skippable: true},
				{Name: "shard3", Skippable: false},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap: map[string][]string{
					"shard1": {"test1"},
					"shard3": {"test2"},
				},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        false,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []testsharder.Shard{{Name: "shard2", Skippable: true}},
			expectErr:       false,
		},
		{
			name: "build not skipping test shards on a presubmit retry",
			shards: []testsharder.Shard{
				{Name: "shard1", Skippable: false},
				{Name: "shard2", Skippable: true},
				{Name: "shard3", Skippable: false},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap: map[string][]string{
					"shard1": {"test1"},
					"shard3": {"test2"},
				},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: false,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []testsharder.Shard{{Name: "shard2", Skippable: true}},
			expectErr:       false,
		},
		{
			name: "shard to failed tests map is empty",
			shards: []testsharder.Shard{
				{Name: "shard1", Skippable: false},
				{Name: "shard2", Skippable: false},
				{Name: "shard3", Skippable: true},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap:        map[string][]string{},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []testsharder.Shard{{Name: "shard3", Skippable: true}},
			expectErr:       false,
		},
		{
			name: "shards with tefmocheck failures are not skipped",
			shards: []testsharder.Shard{
				{Name: "shard1", Skippable: false},
				{Name: "shard2", Skippable: false},
				{Name: "shard3", Skippable: true},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap:        map[string][]string{"shard1": {"test1"}},
				ShardsWithTefmocheckFailures: []string{"shard2"},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []testsharder.Shard{{Name: "shard3", Skippable: true}},
			expectErr:       false,
		},
		{
			name:   "empty shards",
			shards: []testsharder.Shard{},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap: map[string][]string{
					"shard1": {"test1"},
					"shard3": {"test2"},
				},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []testsharder.Shard{},
			expectErr:       false,
		},
		{
			name: "skip shards that are not in the shard to failed test map and skippable shards",
			shards: []testsharder.Shard{
				{Name: "shard1", Skippable: false},
				{Name: "shard2", Skippable: false},
				{Name: "shard3", Skippable: true},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap: map[string][]string{
					"shard1": {"test1"},
				},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      false,
			},
			expectedResults: []testsharder.Shard{
				{Name: "shard2", Skippable: false},
				{Name: "shard3", Skippable: true},
			},
			expectErr: false,
		},
		{
			name: "only add skipped results for previously passed tests",
			shards: []testsharder.Shard{
				{Name: "shard1", Skippable: false, SummaryIfSkipped: runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{Name: "test1"},
						{Name: "test2"},
					},
				}},
				{Name: "shard2", Skippable: false, SummaryIfSkipped: runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{Name: "test3"},
						{Name: "test4"},
					},
				}},
				{Name: "shard3", Skippable: true, SummaryIfSkipped: runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{Name: "test5"},
						{Name: "test6"},
					},
				}},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap: map[string][]string{
					"shard1": {"test1"},
				},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []testsharder.Shard{
				{Name: "shard1", Skippable: false, SummaryIfSkipped: runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{Name: "test2"},
					},
				}},
				{Name: "shard2", Skippable: false, SummaryIfSkipped: runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{Name: "test3"},
						{Name: "test4"},
					},
				}},
				{Name: "shard3", Skippable: true, SummaryIfSkipped: runtests.TestSummary{
					Tests: []runtests.TestDetails{
						{Name: "test5"},
						{Name: "test6"},
					},
				}},
			},
			expectErr: false,
		},
	}
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := createSkippedShards(tc.shards, tc.metadata, &tc.flags)

			if diff := cmp.Diff(tc.expectedResults, result, protocmp.Transform()); diff != "" {
				t.Errorf("createSkippedShards() mismatch (-want +got):\n%s", diff)
			}
		})
	}
}

func TestCreateFilteredTaskRequests(t *testing.T) {
	testCases := []struct {
		name            string
		taskRequests    []swarmingpb.NewTaskRequest
		metadata        PresubmitRetryMetadata
		flags           flags
		expectedResults []swarmingpb.NewTaskRequest
		expectErr       bool
	}{
		{
			name: "build not skipping building fuchsia on a presubmit retry",
			taskRequests: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap: map[string][]string{
					"shard1": {"test1"},
					"shard3": {"test2"},
				},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        false,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			expectErr: false,
		},
		{
			name: "build not skipping test shards on a presubmit retry",
			taskRequests: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap: map[string][]string{
					"shard1": {"test1"},
					"shard3": {"test2"},
				},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: false,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			expectErr: false,
		},
		{
			name: "shard to failed tests map is empty",
			taskRequests: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap:        map[string][]string{},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			expectErr: false,
		},
		{
			name: "tasks with tefmocheck failures are not skipped",
			taskRequests: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard4|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap:        map[string][]string{"shard1": {"test1"}, "shard3": {"test2"}},
				ShardsWithTefmocheckFailures: []string{"shard2", "shard3"},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{
					{Properties: &swarmingpb.TaskProperties{
						Env: []*swarmingpb.StringPair{
							{Key: "TEST_ALLOWLIST_LENGTH", Value: "1"},
							{Key: "TEST_ALLOWLIST_INDEX_0", Value: "test1"},
						},
					}},
				}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			expectErr: false,
		},
		{
			name:         "empty task requests",
			taskRequests: []swarmingpb.NewTaskRequest{},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap: map[string][]string{
					"shard1": {"test1"},
					"shard3": {"test2"},
				},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []swarmingpb.NewTaskRequest{},
			expectErr:       false,
		},
		{
			name: "skip task requests that are not in the shard to failed test map and skippable shards",
			taskRequests: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap:        map[string][]string{"shard1": {"test1"}},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      false,
			},
			expectedResults: []swarmingpb.NewTaskRequest{
				{
					Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
						Properties: &swarmingpb.TaskProperties{},
					}},
				},
			},
			expectErr: false,
		},
		{
			name: "only run previously failed tests",
			taskRequests: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard2|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
				{Name: "shard3|builder", TaskSlices: []*swarmingpb.TaskSlice{{
					Properties: &swarmingpb.TaskProperties{},
				}}},
			},
			metadata: PresubmitRetryMetadata{
				ShardToFailedTestsMap:        map[string][]string{"shard1": {"test1"}},
				ShardsWithTefmocheckFailures: []string{},
			},
			flags: flags{
				hasReusedBuildArtifacts:        true,
				skipPreviouslyPassedTestShards: true,
				skipPreviouslyPassedTests:      true,
			},
			expectedResults: []swarmingpb.NewTaskRequest{
				{Name: "shard1|builder", TaskSlices: []*swarmingpb.TaskSlice{
					{Properties: &swarmingpb.TaskProperties{
						Env: []*swarmingpb.StringPair{
							{Key: "TEST_ALLOWLIST_LENGTH", Value: "1"},
							{Key: "TEST_ALLOWLIST_INDEX_0", Value: "test1"},
						},
					}},
				}},
			},
			expectErr: false,
		},
	}
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := createFilteredTaskRequests(tc.taskRequests, tc.metadata, &tc.flags)

			if diff := cmp.Diff(tc.expectedResults, result, protocmp.Transform()); diff != "" {
				t.Errorf("createFilteredTaskRequests() mismatch (-want +got):\n%s", diff)
			}
		})
	}
}

func TestReadTaskRequests(t *testing.T) {
	tempFile, err := os.CreateTemp("", "task_requests.json")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}
	defer os.Remove(tempFile.Name())
	defer tempFile.Close()

	// Write test data to the temp file
	jsonString := `
			[
				{
					"name": "shard1|bucket/builder",
					"priority": 20,
					"service_account": "test-service-account",
					"realm": "project:bucket",
					"tags": ["tag1", "tag2"],
					"task_slices": [
						{
							"properties": {
								"cas_input_root": {
									"cas_instance": "projects/chromium_swarm/instances/default_instance",
									"digest": {
										"hash": "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
										"size_bytes": 238
									}
								},
								"command": ["echo", "hello"],
								"containment": {
									"containment_type": "NONE"
								},
								"dimensions": [
									{
										"key": "key1",
										"value": "value1"
									}
								],
								"env": [
									{
										"key": "key1",
										"value": "value1"
									}
								],
								"execution_timeout_secs": 3600,
								"grace_period_secs": 60,
								"outputs": [
									"serial_logs",
									"syslogs"
								],
								"relative_cwd": "/path/to/relative/cwd"
							}
						}
					]
				}
			]
		`
	expectedResults := []swarmingpb.NewTaskRequest{
		{
			Name:           "shard1|bucket/builder",
			Priority:       20,
			ServiceAccount: "test-service-account",
			Realm:          "project:bucket",
			Tags:           []string{"tag1", "tag2"},
			TaskSlices: []*swarmingpb.TaskSlice{
				{
					Properties: &swarmingpb.TaskProperties{
						CasInputRoot: &swarmingpb.CASReference{
							CasInstance: "projects/chromium_swarm/instances/default_instance",
							Digest: &swarmingpb.Digest{
								Hash:      "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
								SizeBytes: 238,
							},
						},
						Command: []string{"echo", "hello"},
						Containment: &swarmingpb.Containment{
							ContainmentType: swarmingpb.Containment_NONE,
						},
						Dimensions: []*swarmingpb.StringPair{
							{Key: "key1", Value: "value1"},
						},
						Env: []*swarmingpb.StringPair{
							{Key: "key1", Value: "value1"},
						},
						ExecutionTimeoutSecs: 3600,
						GracePeriodSecs:      60,
						Outputs:              []string{"serial_logs", "syslogs"},
						RelativeCwd:          "/path/to/relative/cwd",
					},
				},
			},
		},
	}
	if _, err := tempFile.Write([]byte(jsonString)); err != nil {
		t.Fatalf("Failed to write test data: %v", err)
	}
	if err := tempFile.Close(); err != nil {
		t.Fatalf("Failed to close temp file: %v", err)
	}

	// Read the data back using the function under test
	result := readTaskRequests(tempFile.Name())

	// Compare the result with the expected data
	if diff := cmp.Diff(expectedResults, result, protocmp.Transform()); diff != "" {
		t.Errorf("readTaskRequests() mismatch (-want +got):\n%s", diff)
	}
}

func TestBuildShardToFailedTestsMap(t *testing.T) {
	shards := []testsharder.Shard{
		{
			Name: "shard1",
			Tests: []testsharder.Test{
				{Test: build.Test{Name: "test1"}},
				{Test: build.Test{Name: "test2"}},
				{Test: build.Test{Name: "other_test"}},
			},
		},
		{
			Name: "shard2",
			Tests: []testsharder.Test{
				{Test: build.Test{Name: "test3"}},
				{Test: build.Test{Name: "another_test"}},
			},
		},
	}

	testCases := []struct {
		name        string
		targetTests []string
		expected    map[string][]string
	}{
		{
			name:        "exact match",
			targetTests: []string{"test1"},
			expected: map[string][]string{
				"shard1": {"test1"},
			},
		},
		{
			name:        "regex wildcard match",
			targetTests: []string{"^test.*"},
			expected: map[string][]string{
				"shard1": {"test1", "test2"},
				"shard2": {"test3"},
			},
		},
		{
			name:        "multiple regexes",
			targetTests: []string{"test1", "another_.*"},
			expected: map[string][]string{
				"shard1": {"test1"},
				"shard2": {"another_test"},
			},
		},
		{
			name:        "no match",
			targetTests: []string{"non_existent"},
			expected:    map[string][]string{},
		},
		{
			name:        "target 2 tests and skip the rest",
			targetTests: []string{"test1", "test3"},
			expected: map[string][]string{
				"shard1": {"test1"},
				"shard2": {"test3"},
			},
		},
		{
			name:        "target 0 tests (empty list) and skip all",
			targetTests: []string{},
			expected:    map[string][]string{},
		},
		{
			name:        "invalid regex ignored",
			targetTests: []string{"[invalid", "test1"},
			expected: map[string][]string{
				"shard1": {"test1"},
			},
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			result := buildShardToFailedTestsMap(shards, tc.targetTests)
			if diff := cmp.Diff(tc.expected, result); diff != "" {
				t.Errorf("buildShardToFailedTestsMap() mismatch (-want +got):\n%s", diff)
			}
		})
	}
}
