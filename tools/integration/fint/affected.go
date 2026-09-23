// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fint

import (
	"context"
	"encoding/json"
	"os"
	"slices"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/build"
	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/hostplatform"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

// Affected runs both the legacy ninja dry-run and the new build/api/client
// affected_tests logic, comparing their results side-by-side.
func Affected(ctx context.Context, staticSpec *fintpb.Static, contextSpec *fintpb.Context) (*fintpb.BuildArtifacts, error) {
	platform, err := hostplatform.Name()
	if err != nil {
		return nil, err
	}
	modules, err := build.NewModules(contextSpec.BuildDir)
	if err != nil {
		return nil, err
	}
	client, err := build.NewBuildAPIClient(contextSpec.BuildDir)
	if err != nil {
		return nil, err
	}
	ninjaTargets, _, err := constructNinjaTargets(modules, staticSpec, contextSpec, platform)
	if err != nil {
		return &fintpb.BuildArtifacts{}, err
	}
	artifacts, err := affectedImpl(ctx, newRunner(contextSpec), client, contextSpec, modules, platform, ninjaTargets)
	if err != nil && artifacts != nil && artifacts.FailureSummary == "" {
		// Fall back to using the error text as the failure summary if the
		// failure summary is unset. It's better than failing without emitting
		// any information.
		artifacts.FailureSummary = err.Error()
	}
	return artifacts, err
}

// affectedImpl contains the business logic of finding affected tests.
// It dual-runs both the legacy dry-run algorithm and the new build/api/client
// tool, outputting a side-by-side comparison report to log_files.
func affectedImpl(
	ctx context.Context,
	runner subprocessRunner,
	client buildAPIClient,
	contextSpec *fintpb.Context,
	modules buildModules,
	platform string,
	ninjaTargets []string,
) (*fintpb.BuildArtifacts, error) {
	artifacts := &fintpb.BuildArtifacts{}

	if contextSpec.ArtifactDir == "" || len(contextSpec.ChangedFiles) == 0 || len(modules.TestSpecs()) == 0 {
		return artifacts, nil
	}

	// 1. Run legacy logic via ninja dry-runs.
	ninjaPath, err := toolAbsPath(modules, platform, "ninja")
	if err != nil {
		return artifacts, err
	}
	r := ninjaRunner{
		runner:    runner,
		ninjaPath: ninjaPath,
		buildDir:  contextSpec.BuildDir,
		jobCount:  int(contextSpec.JobCount),
	}

	var tests []build.Test
	for _, t := range modules.TestSpecs() {
		tests = append(tests, t.Test)
	}

	startLegacy := time.Now()
	legacyResult, err := affectedTestsNoWork(ctx, r, contextSpec, tests, ninjaTargets)
	legacyDuration := time.Since(startLegacy)
	if err != nil {
		return artifacts, err
	}

	// 2. Run new logic via build/api/client affected_tests tool.
	var newAffectedTests []string
	var newBuildNotAffected bool
	var newToolErr error
	var newDuration time.Duration

	filesListPath, cleanup, err := writeChangedFilesList(ctx, contextSpec.ArtifactDir, contextSpec.ChangedFiles)
	if err != nil {
		newToolErr = err
	} else {
		defer cleanup()
		if client != nil {
			startNew := time.Now()
			outputLines, err := client.AffectedTests(ctx, filesListPath)
			newDuration = time.Since(startNew)
			if err != nil {
				newToolErr = err
			} else {
				newAffectedTests = resolveAffectedTestNames(modules.TestSpecs(), outputLines)
				newBuildNotAffected = len(newAffectedTests) == 0
			}
		}
	}

	// 3. Compare the two methods side-by-side.
	legacySet := make(map[string]struct{})
	for _, t := range legacyResult.affectedTests {
		legacySet[t] = struct{}{}
	}
	newSet := make(map[string]struct{})
	for _, t := range newAffectedTests {
		newSet[t] = struct{}{}
	}

	var onlyInLegacy, onlyInNew []string
	for _, t := range legacyResult.affectedTests {
		if _, ok := newSet[t]; !ok {
			onlyInLegacy = append(onlyInLegacy, t)
		}
	}
	for _, t := range newAffectedTests {
		if _, ok := legacySet[t]; !ok {
			onlyInNew = append(onlyInNew, t)
		}
	}

	if legacyResult.affectedTests == nil {
		legacyResult.affectedTests = []string{}
	}
	if newAffectedTests == nil {
		newAffectedTests = []string{}
	}
	if onlyInLegacy == nil {
		onlyInLegacy = []string{}
	}
	if onlyInNew == nil {
		onlyInNew = []string{}
	}

	comparisonReport := map[string]any{
		"legacy_affected_tests":     legacyResult.affectedTests,
		"new_affected_tests":        newAffectedTests,
		"only_in_legacy":            onlyInLegacy,
		"only_in_new":               onlyInNew,
		"legacy_build_not_affected": legacyResult.noWork,
		"new_build_not_affected":    newBuildNotAffected,
		"matches":                   len(onlyInLegacy) == 0 && len(onlyInNew) == 0 && (legacyResult.noWork == newBuildNotAffected),
		"legacy_duration_seconds":   legacyDuration.Seconds(),
		"new_duration_seconds":      newDuration.Seconds(),
	}
	if newToolErr != nil {
		comparisonReport["new_tool_error"] = newToolErr.Error()
	}

	logger.Infof(
		ctx,
		"Affected tests comparison: legacy_count=%d, new_count=%d, only_in_legacy=%d, only_in_new=%d, legacy_duration=%.2fs, new_duration=%.2fs",
		len(legacyResult.affectedTests),
		len(newAffectedTests),
		len(onlyInLegacy),
		len(onlyInNew),
		legacyDuration.Seconds(),
		newDuration.Seconds(),
	)

	if comparisonBytes, err := json.MarshalIndent(comparisonReport, "", "  "); err == nil {
		if legacyResult.logs == nil {
			legacyResult.logs = make(map[string]string)
		}
		legacyResult.logs["affected_tests_comparison.json"] = string(comparisonBytes)
	}

	if err := saveLogs(contextSpec.ArtifactDir, artifacts, legacyResult.logs); err != nil {
		return artifacts, err
	}

	// Keep legacy result as source of truth for now during comparison phase.
	artifacts.AffectedTests = legacyResult.affectedTests
	artifacts.BuildNotAffected = legacyResult.noWork
	return artifacts, nil
}

// writeChangedFilesList writes changed files to a temporary file in artifactDir, one per line.
// It returns the file path and a cleanup function to delete the temporary file.
func writeChangedFilesList(ctx context.Context, artifactDir string, changedFiles []*fintpb.Context_ChangedFile) (string, func(), error) {
	var paths []string
	for _, f := range changedFiles {
		paths = append(paths, f.Path)
	}

	if artifactDir != "" {
		if err := os.MkdirAll(artifactDir, 0o700); err != nil {
			return "", nil, err
		}
	}

	tmpFile, err := os.CreateTemp(artifactDir, "changed_files_*.txt")
	if err != nil {
		return "", nil, err
	}
	cleanup := func() {
		if err := os.Remove(tmpFile.Name()); err != nil {
			logger.Warningf(ctx, "failed to remove temporary file %s: %s", tmpFile.Name(), err)
		}
	}

	if _, err := tmpFile.WriteString(strings.Join(paths, "\n") + "\n"); err != nil {
		tmpFile.Close()
		cleanup()
		return "", nil, err
	}
	if err := tmpFile.Close(); err != nil {
		cleanup()
		return "", nil, err
	}

	return tmpFile.Name(), cleanup, nil
}

// resolveAffectedTestNames maps the target labels returned by build/api/client affected_tests
func resolveAffectedTestNames(testSpecs []build.TestSpec, outputLines []string) []string {
	testsByLabel := make(map[string][]string)
	testsByNoToolchainLabel := make(map[string][]string)

	for _, spec := range testSpecs {
		test := spec.Test
		labelNoToolchain := strings.Split(test.Label, "(")[0]
		if slices.Contains(neverAffectedTestLabels, labelNoToolchain) {
			continue
		}
		if test.Label != "" {
			testsByLabel[test.Label] = append(testsByLabel[test.Label], test.Name)
			testsByNoToolchainLabel[labelNoToolchain] = append(testsByNoToolchainLabel[labelNoToolchain], test.Name)
		}
		if test.PackageLabel != "" {
			testsByLabel[test.PackageLabel] = append(testsByLabel[test.PackageLabel], test.Name)
			pkgNoToolchain := strings.Split(test.PackageLabel, "(")[0]
			testsByNoToolchainLabel[pkgNoToolchain] = append(testsByNoToolchainLabel[pkgNoToolchain], test.Name)
		}
		if test.SourceLabel != "" {
			testsByLabel[test.SourceLabel] = append(testsByLabel[test.SourceLabel], test.Name)
			srcNoToolchain := strings.Split(test.SourceLabel, "(")[0]
			testsByNoToolchainLabel[srcNoToolchain] = append(testsByNoToolchainLabel[srcNoToolchain], test.Name)
		}
		testsByLabel[test.Name] = append(testsByLabel[test.Name], test.Name)
	}

	var affectedTests []string
	for _, line := range outputLines {
		targetLabel := strings.SplitN(line, ",", 2)[0]
		labelNoToolchain := strings.Split(targetLabel, "(")[0]
		if slices.Contains(neverAffectedTestLabels, labelNoToolchain) {
			continue
		}
		if names, ok := testsByLabel[targetLabel]; ok {
			affectedTests = append(affectedTests, names...)
		} else if names, ok := testsByNoToolchainLabel[labelNoToolchain]; ok {
			affectedTests = append(affectedTests, names...)
		} else {
			affectedTests = append(affectedTests, targetLabel)
		}
	}

	return removeDuplicates(affectedTests)
}
