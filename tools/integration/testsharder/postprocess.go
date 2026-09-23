// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testsharder

import (
	"cmp"
	"container/heap"
	"context"
	"encoding/json"
	"fmt"
	"hash/fnv"
	"math"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

const (
	// The maximum number of runs that testsharder will calculate for a multiplied
	// test if totalRuns is unset.
	// TODO(olivernewman): Apply a maximum to user-specified values too, but
	// probably not in testsharder since we'll want to get feedback to users ASAP if
	// validation fails.
	multipliedTestMaxRuns = 1000

	// The maximum total runs across all automatically multiplied tests.
	MaxMultipliedRunsPerShard = 1000

	// The maximum number of multiplied shards allowed per environment.
	maxMultipliedShardsPerEnv = 5

	// The maximum number of multiplied shards to run per isolated test (1 run per shard).
	// This applies if the number of runs are not specified or if -ignore-mulitply-limit
	// is not set.
	maxMultipliedShardsPerIsolatedTest = 5

	// The prefix added to the names of shards that run affected hermetic tests.
	AffectedShardPrefix = "affected:"

	// The prefix added to the names of shards that run unaffected hermetic tests.
	HermeticShardPrefix = "hermetic:"

	// The prefix added to the names of shards that run multiplied tests.
	MultipliedShardPrefix = "multiplied:"

	// The name of the key of the expected duration test tag.
	expectedDurationTagKey = "expected_duration_milliseconds"
)

// ApplyModifiers will return an error that unwraps to this if multiple default
// test modifiers are provided.
var errMultipleDefaultModifiers = fmt.Errorf("too many default modifiers, only one is allowed")

// MarkShardsSkipped will return an error that unwraps to this if the shards
// contain an affected test.
var errSkippedAffectedTest = fmt.Errorf("attempted to skip an affected test")

func ExtractDeps(shards []*Shard, fuchsiaBuildDir string) error {
	for _, shard := range shards {
		if err := extractDepsFromShard(shard, fuchsiaBuildDir); err != nil {
			return err
		}
	}
	return nil
}

func extractDepsFromShard(shard *Shard, fuchsiaBuildDir string) error {
	var shardDeps []string
	for i := range shard.Tests {
		test, deps, err := extractDepsFromTest(shard.Tests[i], fuchsiaBuildDir)
		if err != nil {
			return err
		}
		// extractDepsFromTest may modify the test, so we need to overwrite the
		// entry.
		shard.Tests[i] = test
		// Any test that doesn't run on Fuchsia is invoked via an executable in
		// the build out directory. The executable itself needs to be copied to
		// the testing bot along with the test's deps.
		if test.OS != "fuchsia" && test.Path != "" {
			deps = append(deps, test.Path)
		}
		for _, dep := range deps {
			_, err := os.Stat(filepath.Join(fuchsiaBuildDir, dep))
			if os.IsNotExist(err) {
				return fmt.Errorf("dependency for test %q was not built: %s", test.Name, dep)
			} else if err != nil {
				return err
			}
		}
		shardDeps = append(shardDeps, deps...)
	}
	shard.AddDeps(shardDeps)
	return nil
}

func extractDepsFromTest(test Test, fuchsiaBuildDir string) (Test, []string, error) {
	if test.RuntimeDepsFile == "" {
		return test, nil, nil
	}
	path := filepath.Join(fuchsiaBuildDir, test.RuntimeDepsFile)
	f, err := os.Open(path)
	if err != nil {
		return test, nil, err
	}
	defer f.Close()
	// RuntimeDepsFile is no longer needed at this point and clutters the output.
	test.RuntimeDepsFile = ""
	var deps []string
	err = json.NewDecoder(f).Decode(&deps)
	return test, deps, err
}

func envsEqual(env1, env2 build.Environment) bool {
	return environmentName(env1) == environmentName(env2)
}

// SplitOutMultipliers appends new shards to shards where each new shard contains one test
// repeated multiple times according to the TotalRuns for the test. Multiplier modifiers
// should already be applied to the shards beforehand using ApplyModifiers().
// It also removes all multiplied tests from the input shards.
func SplitOutMultipliers(
	ctx context.Context,
	shards []*Shard,
	testDurations TestDurationsMap,
	// TODO(olivernewman): Use the adjusted target duration calculated by
	// WithTargetDuration instead of the original target duration.
	targetDuration time.Duration,
	targetTestCount int,
	maxMultipliedRunsPerShard int,
	prefix string,
	ignoreMultiplyIsolatedLimit bool,
) []*Shard {
	shardIdxToMatches := make([]map[int]struct{}, len(shards))

	for si, shard := range shards {
		shardIdxToMatches[si] = make(map[int]struct{})
		for ti, test := range shard.Tests {
			if test.RunAlgorithm != StopOnFailure {
				continue
			}
			shard.Tests[ti] = test
			shardIdxToMatches[si][ti] = struct{}{}
		}
	}

	for si := len(shards) - 1; si >= 0; si-- {
		shard := shards[si]
		matches := shardIdxToMatches[si]
		// Remove the multiplied tests from the other shard. It's wasteful to run it in
		// different shards.
		// If entire shard is empty (every test is matched to a multiplier), remove it.
		shardRemoved := false
		if len(matches) == len(shard.Tests) {
			copy(shards[si:], shards[si+1:])
			shards = shards[:len(shards)-1]
			shardRemoved = true
		}
		var multipliedTests, multipliedIsolatedTests []Test
		totalTestsDuration := 0
		maxTestsDuration := 0
		totalTestCount := 0
		maxTestCount := 0
		for ti := len(shard.Tests) - 1; ti >= 0; ti-- {
			if _, ok := matches[ti]; !ok {
				// If there is no match associated with this test,
				// leave it in the original shard and don't multiply.
				continue
			}
			test := shard.Tests[ti]
			if test.Isolated {
				multipliedIsolatedTests = append(multipliedIsolatedTests, test)
			} else {
				if targetDuration > 0 {
					expectedDuration := testDurations.Get(test).MedianDuration
					totalTestsDuration += int(expectedDuration) * max(1, test.Runs)
					maxTestsDuration += int(expectedDuration) * test.maxRuns()
				} else if targetTestCount > 0 {
					totalTestCount += max(1, test.Runs)
					maxTestCount += test.maxRuns()
				} else {
					test.Runs = max(test.Runs, 1)
				}
				multipliedTests = append(multipliedTests, test)
			}
			if shardRemoved {
				continue
			}
			// Remove individual tests from the shard.
			copy(shard.Tests[ti:], shard.Tests[ti+1:])
			shard.Tests = shard.Tests[:len(shard.Tests)-1]

		}

		if len(multipliedIsolatedTests) > 0 {
			for _, test := range multipliedIsolatedTests {
				totalRuns := test.maxRuns()
				if !ignoreMultiplyIsolatedLimit {
					totalRuns = min(totalRuns, maxMultipliedShardsPerIsolatedTest)
				}
				test.Runs = 1
				for i := 1; i <= totalRuns; i++ {
					multShard := *shard
					multShard.Name = fmt.Sprintf("%s-(%d)", shard.Name, i)
					multShard.Tests = []Test{test}
					multShard.TimeoutSecs = int(computeShardTimeout(subshard{targetDuration, multShard.Tests, getTargetStartupDuration(shard.Env)}).Seconds())
					shards = append(shards, &multShard)
				}
			}
		}
		if len(multipliedTests) > 0 {
			maxMultipliedShards := maxMultipliedShardsPerEnv
			if targetDuration > 0 {
				maxMultipliedShards = min(maxMultipliedShards, divRoundUp(maxTestsDuration, int(targetDuration)))
			} else if targetTestCount > 0 {
				maxMultipliedShards = min(maxMultipliedShards, divRoundUp(maxTestCount, targetTestCount))
			} else {
				maxMultipliedShards = 1
			}
			numNewShards := min(len(multipliedTests), maxMultipliedShards)

			multShard := *shard
			multShard.Name = prefix + shard.Name
			multShard.Tests = multipliedTests
			newShards := shardByTime(&multShard, testDurations, numNewShards)

			for _, newShard := range newShards {
				// Give priority to the tests that had a total_runs specified and
				// use the remaining time/test count to fill up the rest of the
				// shard with the remaining tests.
				var usedUpDuration time.Duration
				usedUpCount := 0
				var fillUpTestIdxs []int
				for i, test := range newShard.Tests {
					if test.Runs > 0 {
						if targetDuration > 0 {
							usedUpDuration += testDurations.Get(test).MedianDuration * time.Duration(test.Runs)
						} else if targetTestCount > 0 {
							usedUpCount += test.Runs
						}
					} else {
						fillUpTestIdxs = append(fillUpTestIdxs, i)
					}
				}

				// Fill up the remainder of the shard with the remaining tests,
				// splitting up the duration/count evenly between the tests and
				// running each at least once.
				sort.Slice(fillUpTestIdxs, func(a, b int) bool {
					aDuration := testDurations.Get(newShard.Tests[fillUpTestIdxs[a]]).MedianDuration
					bDuration := testDurations.Get(newShard.Tests[fillUpTestIdxs[b]]).MedianDuration
					return aDuration > bDuration
				})

				// If a shard's tests generate many test cases in a short amount
				// of time, the resulting summary.json file may be too large to
				// be processed. So we set a limit on the total number of
				// multiplied test runs across all tests in the shard.
				maxRunsPerFillUpTest := multipliedTestMaxRuns
				if len(fillUpTestIdxs) > 0 {
					maxRunsPerFillUpTest = min(maxRunsPerFillUpTest, divRoundUp(maxMultipliedRunsPerShard, len(fillUpTestIdxs)))
				}

				for i := len(fillUpTestIdxs) - 1; i >= 0; i-- {
					idx := fillUpTestIdxs[i]
					test := newShard.Tests[idx]
					if targetDuration > 0 {
						remainingDuration := max(0, int(targetDuration-usedUpDuration))
						durationPerTest := divRoundUp(remainingDuration, len(fillUpTestIdxs))
						testDuration := testDurations.Get(test).MedianDuration
						test.Runs = min(maxRunsPerFillUpTest, divRoundUp(durationPerTest, int(testDuration)))
						usedUpDuration += time.Duration(min(int(testDuration)*maxRunsPerFillUpTest, durationPerTest))
						test.Runs = max(1, test.Runs)
						test.StopRepeatingAfterSecs = int(time.Duration(durationPerTest).Seconds())
					} else if targetTestCount > 0 {
						remainingCount := max(0, targetTestCount-usedUpCount)
						countPerTest := divRoundUp(remainingCount, len(fillUpTestIdxs))
						usedUpCount += countPerTest
						test.Runs = max(1, countPerTest)
					} else {
						test.Runs = 1
					}
					newShard.Tests[idx] = test
					fillUpTestIdxs = fillUpTestIdxs[:i]
				}
				newShard.TimeoutSecs = max(newShard.TimeoutSecs, int(computeShardTimeout(subshard{targetDuration, newShard.Tests, getTargetStartupDuration(newShard.Env)}).Seconds()))
			}
			shards = append(shards, newShards...)
		}
	}

	return shards
}

// AddExpectedDurationTags uses the given TestDurations to annotate each test
// with an expected duration tag.
func AddExpectedDurationTags(shards []*Shard, testDurations TestDurationsMap) []*Shard {
	for _, shard := range shards {
		var newTests []Test
		for _, test := range shard.Tests {
			td := testDurations.Get(test)
			test.Tags = append(test.Tags, build.TestTag{
				Key:   expectedDurationTagKey,
				Value: strconv.FormatInt(td.MedianDuration.Milliseconds(), 10),
			})
			newTests = append(newTests, test)
		}
		shard.Tests = newTests
	}
	return shards
}

// ApplyModifiers applies the given test modifiers to tests in the given shards.
func ApplyModifiers(shards []*Shard, modMatches []ModifierMatch) ([]*Shard, error) {
	modsPerEnv := make(map[string]ModifierMatch)
	for _, modMatch := range modMatches {
		if modMatch.Test == "" {
			if _, ok := modsPerEnv[environmentName(modMatch.Env)]; ok {
				return nil, errMultipleDefaultModifiers
			}
			modMatch.Modifier.TotalRuns = -1
			modsPerEnv[environmentName(modMatch.Env)] = modMatch
		}
	}
	defaultEnv := build.Environment{}
	if _, ok := modsPerEnv[environmentName(defaultEnv)]; !ok {
		modsPerEnv[environmentName(defaultEnv)] = ModifierMatch{Modifier: TestModifier{TotalRuns: -1}}
	}

	for _, shard := range shards {
		var modifiedTests []Test
		for _, test := range shard.Tests {
			defaultModForEnv, ok := modsPerEnv[environmentName(shard.Env)]
			if !ok {
				defaultModForEnv = modsPerEnv[environmentName(defaultEnv)]
			}
			test.applyModifier(defaultModForEnv.Modifier, shard)
			for _, modMatch := range modMatches {
				if !envsEqual(modMatch.Env, build.Environment{}) && !envsEqual(modMatch.Env, shard.Env) {
					continue
				}
				if modMatch.Test == test.Name {
					test.applyModifier(modMatch.Modifier, shard)
				}
			}
			modifiedTests = append(modifiedTests, test)
		}
		shard.Tests = modifiedTests
	}
	return shards, nil
}

// PreserveOrder reorders the tests in the shard according to the order of the provided
// modMatches. Any tests not listed in modMatches will come last in the original order.
// Different orders for different environments will be respected and modMatches with a
// default environment will apply to all environments.
func PreserveOrder(shards []*Shard, modMatches []ModifierMatch) []*Shard {
	testOrder := make(map[string]int)
	for i, modMatch := range modMatches {
		testOrder[environmentName(modMatch.Env)+modMatch.Test] = i
	}
	defaultEnv := environmentName(build.Environment{})
	for _, shard := range shards {
		shardEnv := environmentName(shard.Env)
		origOrder := make(map[string]int)
		for i, test := range shard.Tests {
			origOrder[test.Name] = len(modMatches) + i
		}
		slices.SortFunc(shard.Tests, func(a, b Test) int {
			var aOrder, bOrder int
			var ok bool
			if aOrder, ok = testOrder[shardEnv+a.Name]; !ok {
				if aOrder, ok = testOrder[defaultEnv+a.Name]; !ok {
					aOrder = origOrder[a.Name]
				}
			}
			if bOrder, ok = testOrder[shardEnv+b.Name]; !ok {
				if bOrder, ok = testOrder[defaultEnv+b.Name]; !ok {
					bOrder = origOrder[b.Name]
				}
			}
			return cmp.Compare(aOrder, bOrder)
		})
	}
	return shards
}

// PartitionShards splits a set of shards in two using the given partition
// function.
func PartitionShards(shards []*Shard, partitionFunc func(Test) bool, prefix string) ([]*Shard, []*Shard) {
	matchingShards := make([]*Shard, 0, len(shards))
	nonmatchingShards := make([]*Shard, 0, len(shards))
	for _, shard := range shards {
		var matching []Test
		var nonmatching []Test
		for _, test := range shard.Tests {
			if partitionFunc(test) {
				matching = append(matching, test)
			} else {
				nonmatching = append(nonmatching, test)
			}
		}
		if len(matching) > 0 {
			newShard := *shard
			newShard.Name = prefix + shard.Name
			newShard.Tests = matching
			matchingShards = append(matchingShards, &newShard)
		}
		if len(nonmatching) > 0 {
			shard.Tests = nonmatching
			nonmatchingShards = append(nonmatchingShards, shard)
		}
	}
	return matchingShards, nonmatchingShards
}

// MarkShardsSkipped marks the entire set of shards skipped.
func MarkShardsSkipped(shards []*Shard) ([]*Shard, error) {
	var newShards []*Shard
	for _, shard := range shards {
		var summary runtests.TestSummary
		for _, test := range shard.Tests {
			if test.Affected {
				return nil, errSkippedAffectedTest
			}
			summary.Tests = append(summary.Tests, runtests.TestDetails{
				Name:        test.Name,
				GNLabel:     test.Label,
				SourceLabel: test.SourceLabel,
				Status:      runtests.TestSkipped,
				Tags:        test.Tags,
			})
		}
		newShard := *shard
		newShard.Skippable = true
		newShard.Summary = summary
		newShards = append(newShards, &newShard)
	}
	return newShards, nil
}

// AddSummariesToShards adds a TestSummary to all the shards
// in a given list of shards.
func AddSummariesToShards(shards []*Shard) []*Shard {
	var newShards []*Shard
	for _, shard := range shards {
		var summary runtests.TestSummary
		for _, test := range shard.Tests {
			summary.Tests = append(summary.Tests, runtests.TestDetails{
				Name:        test.Name,
				GNLabel:     test.Label,
				SourceLabel: test.SourceLabel,
				Status:      runtests.TestSkipped,
				Tags:        test.Tags,
			})
		}
		newShard := *shard
		newShard.SummaryIfSkipped = summary
		newShards = append(newShards, &newShard)
	}
	return newShards
}

func divRoundUp(a, b int) int {
	if a%b == 0 {
		return a / b
	}
	return (a / b) + 1
}

// WithTargetDuration returns a list of shards such that all shards are expected
// to complete in approximately `targetDuration` time.
// If targetDuration <= 0, just returns its input.
// Alternatively, accepts a `targetTestCount` argument for backwards compatibility.
//
// Each resulting shard will have a TimeoutSecs field populated dynamically
// based on the expected total runtime of its tests. The caller can choose to
// respect and enforce this timeout, or ignore it.
func WithTargetDuration(
	shards []*Shard,
	targetDuration time.Duration,
	targetTestCount,
	maxShardSize,
	maxShardsPerEnvironment int,
	testDurations TestDurationsMap,
) ([]*Shard, time.Duration) {
	if targetDuration <= 0 && targetTestCount <= 0 && maxShardSize <= 0 {
		return shards, targetDuration
	}
	if maxShardsPerEnvironment <= 0 {
		maxShardsPerEnvironment = math.MaxInt64
	}

	shardsPerEnv := make(map[string][]*Shard)
	targetDurationPerEnv := make(map[string]time.Duration)
	for _, shard := range shards {
		envName := environmentName(shard.Env)
		shardsPerEnv[envName] = append(shardsPerEnv[envName], shard)
		targetDurationPerEnv[envName] = targetDuration
	}

	largestTargetDuration := time.Duration(0)
	if targetDuration > 0 {
		for env, shards := range shardsPerEnv {
			totalTestsPerEnv := 0
			minTargetDurationForEnv := time.Duration(0)
			targetDurationForEnv := targetDuration
			var shardDuration time.Duration
			for _, shard := range shards {
				// If any single test is expected to take longer than `targetDuration`,
				// it's no use creating shards whose entire expected runtimes are
				// shorter than that one test. So in that case we use the longest test's
				// expected duration as the target duration.
				for _, t := range shard.Tests {
					duration := testDurations.Get(t).MedianDuration
					if duration > targetDurationForEnv {
						minTargetDurationForEnv = duration
						targetDurationForEnv = duration
					}
					shardDuration += duration * time.Duration(t.minRequiredRuns())
				}
				totalTestsPerEnv += len(shard.Tests)
			}

			// If any environment would exceed the maximum shard count (either because the total
			// shard duration is too long or the total test count is too big), then its
			// shard durations will exceed the specified target duration. So
			// increase the target duration accordingly.
			subShardCount := divRoundUp(int(shardDuration), int(targetDurationForEnv))
			if subShardCount > maxShardsPerEnvironment {
				subShardCount = maxShardsPerEnvironment
			} else if subShardCount > 0 {
				// If the number of shards does not exceed the max shards per env,
				// then check that the average tests per shards wouldn't exceed
				// the max shard size. If it does, increase the expected number
				// of shards to try to respect the maxShardSize as long as it fits
				// within the max shards per env.
				avgTestsPerShard := divRoundUp(totalTestsPerEnv, subShardCount)
				if maxShardSize > 0 && avgTestsPerShard > maxShardSize {
					subShardCount = min(divRoundUp(totalTestsPerEnv, maxShardSize), maxShardsPerEnvironment)
				}
			}
			if subShardCount > 0 {
				newTargetDurationForEnv := time.Duration(divRoundUp(int(shardDuration), subShardCount))
				// In the case where maxShardSize > 0 and we try to increase the number of shards,
				// the new target duration would be decreased. However, we still need to make sure
				// it's greater than the minimum reuired target duration.
				if newTargetDurationForEnv > minTargetDurationForEnv {
					targetDurationForEnv = newTargetDurationForEnv
				}
			}
			if targetDurationForEnv > largestTargetDuration {
				largestTargetDuration = targetDurationForEnv
			}
			targetDurationPerEnv[env] = targetDurationForEnv
		}
	}

	output := make([]*Shard, 0, len(shards))
	for _, shard := range shards {
		numNewShards := 0
		totalTests := 0
		for _, t := range shard.Tests {
			totalTests += t.minRequiredRuns()
		}
		if targetDuration > 0 {
			var targetDurationForShard time.Duration
			if maxShardSize > 0 {
				// If we're limiting by max shard size, then the target duration per env might be different.
				targetDurationForShard = targetDurationPerEnv[environmentName(shard.Env)]
			} else {
				// Otherwise use the largest duration size.
				targetDurationForShard = largestTargetDuration
			}
			var total time.Duration
			for _, t := range shard.Tests {
				total += testDurations.Get(t).MedianDuration * time.Duration(t.minRequiredRuns())
			}
			numNewShards = divRoundUp(int(total), int(targetDurationForShard))
			// While calculating the targetDurationForShard, some rounding might happen
			// that could cause it to be slightly less than the duration of a test, which
			// could cause numNewShards to be greater than the number of tests in the shard,
			// so set it to the number of tests instead so that at least one test is run
			// per shard.
			numNewShards = min(totalTests, numNewShards)
		} else {
			numNewShards = divRoundUp(totalTests, targetTestCount)
		}
		if numNewShards == 0 {
			// If targetDuration is set but all durations are zero, we'll
			// determine that we need zero new shards. In this case, we'll be
			// careful and assume that we need the maximum allowed shards to be
			// able to fit all tests or one shard per test if the number of tests
			// is less than the maximum allowed shards.
			numNewShards = min(totalTests, maxShardsPerEnvironment)
		}
		numNewShards = min(numNewShards, maxShardsPerEnvironment)

		newShards := shardByTime(shard, testDurations, numNewShards)
		output = append(output, newShards...)
	}
	return output, targetDuration
}

type subshard struct {
	duration              time.Duration
	tests                 []Test
	targetStartupDuration time.Duration
}

// A subshardHeap is a min heap of subshards, using subshard duration as the key
// to sort by.
// It implements heap.Interface.
type subshardHeap []subshard

func (h subshardHeap) Len() int {
	return len(h)
}

func (h subshardHeap) Less(i, j int) bool {
	if h[i].duration != h[j].duration {
		return h[i].duration < h[j].duration
	}
	// All durations being equal, fall back to comparing test counts. This
	// ensures that even if all expected durations are zero (which generally
	// shouldn't happen, but is possible), we'll still divide tests evenly by
	// test count across shards.
	return len(h[i].tests) < len(h[j].tests)
}

func (h subshardHeap) Swap(i, j int) {
	h[i], h[j] = h[j], h[i]
}

func (h *subshardHeap) Push(s any) {
	*h = append(*h, s.(subshard))
}

func (h *subshardHeap) Pop() any {
	old := *h
	n := len(old)
	s := old[n-1]
	*h = old[0 : n-1]
	return s
}

func getTargetStartupDuration(shardEnv build.Environment) time.Duration {
	if shardEnv.Dimensions.DeviceType() == "" || shardEnv.TargetsEmulator() {
		return 0
	} else if shardEnv.Dimensions.DeviceType() == "Sorrel" {
		// TODO(https://fxbug.dev/493277370): Sorrels can take longer to flash
		// when USB hubs are busy.
		return 20 * time.Minute
	}
	return 10 * time.Minute
}

// shardByTime breaks a single original shard into numNewShards subshards such
// that each subshard has approximately the same expected total duration.
//
// It does this using a greedy approximation algorithm for static multiprocessor
// scheduling (https://en.wikipedia.org/wiki/Multiprocessor_scheduling). It
// first sorts the tests in descending order by expected duration and then
// successively allocates each test to the subshard with the lowest total
// expected duration so far.
//
// Within each returned shard, tests will be sorted pseudo-randomly.
func shardByTime(shard *Shard, testDurations TestDurationsMap, numNewShards int) []*Shard {
	sort.Slice(shard.Tests, func(index1, index2 int) bool {
		test1, test2 := shard.Tests[index1], shard.Tests[index2]
		duration1 := testDurations.Get(test1).MedianDuration
		duration2 := testDurations.Get(test2).MedianDuration
		if duration1 == duration2 {
			// Sort by name for tests of equal duration to ensure deterministic
			// ordering.
			return test1.Name < test2.Name
		}
		// "greater than" instead of "less than" to achieve descending ordering
		return duration1 > duration2
	})

	var h subshardHeap
	for i := 0; i < numNewShards; i++ {
		// Initialize each subshard to have an empty list of tests so that it will
		// be properly outputted in the json output file as a list instead of a nil
		// value if the shard ends up with zero tests.
		s := subshard{tests: []Test{}, targetStartupDuration: getTargetStartupDuration(shard.Env)}
		h = append(h, s)
	}

	for _, test := range shard.Tests {
		shardsPerTest := 1
		// Only if the test must be run more than once and it's the only test
		// in the shard should we split it across multiple shards.
		splitAcrossShards := test.minRequiredRuns() > 1 && len(shard.Tests) == 1
		if splitAcrossShards {
			shardsPerTest = numNewShards
		}
		runsPerShard := divRoundUp(test.minRequiredRuns(), shardsPerTest)
		extra := runsPerShard*shardsPerTest - test.minRequiredRuns()
		for i := 0; i < shardsPerTest; i++ {
			testCopy := test
			var runs int
			if i < numNewShards-extra {
				runs = runsPerShard
			} else {
				runs = runsPerShard - 1
			}
			if runs == 0 {
				break
			}
			if splitAcrossShards {
				testCopy.Runs = runs
			}
			// Assign this test to the subshard with the lowest total expected
			// duration at this iteration of the for loop.
			ss := heap.Pop(&h).(subshard)
			ss.duration += testDurations.Get(test).MedianDuration * time.Duration(testCopy.minRequiredRuns())
			ss.tests = append(ss.tests, testCopy)
			heap.Push(&h, ss)
		}
	}

	// Sort the resulting shards by the basename of the first test. Otherwise,
	// changes to the input set of tests (adding, removing or renaming a test)
	// result in confusing reordering of the shard names. This ensures that a
	// given named shard (e.g. "QEMU-(1)") will contain roughly the same set of
	// longer-running tests across multiple builds, even if the input set of
	// tests changes. Shorter tests are more likely to be switched between
	// shards because we're sorting by the name of the longest test.
	sort.Slice(h, func(i, j int) bool {
		return h[i].tests[0].Name < h[j].tests[0].Name
	})

	newShards := make([]*Shard, 0, numNewShards)
	for i, subshard := range h {
		// If we left tests in descending order by duration, updates to
		// checked-in test durations could arbitrarily reorder tests, making
		// those updates more likely to break non-hermetic tests.
		// To avoid that, sort by a hash of the name so that similar tests (e.g.
		// all tests from one area of the codebase, or all host-side tests)
		// tests don't always run sequentially, since similar tests are more
		// likely to non-hermetically conflict with each other. Using a
		// deterministic hash ensures that given tests A and B in the same
		// shard, A will *always* run before B or vice versa.
		sort.Slice(subshard.tests, func(i, j int) bool {
			return hash(subshard.tests[i].Name) < hash(subshard.tests[j].Name)
		})
		name := shard.Name
		if numNewShards > 1 {
			name = fmt.Sprintf("%s-(%d)", shard.Name, i+1)
		}
		newShard := *shard
		newShard.Name = name
		newShard.Tests = subshard.tests
		newShard.TimeoutSecs = int(computeShardTimeout(subshard).Seconds())
		newShards = append(newShards, &newShard)
	}
	return newShards
}

// computeShardTimeout dynamically calculates an appropriate timeout for the
// given shard based on the number of tests it runs and the expected duration of
// each test, with a large buffer to account for tests occasionally taking much
// longer than expected.
//
// The constants we use to compute the timeout are somewhat arbitrary and can be
// adjusted if necessary.
func computeShardTimeout(s subshard) time.Duration {
	// Allow for tests running longer than expected by a constant factor. It's
	// unlikely that *every* test would be this much slower than expected, but
	// it's conceivable that a significant number of tests could be so affected.
	perTestMultiplier := time.Duration(2)
	// Conservative estimated overhead for the shard independent of how many
	// tests the shard runs.
	shardOverhead := 10 * time.Minute
	// Conservative estimate of the overhead for running each test.
	perTestOverhead := time.Second

	testCount := time.Duration(len(s.tests))

	// Take into account the expected overhead if a test times out to make sure
	// that the shard timeout leaves time for individual tests to time out. This
	// is determined by taking the maximum test timeout minus the average
	// expected test runtime, and doubling it if the test is allowed to run
	// multiple times or if there are multiple tests.
	longestTimeoutTest := s.tests[0]
	for _, test := range s.tests {
		if test.Timeout > longestTimeoutTest.Timeout {
			longestTimeoutTest = test
		}
	}
	avgDuration := s.duration / testCount
	testTimeoutOverhead := longestTimeoutTest.Timeout - avgDuration
	if longestTimeoutTest.Runs > 1 || len(s.tests) > 1 {
		// Only add enough overhead to guarantee two test runs enough
		// time to time out. If we allowed enough overhead for *all*
		// tests to time out then the task timeout would be way too
		// high. If all tests are timing out we should cancel the shard
		// early rather than waiting for all the tests to finish, which
		// would be wasteful.
		testTimeoutOverhead *= 2
	}

	timeout := perTestMultiplier * (s.duration + testCount*perTestOverhead)
	if testTimeoutOverhead > 0 {
		timeout += testTimeoutOverhead
	}
	// Include the estimated target startup duration in the timeout,
	// multiplied by 2 attempts.
	timeout += shardOverhead + 2*s.targetStartupDuration
	return timeout
}

func hash(s string) uint32 {
	h := fnv.New32a()
	h.Write([]byte(s))
	return h.Sum32()
}

// Removes leading slashes and replaces all other `/` with `_`. This allows the
// shard name to appear in filepaths.
func normalizeTestName(name string) string {
	trimmedName := strings.TrimLeft(name, "/")
	return strings.ReplaceAll(trimmedName, "/", "_")
}

// ApplyTestTimeouts sets the timeout field on every test to the specified
// duration. Timeouts already declared for tests in tests.json take precedence.
func ApplyTestTimeouts(shards []*Shard, perTestTimeout time.Duration) {
	for _, shard := range shards {
		for i := range shard.Tests {
			testTimeoutSecs := shard.Tests[i].Test.TimeoutSecs
			if testTimeoutSecs == 0 {
				shard.Tests[i].Timeout = perTestTimeout
			} else {
				shard.Tests[i].Timeout = time.Duration(testTimeoutSecs) * time.Second
			}
		}
	}
}
