// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This is a tool for modifying the shards.json and task_requests.json files
// to only run the shards/tests that failed in the previous run.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"regexp"
	"strconv"
	"strings"

	swarmingpb "go.chromium.org/luci/swarming/proto/api_v2"
	"go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
	"google.golang.org/protobuf/encoding/protojson"
)

type flags struct {
	hasReusedBuildArtifacts        bool
	skipPreviouslyPassedTestShards bool
	skipPreviouslyPassedTests      bool
	shardsJsonPath                 string
	taskRequestsJsonPath           string
	skippedShardsJsonPath          string
	filteredTaskRequestsJsonPath   string
	presubmitRetryMetadataJsonPath string
}

type PresubmitRetryMetadata struct {
	ShardToFailedTestsMap        map[string][]string `json:"shard_to_failed_tests_map"`
	ShardsWithTefmocheckFailures []string            `json:"shards_with_tefmocheck_failures"`
	TargetTests                  []string            `json:"target_tests"`
}

type Test struct {
	Name string `json:"name"`
}

const twoSpaces = "  "

func parseFlags() flags {
	var f flags
	flag.BoolVar(
		&f.hasReusedBuildArtifacts,
		"has_reused_build_artifacts",
		false,
		"Build uses reused build artifacts from a previous run.",
	)
	flag.BoolVar(
		&f.skipPreviouslyPassedTestShards,
		"skip_previously_passed_test_shards",
		false,
		"Skip test shards that passed in the previous run.",
	)
	flag.BoolVar(
		&f.skipPreviouslyPassedTests,
		"skip_previously_passed_tests",
		false,
		"Skip tests that passed in the previous run.",
	)
	flag.StringVar(
		&f.shardsJsonPath,
		"shards_json_path",
		"shards.json",
		"Path to the shards json file.",
	)
	flag.StringVar(
		&f.taskRequestsJsonPath,
		"task_requests_json_path",
		"task_requests.json",
		"Path to the task requests json file.",
	)
	flag.StringVar(
		&f.skippedShardsJsonPath,
		"skipped_shards_json_path",
		"skipped_shards.json",
		"Path to the skipped shards json file.",
	)
	flag.StringVar(
		&f.filteredTaskRequestsJsonPath,
		"filtered_task_requests_json_path",
		"filtered_task_requests.json",
		"Path to the filtered task requests json file.",
	)
	flag.StringVar(
		&f.presubmitRetryMetadataJsonPath,
		"presubmit_retry_metadata_json_path",
		"presubmit_retry_metadata.json",
		"Path to the presubmit retry metadata json file.",
	)
	flag.Parse()
	return f
}

func readPresubmitRetryMetadata(presubmitRetryMetadataJsonPath string) PresubmitRetryMetadata {
	var metadata PresubmitRetryMetadata
	data, err := os.ReadFile(presubmitRetryMetadataJsonPath)
	if err != nil {
		log.Fatalf("Failed to read presubmit retry metadata json file: %v", err)
	}
	if err := json.Unmarshal(data, &metadata); err != nil {
		log.Fatalf("Failed to unmarshal presubmit retry metadata json: %v", err)
	}
	return metadata
}

func readShards(shardsJsonPath string) []testsharder.Shard {
	var shards []testsharder.Shard
	data, err := os.ReadFile(shardsJsonPath)
	if err != nil {
		log.Fatalf("Failed to read shards json file: %v", err)
	}
	if err := json.Unmarshal(data, &shards); err != nil {
		log.Fatalf("Failed to unmarshal shards json: %v", err)
	}
	return shards
}

func readTaskRequests(taskRequestsJsonPath string) []swarmingpb.NewTaskRequest {
	data, err := os.ReadFile(taskRequestsJsonPath)
	if err != nil {
		log.Fatalf("Failed to read task requests json file: %v", err)
	}
	var rawTaskRequests []json.RawMessage
	if err := json.Unmarshal(data, &rawTaskRequests); err != nil {
		log.Fatalf("Failed to unmarshal task requests json: %v", err)
	}

	taskRequests := make([]swarmingpb.NewTaskRequest, len(rawTaskRequests))
	for i, rawItem := range rawTaskRequests {
		msg := swarmingpb.NewTaskRequest{}
		if err := protojson.Unmarshal(rawItem, &msg); err != nil {
			log.Fatalf("Failed to unmarshal task requests: %v", err)
		}
		taskRequests[i] = msg
	}
	return taskRequests
}

func createSkippedShards(
	shards []testsharder.Shard,
	metadata PresubmitRetryMetadata,
	flags *flags,
) []testsharder.Shard {
	checkShardToFailedTestsMap := flags.hasReusedBuildArtifacts && flags.skipPreviouslyPassedTestShards
	// only include shards that are skippable or are included in the shard_to_failed_tests_map
	skippedShards := []testsharder.Shard{} // do not return a nil slice; this will be marshalled to json
	tefmocheckFailureSet := make(map[string]struct{})
	for _, shardName := range metadata.ShardsWithTefmocheckFailures {
		tefmocheckFailureSet[shardName] = struct{}{}
	}
	for _, shard := range shards {
		_, hasTefmocheckFailure := tefmocheckFailureSet[shard.Name]
		if hasTefmocheckFailure {
			continue
		}
		failedTests, ok := metadata.ShardToFailedTestsMap[shard.Name]
		shardHasFailedTests := ok && len(failedTests) > 0
		shouldSkipPassedShards :=
			checkShardToFailedTestsMap && len(metadata.ShardToFailedTestsMap) > 0
		isFullyPassedShard := !ok && shouldSkipPassedShards
		hasBothFailedAndPassedTests :=
			checkShardToFailedTestsMap &&
				flags.skipPreviouslyPassedTests &&
				shardHasFailedTests &&
				len(shard.SummaryIfSkipped.Tests) > len(metadata.ShardToFailedTestsMap[shard.Name])
		if shard.Skippable || isFullyPassedShard || hasBothFailedAndPassedTests {
			skippedShards = append(skippedShards, shard)
		}
	}
	// do not include tests in the summary that failed in the previous run
	if checkShardToFailedTestsMap &&
		flags.skipPreviouslyPassedTests &&
		len(metadata.ShardToFailedTestsMap) > 0 {
		for i, shard := range skippedShards {
			var filteredTests []runtests.TestDetails
			previouslyFailedTests := metadata.ShardToFailedTestsMap[shard.Name]
			previouslyFailedTestsSet := make(map[string]struct{})
			for _, failedTest := range previouslyFailedTests {
				previouslyFailedTestsSet[failedTest] = struct{}{}
			}
			for _, test := range shard.SummaryIfSkipped.Tests {
				if _, ok := previouslyFailedTestsSet[test.Name]; !ok {
					filteredTests = append(filteredTests, test)
				}
			}
			skippedShards[i].SummaryIfSkipped.Tests = filteredTests
		}
	}
	return skippedShards
}

func getShardNameFromTaskRequest(taskRequest swarmingpb.NewTaskRequest) string {
	return strings.SplitN(taskRequest.Name, "|", 2)[0]
}

func createFilteredTaskRequests(
	taskRequests []swarmingpb.NewTaskRequest,
	metadata PresubmitRetryMetadata,
	flags *flags,
) []swarmingpb.NewTaskRequest {
	if !flags.hasReusedBuildArtifacts ||
		!flags.skipPreviouslyPassedTestShards ||
		len(metadata.ShardToFailedTestsMap) == 0 {
		return taskRequests
	}
	tefmocheckFailureSet := make(map[string]struct{})
	for _, shardName := range metadata.ShardsWithTefmocheckFailures {
		tefmocheckFailureSet[shardName] = struct{}{}
	}
	// only include task requests for shards that are present in the shard_to_failed_tests_map
	filteredTaskRequests := []swarmingpb.NewTaskRequest{} // do not return a nil slice; this will be marshalled to json
	for _, taskRequest := range taskRequests {
		_, isFailedShard := metadata.ShardToFailedTestsMap[getShardNameFromTaskRequest(taskRequest)]
		_, hasTefmocheckFailure := tefmocheckFailureSet[getShardNameFromTaskRequest(taskRequest)]
		if isFailedShard || hasTefmocheckFailure {
			filteredTaskRequests = append(filteredTaskRequests, taskRequest)
		}
	}
	if flags.skipPreviouslyPassedTests {
		for i, taskRequest := range filteredTaskRequests {
			_, hasTefmocheckFailure := tefmocheckFailureSet[getShardNameFromTaskRequest(taskRequest)]
			if hasTefmocheckFailure {
				continue
			}
			if failedTests, _ :=
				metadata.ShardToFailedTestsMap[getShardNameFromTaskRequest(taskRequest)]; len(failedTests) > 0 {
				for j, slice := range taskRequest.TaskSlices {
					if slice.Properties.Env == nil {
						filteredTaskRequests[i].TaskSlices[j].Properties.Env =
							[]*swarmingpb.StringPair{}
					}
					filteredTaskRequests[i].TaskSlices[j].Properties.Env =
						append(filteredTaskRequests[i].TaskSlices[j].Properties.Env,
							&swarmingpb.StringPair{
								Key:   constants.TestAllowlistLengthEnvKey,
								Value: strconv.Itoa(len(failedTests)),
							})
					for k, test := range failedTests {
						filteredTaskRequests[i].TaskSlices[j].Properties.Env =
							append(filteredTaskRequests[i].TaskSlices[j].Properties.Env,
								&swarmingpb.StringPair{
									Key:   fmt.Sprintf(constants.TestAllowlistIndexEnvKeyTemplate, k),
									Value: test,
								})
					}
				}
			}
		}
	}
	return filteredTaskRequests
}

func buildShardToFailedTestsMap(shards []testsharder.Shard, targetTests []string) map[string][]string {
	result := make(map[string][]string)
	if len(targetTests) == 0 {
		return result
	}

	var regexes []*regexp.Regexp
	for _, t := range targetTests {
		re, err := regexp.Compile(t)
		if err != nil {
			log.Printf("Warning: failed to compile regex %q: %v", t, err)
			continue
		}
		regexes = append(regexes, re)
	}

	for _, shard := range shards {
		var matchedTests []string
		for _, test := range shard.Tests {
			for _, re := range regexes {
				if re.MatchString(test.Name) {
					matchedTests = append(matchedTests, test.Name)
					break
				}
			}
		}
		if len(matchedTests) > 0 {
			result[shard.Name] = matchedTests
		}
	}
	return result
}

func main() {
	flags := parseFlags()
	metadata := readPresubmitRetryMetadata(flags.presubmitRetryMetadataJsonPath)
	shards := readShards(flags.shardsJsonPath)
	taskRequests := readTaskRequests(flags.taskRequestsJsonPath)
	if len(metadata.TargetTests) > 0 {
		// Clean input patterns (strip spaces)
		cleanedTargets := make([]string, 0, len(metadata.TargetTests))
		for _, t := range metadata.TargetTests {
			trimmed := strings.TrimSpace(t)
			if trimmed != "" {
				cleanedTargets = append(cleanedTargets, trimmed)
			}
		}

		if len(cleanedTargets) > 0 {
			metadata.ShardToFailedTestsMap = buildShardToFailedTestsMap(shards, cleanedTargets)
			// Fail-fast if targeted tests were requested but matched nothing
			if len(metadata.ShardToFailedTestsMap) == 0 {
				log.Fatalf("Error: target_tests %v matched 0 tests in the build graph. Please check for typos!", cleanedTargets)
			}
			// Clear previous tefmocheck failures as they don't apply to targeted tests
			metadata.ShardsWithTefmocheckFailures = nil

			flags.hasReusedBuildArtifacts = true
			flags.skipPreviouslyPassedTestShards = true
			flags.skipPreviouslyPassedTests = true
		}
	}
	filteredShards := createSkippedShards(shards, metadata, &flags)
	filteredTaskRequests := createFilteredTaskRequests(taskRequests, metadata, &flags)
	jsonData, err := json.MarshalIndent(filteredShards, "", twoSpaces)
	if err != nil {
		log.Fatalf("Failed to marshal filtered shards: %v", err)
	}
	// 0644: readable by everyone, writable by the owner
	if err := os.WriteFile(flags.skippedShardsJsonPath, jsonData, 0644); err != nil {
		log.Fatalf("Failed to write skipped shards: %v", err)
	}
	jsonData, err = json.MarshalIndent(filteredTaskRequests, "", twoSpaces)
	if err != nil {
		log.Fatalf("Failed to marshal filtered task requests: %v", err)
	}
	// 0644: readable by everyone, writable by the owner
	if err := os.WriteFile(flags.filteredTaskRequestsJsonPath, jsonData, 0644); err != nil {
		log.Fatalf("Failed to write filtered task requests: %v", err)
	}
}
