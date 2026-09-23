// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testsharder

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"regexp"
	"slices"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

const (
	fuchsia = "fuchsia"
	linux   = "linux"
	x64     = "x64"

	// The maximum number of tests that a multiplier can match. testsharder will
	// fail if this is exceeded.
	defaultMaxMatchesPerMultiplier = 50
)

// matchModifiersToTests will return an error that unwraps to this if a multiplier's
// "name" field does not compile to a valid regex.
var errInvalidMultiplierRegex = fmt.Errorf("invalid multiplier regex")

// TestModifier is the specification for a single test and the number of
// times it should be run.
type TestModifier struct {
	// Name is the name of the test.
	Name string `json:"name"`

	// OS is the operating system in which this test must be executed. If not
	// present, this multiplier will match tests from any operating system.
	OS string `json:"os,omitempty"`

	// TotalRuns is the number of times to run the test. If zero, testsharder
	// will use historical test duration data to try to run this test along with
	// other multiplied tests as many times as it can within the max allowed
	// multiplied shards per environment. A negative value means to NOT designate
	// this test as a multiplier test and to leave the original runs as-is.
	TotalRuns int `json:"total_runs,omitempty"`

	// Affected specifies whether the test is an affected test. If affected,
	// it will be run in a separate shard than the unaffected tests.
	Affected bool `json:"affected,omitempty"`

	// MaxAttempts is the max number of times to run this test if it fails.
	// This is the max attempts per run as specified by the `TotalRuns` field.
	MaxAttempts int `json:"max_attempts,omitempty"`

	// MaxMatches is the max number of tests which can be matched by this modifier.
	// Defaults to defaultMaxMatchesPerMultiplier.
	MaxMatches int `json:"max_matches,omitempty"`
}

// ModifierMatch is the calculated match of a single test in a single environment
// with the modifier that it matches. After processing all modifiers, we should
// return a ModifierMatch for each test-env combination that the modifiers apply to.
// An empty Env means it matches all environments.
type ModifierMatch struct {
	Test     string
	Env      build.Environment
	Modifier TestModifier
}

// LoadTestModifiers loads a set of test modifiers from a json manifest.
func LoadTestModifiers(ctx context.Context, testSpecs []build.TestSpec, manifestPath string) ([]ModifierMatch, error) {
	bytes, err := os.ReadFile(manifestPath)
	if err != nil {
		return nil, err
	}
	var specs []TestModifier
	if err = json.Unmarshal(bytes, &specs); err != nil {
		return nil, err
	}

	for i := range specs {
		if specs[i].Name == "" {
			return nil, fmt.Errorf("A test spec's target must have a non-empty name")
		}
	}
	return matchModifiersToTests(ctx, testSpecs, specs)
}

// TODO(https://fxbug.dev/384562300): Remove dm_reboot_bringup_test.sh when flake is fixed.
// TODO(https://fxbug.dev/377625303): Remove kill_critical_process_test.sh when flake is fixed.
var knownFlakyTests = []string{
	"host_x64/obj/src/tests/reboot/dm_reboot_bringup_test/dm_reboot_bringup_test.sh",
	"host_x64/obj/src/tests/reboot/kill_critical_process_test.sh",
}

// AffectedModifiers returns modifiers for tests that are in both testSpecs and
// affectedTestNames.
// maxAttempts will be applied to any test that is not multiplied.
// Tests will be considered for multiplication only if num affected tests <= multiplyThreshold.
func AffectedModifiers(testSpecs []build.TestSpec, affectedTestNames []string, maxAttempts, multiplyThreshold int) ([]ModifierMatch, error) {
	nameToSpec := make(map[string]build.TestSpec)
	for _, ts := range testSpecs {
		nameToSpec[ts.Name] = ts
	}
	var ret []ModifierMatch
	if len(affectedTestNames) > multiplyThreshold {
		for _, name := range affectedTestNames {
			_, found := nameToSpec[name]
			if !found {
				continue
			}
			// Since we're not multiplying the tests, apply maxAttempts to them instead.
			ret = append(ret, ModifierMatch{
				Test: name,
				Modifier: TestModifier{
					Name:        name,
					TotalRuns:   -1,
					Affected:    true,
					MaxAttempts: maxAttempts,
				},
			})
		}
	} else {
		for _, name := range affectedTestNames {
			spec, found := nameToSpec[name]
			if !found {
				continue
			}
			// Only x64 Linux VMs are plentiful, don't multiply affected tests that
			// would require any other type of bot. Also, don't multiply isolated tests
			// because they are expected to be the only test running in its shard and
			// should only run once. Also don't multiply tests that are known to be flaky.
			if spec.CPU != x64 || (spec.OS != fuchsia && spec.OS != linux) || spec.Isolated || slices.Contains(knownFlakyTests, name) {
				ret = append(ret, ModifierMatch{
					Test: name,
					Modifier: TestModifier{
						Name:        name,
						TotalRuns:   -1,
						Affected:    true,
						MaxAttempts: maxAttempts,
					},
				})
				continue
			}
			for _, env := range spec.Envs {
				shouldMultiply := true
				if env.Dimensions.DeviceType() != "" && spec.OS != fuchsia {
					// Don't multiply host+target tests because they tend to be
					// flaky already. The idea is to expose new flakiness, not
					// pre-existing flakiness.
					shouldMultiply = false
				} else if env.Dimensions.DeviceType() != "" &&
					!strings.HasSuffix(env.Dimensions.DeviceType(), "EMU") {
					// Only x64 Linux VMs are plentiful, don't multiply affected
					// tests that would require any other type of bot.
					shouldMultiply = false
				}
				match := ModifierMatch{
					Test:     name,
					Env:      env,
					Modifier: TestModifier{Name: name, Affected: true},
				}
				if !shouldMultiply {
					match.Modifier.TotalRuns = -1
					match.Modifier.MaxAttempts = maxAttempts
				}
				ret = append(ret, match)
			}
		}
	}
	return ret, nil
}

// matchModifiersToTests analyzes the given modifiers against the testSpec to return
// modifiers that match tests exactly per allowed environment.
func matchModifiersToTests(ctx context.Context, testSpecs []build.TestSpec, modifiers []TestModifier) ([]ModifierMatch, error) {
	var ret []ModifierMatch
	var tooManyMatchesMultipliers []string
	for _, modifier := range modifiers {
		if modifier.Name == "*" {
			ret = append(ret, ModifierMatch{Modifier: modifier})
			continue
		}
		nameRegex, err := regexp.Compile(modifier.Name)
		var exactMatches []ModifierMatch
		var regexMatches []ModifierMatch
		numExactMatches := 0
		numRegexMatches := 0
		if err != nil {
			return nil, fmt.Errorf("%w %q: %s", errInvalidMultiplierRegex, modifier.Name, err)
		}
		for _, ts := range testSpecs {
			if nameRegex.FindString(ts.Name) == "" {
				continue
			}
			if modifier.OS != "" && modifier.OS != ts.OS {
				continue
			}

			isExactMatch := ts.Name == modifier.Name
			if len(ts.Envs) > 0 {
				if isExactMatch {
					numExactMatches += 1
				} else {
					numRegexMatches += 1
				}
			}
			for _, env := range ts.Envs {
				match := ModifierMatch{Test: ts.Name, Env: env, Modifier: modifier}
				if isExactMatch {
					exactMatches = append(exactMatches, match)
				} else {
					regexMatches = append(regexMatches, match)
				}
			}
		}
		// We'll consider partial regex matches only when we have no exact
		// matches.
		matches := exactMatches
		numMatches := numExactMatches
		if numMatches == 0 {
			matches = regexMatches
			numMatches = numRegexMatches
		}

		maxMatches := modifier.MaxMatches
		if maxMatches == 0 {
			maxMatches = defaultMaxMatchesPerMultiplier
		}
		if numMatches > maxMatches {
			tooManyMatchesMessage := fmt.Sprintf(
				"%s multiplier cannot match more than %d tests, matched %d",
				modifier.Name,
				maxMatches,
				numMatches,
			)
			tooManyMatchesMultipliers = append(tooManyMatchesMultipliers, tooManyMatchesMessage)
			logger.Errorf(ctx, "%s", tooManyMatchesMessage)
			continue
		}
		ret = append(ret, matches...)
	}
	if len(tooManyMatchesMultipliers) > 0 {
		return nil, errors.New(strings.Join(tooManyMatchesMultipliers, "\n"))
	}
	return ret, nil
}
