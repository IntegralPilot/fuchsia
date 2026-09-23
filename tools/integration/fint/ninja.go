// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fint

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/build"
	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/streams"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
)

var (
	// explainRegex matches a singular line of Ninja explain stdout,
	// e.g. "ninja explain: host_x64/pm is dirty"
	explainRegex = regexp.MustCompile(`^\s*ninja explain:.*`)

	// Explicitly format Ninja stdout lines via the NINJA_STATUS
	// environment variable.  %f=finished, %t=remaining, %r=running
	ninjaStatus = "[%f/%t](%r) "

	// noWorkString in the Ninja output indicates a null build (i.e. all the
	// requested targets have already been built).
	noWorkString = "\nninja: no work to do."

	// Allow dirty no-op builds, but only if they appear to be failing on these
	// paths on Mac where the filesystem has a bug that causes it to erroneously
	// report that system files do not exist when referenced via relative paths.
	// See https://fxbug.dev/42140108.
	brokenMacPaths = []string{
		"/usr/bin/env",
		"/bin/ln",
		"/bin/bash",
		"/bin/sh",
		"/dev/zero",
	}

	// The following tests should never be considered affected. These tests use
	// a system image as data, so they appear affected by a broad range of
	// changes, but they're almost never actually sensitive to said changes.
	// https://fxbug.dev/42146209 tracks generating this list automatically.
	neverAffectedTestLabels = []string{
		"//src/recovery/simulator:recovery_simulator_boot_test",
		"//src/recovery/simulator:recovery_simulator_serial_test",
	}
)

const (
	// ninjaLogPath is the path to the main ninja log relative to the build directory.
	ninjaLogPath = ".ninja_log"

	// ninjaDepsPath is the path to the log of ninja deps relative to the build
	// directory.
	ninjaDepsPath = ".ninja_deps"

	// unrecognizedFailureMsg is the message we'll output if ninja fails but its
	// output doesn't match any of the known failure modes.
	unrecognizedFailureMsg = "Unrecognized failures, please check the original stdout instead."

	// ninjaEdgeWeightsArg is the arg to pass to ninja to use the ninja edge weights
	// file created by the regeneration script.
	// LINT.IfChange(edge_weights_file)
	ninjaEdgeWeightsArg = "--edge_weights_list=ninja_edge_weights.csv"
	// LINT.ThenChange(//tools/devshell/lib/vars.sh)
)

// ninjaRunner provides logic for running ninja commands using common flags
// (e.g. build directory name).
type ninjaRunner struct {
	runner    subprocessRunner
	ninjaPath string
	buildDir  string
	jobCount  int
}

// run runs a ninja command as a subprocess, passing `args` in addition to the
// common args configured on the ninjaRunner.
//
// Its first return value is a verbose failure message extracted from the ninja
// failure log and stderr. If ninja exits successfully, this will be empty.
func (r ninjaRunner) run(ctx context.Context, args []string, stdout, stderr io.Writer) (string, error) {
	cmd := []string{r.ninjaPath, "-C", r.buildDir}
	if r.jobCount > 0 {
		cmd = append(cmd, "-j", fmt.Sprintf("%d", r.jobCount))
	}

	// Tell ninja to source edge weights from a GN-generated file of estimates
	// that come from GN metadata on the actions.
	cmd = append(cmd, ninjaEdgeWeightsArg)

	// Write ninja errors to a temporary file so it doesn't get persisted
	// between builds in infrastructure.
	tmpDir, err := os.MkdirTemp("", "ninja_errors_*")
	if err != nil {
		return "", fmt.Errorf("creating temp dir for ninja errors: %w", err)
	}
	defer os.RemoveAll(tmpDir)
	errorsFileName := filepath.Join(tmpDir, "ninja_errors.json")
	cmd = append(cmd, fmt.Sprintf("--error_logging_output=%s", errorsFileName))

	cmd = append(cmd, args...)

	var stderrBuf bytes.Buffer
	multiStderr := io.MultiWriter(stderr, &stderrBuf)

	runErr := r.runner.Run(ctx, cmd, subprocess.RunOptions{Stdout: stdout, Stderr: multiStderr, Env: []string{
		fmt.Sprintf("NINJA_STATUS=%s", ninjaStatus),
	}})

	if runErr != nil {
		failureMsg, err := ninjaFailureMessage(ctx, errorsFileName, stderrBuf.String())
		if err != nil {
			return "", err
		}
		return failureMsg, runErr
	}

	return "", nil
}

// ninjaExplainExtractor is a writer that removes all Ninja explain outputs
// before writing to the underlying writer. If explainSink is provided, explain
// output is copied to it.
type ninjaExplainExtractor struct {
	buf         *bytes.Buffer
	w           io.Writer
	explainSink io.Writer
}

// Write implements io.Writer for ninjaExplainExtractor.
func (w *ninjaExplainExtractor) Write(bs []byte) (int, error) {
	if _, err := w.buf.Write(bs); err != nil {
		return 0, err
	}
	for {
		line, err := w.buf.ReadBytes('\n')
		// Put incomplete lines back to buffer.
		if errors.Is(err, io.EOF) {
			w.buf.Write(line)
			break
		}
		if err != nil {
			return 0, err
		}
		if !explainRegex.MatchString(string(line)) {
			w.w.Write(line)
		} else if w.explainSink != nil {
			w.explainSink.Write(line)
		}
	}
	return len(bs), nil
}

// Flush empties the internal buffer and forward non-ninja-explain lines.
func (w *ninjaExplainExtractor) Flush() error {
	for {
		line, err := w.buf.ReadBytes('\n')
		if err != nil && !errors.Is(err, io.EOF) {
			return err
		}
		if !explainRegex.MatchString(string(line)) {
			w.w.Write(line)
		} else if w.explainSink != nil {
			w.explainSink.Write(line)
		}
		// The last line may not finish with a '\n', so handle it before breaking.
		if errors.Is(err, io.EOF) {
			break
		}
	}
	return nil
}

// newNinjaExplainExtractor returns a writer that strips all Ninja explain
// outputs and forwards the rest to input writer. If explainSink is provided,
// explain output is copied to it.
func newNinjaExplainExtractor(w io.Writer, explainSink io.Writer) *ninjaExplainExtractor {
	return &ninjaExplainExtractor{
		buf:         new(bytes.Buffer),
		w:           w,
		explainSink: explainSink,
	}
}

// ninjaFailureLog represents the top-level structure of .ninja_errors.json.
//
// The schema is documented here:
// https://fuchsia.googlesource.com/third_party/github.com/ninja-build/ninja/+/8ffce4dbe12ce518cb21c70c4058039e737be28c/src/status_to_error_log.h#29
type ninjaFailureLog struct {
	Version  int            `json:"version"`
	Failures []ninjaFailure `json:"failures"`
}

// ninjaFailure represents a single failure as output in .ninja_errors.json
type ninjaFailure struct {
	Artifacts []string `json:"artifacts"`
	ExitCode  int      `json:"exit_code"`
	Output    string   `json:"output"`
}

type ninjaActionMetrics struct {
	InitialActions int32            `json:"initial_actions"`
	FinalActions   int32            `json:"final_actions"`
	ActionCounts   map[string]int32 `json:"action_counts"`
}

// runNinja runs ninja as a subprocess to build the specified targets.
func runNinja(
	ctx context.Context,
	r ninjaRunner,
	ninjaArgs []string,
	targets []string,
	explain bool,
	explainSink io.Writer,
) (string, *fintpb.NinjaActionMetrics, error) {
	if explain {
		targets = append(targets, "-d", "explain")
	}

	stdout := newNinjaExplainExtractor(streams.Stdout(ctx), explainSink)
	stderr := newNinjaExplainExtractor(streams.Stderr(ctx), explainSink)
	failureMsg, err := r.run(
		ctx,
		append(ninjaArgs, targets...),
		stdout,
		stderr,
	)

	if flushErr := stdout.Flush(); flushErr != nil {
		return "", nil, fmt.Errorf("flushing stdout writer: %w", flushErr)
	}
	if flushErr := stderr.Flush(); flushErr != nil {
		return "", nil, fmt.Errorf("flushing stderr writer: %w", flushErr)
	}

	var metrics *fintpb.NinjaActionMetrics
	metricsPath := filepath.Join(r.buildDir, actionMetricsName)
	var am ninjaActionMetrics
	if jsonErr := jsonutil.ReadFromFile(metricsPath, &am); jsonErr == nil {
		metrics = &fintpb.NinjaActionMetrics{
			InitialActions: am.InitialActions,
			FinalActions:   am.FinalActions,
			ActionsByType:  am.ActionCounts,
		}
	} else if !errors.Is(jsonErr, os.ErrNotExist) {
		return "", nil, fmt.Errorf("reading action metrics file %s: %w", metricsPath, jsonErr)
	}

	if err != nil {
		return failureMsg, metrics, err
	}

	// No failure message necessary if Ninja succeeded.
	return "", metrics, nil
}

func ninjaFailureMessage(ctx context.Context, errorsFileName string, ninjaStderr string) (string, error) {
	var failureLog ninjaFailureLog
	parseErr := jsonutil.ReadFromFile(errorsFileName, &failureLog)
	if parseErr != nil {
		logger.Warningf(ctx, "Ninja failed but ninja errors file is not parseable: %s", parseErr)
	} else if failureLog.Version != 1 {
		return "", fmt.Errorf("unsupported ninja failure log version: %d", failureLog.Version)
	}

	if len(failureLog.Failures) == 0 {
		// Ninja failed but didn't report any failures in the JSON file,
		// could be a configuration error (e.g. duplicate rule).
		failureMsg := strings.TrimSpace(ninjaStderr)
		if failureMsg == "" {
			// Only show the parsing error if there is no other information in
			// the stderr. Often parsing ninja_errors.json fails because ninja
			// failed at an earlier step (e.g. detecting a dependency cycle)
			// before generating the file and we want to show the original ninja
			// error (which should be in stderr) instead of complaining about
			// the file being malformed.
			if parseErr != nil {
				return "", fmt.Errorf("failed to read ninja errors file: %w", parseErr)
			}
			failureMsg = unrecognizedFailureMsg
		}
		failureMsg += "\n"
		return failureMsg, nil
	}

	var msgLines []string
	seenOutputs := make(map[string]bool)
	for _, f := range failureLog.Failures {
		// Sometimes multiple actions fail with the same output (e.g. they try
		// to compile the same file and run into the same error mode).
		// Deduplicate them to avoid cluttering the failure message. Only
		// deduplicate if the output is more than 5 lines long, to avoid
		// deduplicating multiple unrelated failures that happen to have the
		// same short output. The goal is to deduplicate compiler error messages
		// that point to a specific line, while not deduplicating generic error
		// messages that may have multiple causes.
		if f.Output != "" && strings.Count(f.Output, "\n") >= 5 {
			if seenOutputs[f.Output] {
				continue
			}
			seenOutputs[f.Output] = true
		}
		msgLines = append(msgLines, fmt.Sprintf("FAILED: [code=%d] %s", f.ExitCode, strings.Join(f.Artifacts, " ")))
		if f.Output != "" {
			// Add a blank line to make it easier to distinguish the header
			// (which can be long and wrapped across multiple lines) from the
			// output containing the actual error message.
			msgLines = append(msgLines, "")
			msgLines = append(msgLines, strings.Trim(f.Output, "\n"))
		}
		msgLines = append(msgLines, "\n")
	}
	return strings.Join(msgLines, "\n"), nil
}

// ninjaDryRun does a `ninja explain` dry run against a build directory and
// returns the stdout and stderr.
func ninjaDryRun(ctx context.Context, r ninjaRunner, targets []string, dirtySourcesListPath string) (string, string, error) {
	// -n means dry-run.
	args := []string{"-d", "explain", "--verbose", "-n"}
	if dirtySourcesListPath != "" {
		args = append(args, "--dirty_sources_list", dirtySourcesListPath)
	}
	args = append(args, targets...)

	var stdout, stderr bytes.Buffer
	_, err := r.run(ctx, args, &stdout, &stderr)
	if err != nil {
		// stdout and stderr are normally not emitted because they're very
		// noisy, but if the dry run fails then they'll likely contain the
		// information necessary to understand the failure.
		streams.Stdout(ctx).Write(stdout.Bytes())
		streams.Stderr(ctx).Write(stderr.Bytes())
	}
	return stdout.String(), stderr.String(), err
}

// checkNinjaNoop runs `ninja explain` against a build directory to determine
// whether an incremental build would be a no-op (i.e. all requested targets
// have already been built). It returns true if the build would be a no-op,
// false otherwise.
//
// It also returns the first line of ninja's output, which often contains a
// useful message, and a map of logs produced by the no-op check, which can be
// presented to the user for help with debugging in case the check fails.
func checkNinjaNoop(
	ctx context.Context,
	r ninjaRunner,
	targets []string,
	isMac bool,
) (bool, string, map[string]string, error) {
	dirtySourcesFile, err := os.CreateTemp("", "dirty_sources_list")
	if err != nil {
		return false, "", nil, fmt.Errorf("failed to create temporary file for dirty sources list: %w", err)
	}
	dirtySourcesPath := dirtySourcesFile.Name()
	dirtySourcesFile.Close()
	defer os.Remove(dirtySourcesPath)

	stdout, stderr, ninjaErr := ninjaDryRun(ctx, r, targets, dirtySourcesPath)
	// Temporarily tolerate a failure if it's on Mac. We won't emit the error if
	// it seemed to be caused by a known broken Mac path.
	if ninjaErr != nil && !isMac {
		return false, "", nil, ninjaErr
	}

	// Different versions of Ninja choose to emit "explain" logs to stderr
	// instead of stdout, so we want to analyze both streams.
	// Concatenate the two streams for simplicity so that we don't need to do
	// the same operation separately on each stream.
	allStdio := strings.Join([]string{stdout, stderr}, "\n\n")
	if !strings.Contains(allStdio, noWorkString) {
		if isMac {
			// TODO(https://fxbug.dev/42140108): Dirty builds should be an error even on Mac.
			for _, path := range brokenMacPaths {
				if strings.Contains(allStdio, path) {
					return true, "", nil, nil
				}
			}
		}
		logs := map[string]string{
			"`ninja -d explain -v -n` stdout": stdout,
			"`ninja -d explain -v -n` stderr": stderr,
		}

		if content, err := os.ReadFile(dirtySourcesPath); err == nil && len(content) > 0 {
			logs["dirty sources list"] = string(content)
		}

		// Return the original ninja error, which may be non-nil if we're
		// running on a Mac and the dry run failed but the stdio didn't contain
		// one of the broken Mac paths.
		noopMsg := strings.Split(stderr, "\n")[0]
		noopMsg = strings.TrimPrefix(noopMsg, "ninja explain: ")
		return false, noopMsg, logs, ninjaErr
	}

	return true, "", nil, nil
}

// touchFiles updates the modified time on all the specified files to the
// current timestamp, skipping any nonexistent files.
// Returns a map of paths touched to their previous stats.
// This map can be passed to resetTouchedFiles to revert the operation.
func touchFiles(paths []string) (map[string]time.Time, error) {
	reset := make(map[string]time.Time)
	now := time.Now()
	for _, path := range paths {
		stat, err := os.Stat(path)
		if err != nil {
			// Skip any paths that don't exist, e.g. because the file was deleted in
			// the change under test.
			if os.IsNotExist(err) {
				continue
			}
			return nil, err
		}
		// Note that we can't get access time in a platform-agnostic way.
		// We end up coupling mtime with atime, even after a reset.
		reset[path] = stat.ModTime()
		if err := os.Chtimes(path, now, now); err != nil {
			return nil, err
		}
	}
	return reset, nil
}

// Rolls back changes made by a previous call to touchFiles.
func resetTouchFiles(touchFilesResult map[string]time.Time) error {
	for path, mtime := range touchFilesResult {
		if err := os.Chtimes(path, mtime, mtime); err != nil {
			return err
		}
	}
	return nil
}

// affectedTestsResult is the type emitted by `affectedTestsNoWork()`. It exists
// solely to keep return statements in that function concise.
type affectedTestsResult struct {
	// Names of tests that are affected based on the paths of the changed files.
	affectedTests []string

	// Whether the build graph is unaffected by the changed files.
	noWork bool

	// Keep track of logs so the caller can choose to present them to the user
	// for debugging purposes.
	logs map[string]string
}

// affectedTestsNoWork touches affected files and then does a ninja dry run and
// analyzes the output, to determine:
// a) If the build graph is affected by the changed files.
// b) If so, which tests are affected by the changed files.
func affectedTestsNoWork(
	ctx context.Context,
	runner ninjaRunner,
	contextSpec *fintpb.Context,
	allTests []build.Test,
	targets []string,
) (affectedTestsResult, error) {
	result := affectedTestsResult{
		logs: map[string]string{},
	}

	// Map from "... is dirty" line printed by Ninja to affected test
	testsByDirtyLine := map[string][]string{}
	// Map from test path (if defined) to test name
	testsByPath := map[string]string{}
	// Map from path to BUILD.gn file defining the test to the test
	testsByBuildGn := map[string][]string{}

	for _, test := range allTests {
		// Ignore any tests that shouldn't be considered affected.
		labelNoToolchain := strings.Split(test.Label, "(")[0]
		if slices.Contains(neverAffectedTestLabels, labelNoToolchain) {
			continue
		}

		// For host tests we use the executable path.
		if test.Path != "" {
			testsByPath[test.Path] = test.Name
		}

		for _, packageManifest := range test.PackageManifests {
			dirtyLine := dirtyLineForPackageManifest(packageManifest)
			testsByDirtyLine[dirtyLine] = append(testsByDirtyLine[dirtyLine], test.Name)
		}

		buildGnPath := buildGnPathForLabel(test.Label)
		testsByBuildGn[buildGnPath] = append(testsByBuildGn[buildGnPath], test.Name)
		if test.PackageLabel != "" {
			buildGnPath = buildGnPathForLabel(test.PackageLabel)
			testsByBuildGn[buildGnPath] = append(testsByBuildGn[buildGnPath], test.Name)
		}
	}

	var gnFiles, nonGNFiles []string
	for _, f := range contextSpec.ChangedFiles {
		ext := filepath.Ext(f.Path)
		if ext == ".gn" || ext == ".gni" {
			gnFiles = append(gnFiles, f.Path)
		} else {
			nonGNFiles = append(nonGNFiles, f.Path)
		}
	}

	var affectedTests []string
	for _, gnFile := range gnFiles {
		gnFile = strings.TrimPrefix(gnFile, "build/secondary/")
		match, ok := testsByBuildGn[gnFile]
		if ok {
			affectedTests = append(affectedTests, match...)
		}
	}

	// Our Ninja graph is set up in such a way that touching any GN files
	// triggers an action to regenerate the entire graph. So if GN files were
	// modified and we touched them then the following dry run results are not
	// useful for determining affected tests.
	touchNonGNResult, err := touchFiles(makeAbsolute(contextSpec.CheckoutDir, nonGNFiles))
	if err != nil {
		return result, err
	}
	defer resetTouchFiles(touchNonGNResult)
	// TODO: Pass a real path here to capture dirty sources in the pre-build phase.
	stdout, stderr, err := ninjaDryRun(ctx, runner, targets, "")
	if err != nil {
		return result, err
	}
	ninjaOutput := strings.Join([]string{stdout, stderr}, "\n\n")

	for _, line := range strings.Split(ninjaOutput, "\n") {
		match, ok := testsByDirtyLine[line]
		if ok {
			// Matched an expected line
			affectedTests = append(affectedTests, match...)
		} else {
			// Look for actions that reference host test path. Different types
			// of host tests have different actions, but they all mention the
			// final executable path.
			// fxbug.dev(85524): tokenize with shlex in case test paths include
			// whitespace.
			for _, maybeTestPath := range strings.Split(line, " ") {
				maybeTestPath = strings.Trim(maybeTestPath, `"`)
				testName, ok := testsByPath[maybeTestPath]
				if !ok {
					continue
				}
				affectedTests = append(affectedTests, testName)
			}
		}
	}

	// For determination of "no work to do", we want to consider all files,
	// *including* GN files. If no GN files are affected, then we already have
	// the necessary output from the first ninja dry run, so we can skip doing
	// the second dry run that includes GN files.
	if len(gnFiles) > 0 {
		result.logs["ninja dry run output (no GN files)"] = ninjaOutput

		// Since we only did a Ninja dry run, the non-GN files will still be
		// considered dirty, so we need only touch the GN files.
		touchGNResult, err := touchFiles(makeAbsolute(contextSpec.CheckoutDir, gnFiles))
		if err != nil {
			return result, err
		}
		defer resetTouchFiles(touchGNResult)
		var stdout, stderr string
		// TODO: Pass a real path here to capture dirty sources in the pre-build phase.
		stdout, stderr, err = ninjaDryRun(ctx, runner, targets, "")
		if err != nil {
			return result, err
		}
		ninjaOutput = strings.Join([]string{stdout, stderr}, "\n\n")
	}
	result.logs["ninja dry run output"] = ninjaOutput
	result.noWork = strings.Contains(ninjaOutput, noWorkString)
	result.affectedTests = removeDuplicates(affectedTests)

	return result, nil
}

func dirtyLineForPackageManifest(label string) string {
	return "ninja explain: " + label + " is dirty"
}

func buildGnPathForLabel(label string) string {
	result := strings.TrimPrefix(label, "//")
	result = strings.Split(result, ":")[0]
	return path.Join(result, "BUILD.gn")
}

func runNinjatrace(ctx context.Context, runner subprocessRunner, ninjatraceToolPath string, ninjaTracePath string, traceJson string) error {
	cmd := []string{ninjatraceToolPath, "-ninjabuildtrace", ninjaTracePath, "-trace-json", traceJson}
	return runner.Run(ctx, cmd, subprocess.RunOptions{})
}

func runBuildstats(ctx context.Context, runner subprocessRunner, buildstatsToolPath string, ninjaTracePath string, statsOutput string) error {
	cmd := []string{buildstatsToolPath, "--ninjatrace", ninjaTracePath, "--output", statsOutput}
	return runner.Run(ctx, cmd, subprocess.RunOptions{})
}
