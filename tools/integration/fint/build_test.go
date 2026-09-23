// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fint

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"
	"github.com/kr/pretty"
	"golang.org/x/sys/unix"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/testing/protocmp"
	"google.golang.org/protobuf/types/known/structpb"

	"go.fuchsia.dev/fuchsia/tools/build"
	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
)

type fakeBuildModules struct {
	args             build.Args
	buildDir         string
	clippyTargets    []build.ClippyTarget
	generatedSources []string
	pbinSets         []build.PrebuiltBinarySet
	testSpecs        []build.TestSpec
	tools            build.Tools
}

func (m fakeBuildModules) Args() build.Args                              { return m.args }
func (m fakeBuildModules) BuildDir() string                              { return m.buildDir }
func (m fakeBuildModules) ClippyTargets() []build.ClippyTarget           { return m.clippyTargets }
func (m fakeBuildModules) GeneratedSources() []string                    { return m.generatedSources }
func (m fakeBuildModules) PrebuiltBinarySets() []build.PrebuiltBinarySet { return m.pbinSets }
func (m fakeBuildModules) TestSpecs() []build.TestSpec                   { return m.testSpecs }
func (m fakeBuildModules) Tools() build.Tools                            { return m.tools }

type fakeBuildAPIClient struct {
	affectedTests []string
}

func (c fakeBuildAPIClient) ExportDebugSymbols(ctx context.Context, dir string, withBreakpad bool) error {
	return os.WriteFile(filepath.Join(dir, "debug_symbols.json"), []byte(`[]`), 0o600)
}

func (c fakeBuildAPIClient) AffectedTests(ctx context.Context, filesList string) ([]string, error) {
	return c.affectedTests, nil
}

// An enum type describing the presence of a build success stamp file
// after a build. The BuildSuccessStampAuto value is the default and
// corresponds to BuildSuccessTampExists for a successful build,
// or BuildSuccessStampMissing for a failed one.
type BuildSuccessStampExpectation int32

const (
	BuildSuccessStampAuto BuildSuccessStampExpectation = iota
	BuildSuccessStampExists
	BuildSuccessStampMissing
)

func TestGetSubbuildSubdirs(t *testing.T) {
	tmpDir := t.TempDir()
	subbuildJSONFile := filepath.Join(tmpDir, "subbuilds.json")
	contents := []byte(`[
  {
    "build_dir": "subbuild.1"
  },
  {
    "build_dir": "subbuild.2"
  }
]`)
	if err := os.WriteFile(subbuildJSONFile, contents, 0o600); err != nil {
		t.Fatalf("Got unexpected error writing file: %s", err)
	}
	ctx := context.Background()
	gotSubdirs, err := getSubbuildSubdirs(ctx, subbuildJSONFile)
	if err != nil {
		t.Fatalf("Got unexpected error in getSubbuildSubdirs(): %s", err)
	}
	wantSubdirs := []string{"subbuild.1", "subbuild.2"}
	if diff := cmp.Diff(wantSubdirs, gotSubdirs, cmp.Options{}); diff != "" {
		t.Errorf("Got wrong subdirs (-want +got):\n%s", diff)
	}
}

func TestBuild(t *testing.T) {
	checkoutDir := t.TempDir()
	buildDir := filepath.Join(checkoutDir, "out", "default")
	artifactDir := t.TempDir()

	// The directories are shared across all test cases so they can be
	// referenced in the `testCases` table, but that means they don't get
	// cleared between sub-tests so we need to clear them explicitly.
	resetDirs := func(t *testing.T) {
		for _, dir := range []string{checkoutDir, buildDir, artifactDir} {
			if err := os.RemoveAll(dir); err != nil {
				t.Fatal(err)
			}
			if err := os.MkdirAll(dir, 0o700); err != nil {
				t.Fatal(err)
			}
		}
	}

	platform := "linux-x64"

	failedBuildRunner := func(cmd []string, _, stderr io.Writer) error {
		// Make ninja build fail, but ninjatrace succeed.
		prog := filepath.Base(cmd[0])
		if prog == "ninjatrace_prebuilt" || prog == "buildstats_prebuilt" {
			return nil
		}
		// prog == "ninja" or "rs-sub-ninja"
		if slices.Contains(cmd, "-t") { // for compdb and graph
			return nil
		}
		stderr.Write([]byte("ninja: error: simulated failure\n"))
		return fmt.Errorf("failed to run command: %s", cmd)
	}

	type subbuildFreshness struct {
		dir   string
		fresh bool // if true, consider the ninja log in this dir fresh, from the latest build
	}

	testCases := []struct {
		name        string
		staticSpec  *fintpb.Static
		contextSpec *fintpb.Context
		// We want to skip the ninja no-op check in most tests because it
		// requires complicated mocking, but without setting
		// `SkipNinjaNoopCheck` on every test's context spec. This effectively
		// makes `SkipNinjaNoopCheck` default to true.
		ninjaNoopCheck bool
		// Mock files to populate the build directory with.
		buildDirFiles []string
		modules       fakeBuildModules
		// ninja sub-build directories, relative to the root build dir
		subbuildDirs []subbuildFreshness
		// Callback that is called by the fake runner whenever it starts
		// "running" a command, allowing each test to fake the result and output
		// of any subprocess.
		runnerFunc func(cmd []string, stdout io.Writer, stderr io.Writer) error
		// List of regex strings, where each string corresponds to a subprocess
		// that must have been run by the runner.
		mustRun []string
		// Targets that should be built.
		expectedTargets   []string
		expectedArtifacts *fintpb.BuildArtifacts
		expectErr         bool
		buildAPIClient    buildAPIClient
		// A value that indicates whether a build success stamp file is expected.
		// Default is an empty string, which models that the file should only exist
		// if expectErr is false. Other values are "yes" or "no"
		expectBuildSuccessStamp BuildSuccessStampExpectation
	}{
		{
			name:              "empty spec produces no ninja targets",
			staticSpec:        &fintpb.Static{},
			expectedArtifacts: &fintpb.BuildArtifacts{},
			mustRun:           []string{`ninja -C .*out/default --edge_weights_list=ninja_edge_weights\.csv --error_logging_output=[^ ]+ --chrome_trace ninja_build_trace\.json\.gz --action_metrics_output ninja_action_metrics\.json$`},
		},
		{
			name: "enable_jobserver flag support",
			staticSpec: &fintpb.Static{
				GnArgs: []string{"enable_jobserver = true"},
			},
			expectedArtifacts: &fintpb.BuildArtifacts{},
			mustRun:           []string{`ninja -C .*out/default --edge_weights_list=ninja_edge_weights\.csv --error_logging_output=[^ ]+ --chrome_trace ninja_build_trace\.json\.gz --action_metrics_output ninja_action_metrics\.json --jobserver$`},
		},
		{
			name: "use rs-sub-ninja when build_event_service_ninja is resultstore_infra",
			staticSpec: &fintpb.Static{
				BuildEventServiceNinja: "resultstore_infra",
			},
			modules: fakeBuildModules{
				tools: makeTools(map[string][]string{
					"rs-sub-ninja": {"linux", "mac"},
				}),
			},
			expectedArtifacts: &fintpb.BuildArtifacts{},
			mustRun:           []string{`rs-sub-ninja -C .*out/default --edge_weights_list=ninja_edge_weights\.csv --error_logging_output=[^ ]+ --chrome_trace ninja_build_trace\.json\.gz --action_metrics_output ninja_action_metrics\.json$`},
		},
		{
			name:       "artifact dir set",
			staticSpec: &fintpb.Static{},
			contextSpec: &fintpb.Context{
				ArtifactDir: artifactDir,
			},
			expectedArtifacts: &fintpb.BuildArtifacts{
				BuildstatsJsonFiles: []string{filepath.Join(buildDir, buildstatsJSONName)},
				LogFiles: map[string]string{
					"debug_symbols.json": filepath.Join(artifactDir, "debug_symbols.json"),
				},
				NinjatraceJsonFiles: []string{filepath.Join(buildDir, ninjatraceJSONName)},
			},
			mustRun: []string{`ninja -C .*out/default --edge_weights_list=ninja_edge_weights\.csv --error_logging_output=[^ ]+ --chrome_trace ninja_build_trace\.json\.gz --action_metrics_output ninja_action_metrics\.json$`},
		},
		{
			name:       "affected tests",
			staticSpec: &fintpb.Static{},
			contextSpec: &fintpb.Context{
				ArtifactDir: artifactDir,
				ChangedFiles: []*fintpb.Context_ChangedFile{
					{Path: "src/foo.py"},
				},
			},
			modules: fakeBuildModules{
				testSpecs: []build.TestSpec{
					{Test: build.Test{Name: "foo", Label: "//src:foo"}},
				},
			},
			buildAPIClient: fakeBuildAPIClient{
				affectedTests: []string{"//src:foo,host"},
			},
			expectedArtifacts: &fintpb.BuildArtifacts{
				BuildstatsJsonFiles: []string{filepath.Join(buildDir, buildstatsJSONName)},
				LogFiles: map[string]string{
					"affected_tests_comparison.json": filepath.Join(artifactDir, "affected_tests_comparison.json"),
					"debug_symbols.json":             filepath.Join(artifactDir, "debug_symbols.json"),
					"ninja dry run output":           filepath.Join(artifactDir, "ninja_dry_run_output"),
				},
				NinjatraceJsonFiles: []string{filepath.Join(buildDir, ninjatraceJSONName)},
			},
		},
		{
			name:       "failed build with clang crash reports",
			staticSpec: &fintpb.Static{},
			contextSpec: &fintpb.Context{
				ArtifactDir: artifactDir,
			},
			// We should check for these files when the build fails and
			// reference them in the output artifacts proto.
			buildDirFiles: []string{
				filepath.Join(clangCrashReportsDirName, "foo.sh"),
				filepath.Join(clangCrashReportsDirName, "bar.sh"),
				filepath.Join(clangCrashReportsDirName, "other.file"),
			},
			runnerFunc: failedBuildRunner,
			expectedArtifacts: &fintpb.BuildArtifacts{
				FailureSummary:      "ninja: error: simulated failure\n",
				BuildstatsJsonFiles: []string{filepath.Join(buildDir, buildstatsJSONName)},
				NinjatraceJsonFiles: []string{filepath.Join(buildDir, ninjatraceJSONName)},
				DebugFiles: []*fintpb.DebugFile{
					{
						Path:       filepath.Join(buildDir, "clang-crashreports", "foo.tar"),
						UploadDest: "clang-crashreports/foo.tar",
					},
					{
						Path:       filepath.Join(buildDir, "clang-crashreports", "bar.tar"),
						UploadDest: "clang-crashreports/bar.tar",
					},
					{
						Path:       filepath.Join(buildDir, "clang-crashreports", "other.file"),
						UploadDest: "clang-crashreports/other.file",
					},
				},
			},
			expectErr: true,
		},
		{
			name:       "failed build with determinism/consistency differences",
			staticSpec: &fintpb.Static{},
			contextSpec: &fintpb.Context{
				ArtifactDir: artifactDir,
			},
			buildDirFiles: []string{
				filepath.Join(comparisonDiagnosticsDirName, "dir", "foo.rlib.local"),
				filepath.Join(comparisonDiagnosticsDirName, "dir", "foo.rlib.remote"),
				filepath.Join("not", "in", "expected", "dir", "foo.rlib"),
			},
			runnerFunc: failedBuildRunner,
			expectedArtifacts: &fintpb.BuildArtifacts{
				FailureSummary:      "ninja: error: simulated failure\n",
				BuildstatsJsonFiles: []string{filepath.Join(buildDir, buildstatsJSONName)},
				NinjatraceJsonFiles: []string{filepath.Join(buildDir, ninjatraceJSONName)},
				DebugFiles: []*fintpb.DebugFile{
					{
						Path:       filepath.Join(buildDir, comparisonDiagnosticsDirName, "dir", "foo.rlib.local"),
						UploadDest: filepath.Join(comparisonDiagnosticsDirName, "dir", "foo.rlib.local"),
					},
					{
						Path:       filepath.Join(buildDir, comparisonDiagnosticsDirName, "dir", "foo.rlib.remote"),
						UploadDest: filepath.Join(comparisonDiagnosticsDirName, "dir", "foo.rlib.remote"),
					},
				},
			},
			expectErr: true,
		},
		{
			name:       "failed build with file access traces",
			staticSpec: &fintpb.Static{},
			contextSpec: &fintpb.Context{
				ArtifactDir: artifactDir,
			},
			buildDirFiles: []string{
				filepath.Join(".traces", "accesses_trace.txt"),
				filepath.Join(".traces", "dir", "accesses_trace.txt"),
				filepath.Join(".traces", "dir", "other_accesses_trace.txt"),
				filepath.Join(".traces", "dir", "not_a_trace"),
				filepath.Join(".traces", "nested", "dir", "accesses_trace.txt"),
				filepath.Join("not", "in", "expected", "dir", "accesses_trace.txt"),
			},
			runnerFunc: failedBuildRunner,
			expectedArtifacts: &fintpb.BuildArtifacts{
				FailureSummary:      "ninja: error: simulated failure\n",
				BuildstatsJsonFiles: []string{filepath.Join(buildDir, buildstatsJSONName)},
				NinjatraceJsonFiles: []string{filepath.Join(buildDir, ninjatraceJSONName)},
				DebugFiles: []*fintpb.DebugFile{
					{
						Path:       filepath.Join(buildDir, ".traces", "accesses_trace.txt"),
						UploadDest: ".traces/accesses_trace.txt",
					},
					{
						Path:       filepath.Join(buildDir, ".traces", "dir", "accesses_trace.txt"),
						UploadDest: ".traces/dir/accesses_trace.txt",
					},
					{
						Path:       filepath.Join(buildDir, ".traces", "dir", "other_accesses_trace.txt"),
						UploadDest: ".traces/dir/other_accesses_trace.txt",
					},
					{
						Path:       filepath.Join(buildDir, ".traces", "nested", "dir", "accesses_trace.txt"),
						UploadDest: ".traces/nested/dir/accesses_trace.txt",
					},
				},
			},
			expectErr: true,
		},
		{
			name:       "sub-builds all fresh",
			staticSpec: &fintpb.Static{},
			contextSpec: &fintpb.Context{
				ArtifactDir: artifactDir,
			},
			subbuildDirs: []subbuildFreshness{
				{dir: "sub1", fresh: true},
				{dir: "sub2", fresh: true},
			},
			expectedArtifacts: &fintpb.BuildArtifacts{
				BuildstatsJsonFiles: []string{
					filepath.Join(buildDir, buildstatsJSONName),
					filepath.Join(buildDir, "sub1", buildstatsJSONName),
					filepath.Join(buildDir, "sub2", buildstatsJSONName),
				},
				LogFiles: map[string]string{
					"debug_symbols.json": filepath.Join(artifactDir, "debug_symbols.json"),
				},
				NinjatraceJsonFiles: []string{
					filepath.Join(buildDir, ninjatraceJSONName),
					filepath.Join(buildDir, "sub1", ninjatraceJSONName),
					filepath.Join(buildDir, "sub2", ninjatraceJSONName),
				},
			},
		},
		{
			name:       "sub-builds partially fresh",
			staticSpec: &fintpb.Static{},
			contextSpec: &fintpb.Context{
				ArtifactDir: artifactDir,
			},
			subbuildDirs: []subbuildFreshness{
				{dir: "sub/sub3", fresh: false}, // this contains stale .ninja_log
				{dir: "sub/sub4", fresh: true},
			},
			expectedArtifacts: &fintpb.BuildArtifacts{
				BuildstatsJsonFiles: []string{
					filepath.Join(buildDir, buildstatsJSONName),
					filepath.Join(buildDir, "sub/sub4", buildstatsJSONName),
				},
				LogFiles: map[string]string{
					"debug_symbols.json": filepath.Join(artifactDir, "debug_symbols.json"),
				},
				NinjatraceJsonFiles: []string{
					filepath.Join(buildDir, ninjatraceJSONName),
					filepath.Join(buildDir, "sub/sub4", ninjatraceJSONName),
				},
			},
		},
		{
			name: "incremental build",
			staticSpec: &fintpb.Static{
				Incremental: true,
			},
			contextSpec: &fintpb.Context{
				ArtifactDir: artifactDir,
			},
			expectedArtifacts: &fintpb.BuildArtifacts{
				BuildstatsJsonFiles: []string{filepath.Join(buildDir, buildstatsJSONName)},
				LogFiles: map[string]string{
					"debug_symbols.json": filepath.Join(artifactDir, "debug_symbols.json"),
					"explain_output.txt": filepath.Join(artifactDir, "explain_output.txt"),
				},
				NinjatraceJsonFiles: []string{filepath.Join(buildDir, ninjatraceJSONName)},
			},
		},
		{
			name:           "failed ninja no-op check",
			staticSpec:     &fintpb.Static{},
			ninjaNoopCheck: true,
			// The fake ninja no-op check command succeeds, but does not print any
			// output, so the no-op check will fail because the "no work to do"
			// string will not be present in the output.
			runnerFunc: func(cmd []string, _, _ io.Writer) error {
				return nil
			},
			expectErr:               true,
			expectBuildSuccessStamp: BuildSuccessStampExists,
			expectedArtifacts: &fintpb.BuildArtifacts{
				FailureSummary: ninjaNoopFailureMessage(platform, ""),
				DebugFiles: []*fintpb.DebugFile{
					{
						Path:       filepath.Join(buildDir, ninjaDepsPath),
						UploadDest: ninjaDepsPath,
					},
					{
						Path:       filepath.Join(buildDir, ninjatraceJSONName),
						UploadDest: ninjatraceJSONName,
					},
					{
						Path:       filepath.Join(buildDir, buildstatsJSONName),
						UploadDest: buildstatsJSONName,
					},
				},
			},
		},
		{
			name:           "passed ninja no-op check",
			staticSpec:     &fintpb.Static{},
			ninjaNoopCheck: true,
			runnerFunc: func(cmd []string, stdout, _ io.Writer) error {
				if slices.Contains(cmd, "-n") { // -n indicates ninja dry run.
					stdout.Write([]byte(noWorkString))
				}
				return nil
			},
			expectedArtifacts: &fintpb.BuildArtifacts{},
		},
		{
			name:       "ninjatrace fails",
			staticSpec: &fintpb.Static{},
			contextSpec: &fintpb.Context{
				ArtifactDir: artifactDir,
			},
			runnerFunc: func(cmd []string, stdout, _ io.Writer) error {
				if filepath.Base(cmd[0]) == "ninjatrace_prebuilt" {
					return fmt.Errorf("failed to run command: %s", cmd)
				}
				return nil
			},
			// these errors are now ignored
			expectedArtifacts: &fintpb.BuildArtifacts{
				LogFiles: map[string]string{
					"debug_symbols.json": filepath.Join(artifactDir, "debug_symbols.json"),
				},
			},
		},
		{
			name: "extra ad-hoc ninja targets",
			staticSpec: &fintpb.Static{
				NinjaTargets: []string{"bar", "foo"},
			},
			expectedTargets: []string{"bar", "foo"},
		},
		{
			name: "duplicate targets",
			staticSpec: &fintpb.Static{
				NinjaTargets: []string{"foo", "foo"},
			},
			expectedTargets: []string{"foo"},
		},
		{
			name: "default ninja target included",
			staticSpec: &fintpb.Static{
				IncludeDefaultNinjaTarget: true,
			},
			expectedTargets: []string{":default"},
		},
		{
			name: "host tests included",
			staticSpec: &fintpb.Static{
				IncludeHostTests: true,
			},
			modules: fakeBuildModules{
				testSpecs: []build.TestSpec{
					{Test: build.Test{OS: "fuchsia", Path: "fuchsia_path"}},
					{Test: build.Test{OS: "linux", Path: "linux_path"}},
					{Test: build.Test{OS: "mac", Path: "mac_path"}},
				},
			},
			expectedTargets: []string{"linux_path", "mac_path"},
		},
		{
			name: "generated sources included",
			staticSpec: &fintpb.Static{
				IncludeGeneratedSources: true,
			},
			modules: fakeBuildModules{
				generatedSources: []string{
					"foo.h",
					"bar.cc",
					// Non-C++ files should be ignored, see https://fxbug.dev/42066838.
					"baz.go",
					"quux.rs",
				},
			},
			expectedTargets: []string{"foo.h", "bar.cc"},
		},
		{
			name: "prebuilt binary manifests included",
			staticSpec: &fintpb.Static{
				IncludePrebuiltBinaryManifests: true,
			},
			modules: fakeBuildModules{
				pbinSets: []build.PrebuiltBinarySet{
					{Manifest: "manifest1.json"},
					{Manifest: "manifest2.json"},
				},
			},
			expectedTargets: []string{"manifest1.json", "manifest2.json"},
		},
		{
			name: "tools included",
			staticSpec: &fintpb.Static{
				Tools: []string{"tool1", "tool2"},
			},
			modules: fakeBuildModules{
				tools: makeTools(map[string][]string{
					"tool1": {"linux", "mac"},
					"tool2": {"linux"},
					"tool3": {"linux", "mac"},
				}),
			},
			expectedTargets: []string{"linux_x64/tool1", "linux_x64/tool2"},
		},
		{
			name: "all lint targets",
			staticSpec: &fintpb.Static{
				IncludeLintTargets: fintpb.Static_ALL_LINT_TARGETS,
			},
			modules: fakeBuildModules{
				clippyTargets: []build.ClippyTarget{
					{
						Output: "gen/src/foo.clippy",
						Sources: []string{
							"../../src/foo/x.rs",
							"../../src/foo/y.rs",
						},
						DisableClippy: false,
					},
					{
						Output: "gen/src/bar.clippy",
						Sources: []string{
							"../../src/bar/x.rs",
							"../../src/bar/y.rs",
						},
						DisableClippy: false,
					},
				},
			},
			expectedTargets: []string{"gen/src/foo.clippy", "gen/src/bar.clippy"},
		},
		{
			name: "affected lint targets",
			staticSpec: &fintpb.Static{
				IncludeLintTargets: fintpb.Static_AFFECTED_LINT_TARGETS,
			},
			contextSpec: &fintpb.Context{
				ChangedFiles: []*fintpb.Context_ChangedFile{
					{Path: "src/foo/x.rs"},
					{Path: "src/foo/y.rs"},
				},
			},
			modules: fakeBuildModules{
				clippyTargets: []build.ClippyTarget{
					{
						Output: "gen/src/foo.clippy",
						Sources: []string{
							"../../src/foo/x.rs",
							"../../src/foo/y.rs",
						},
						DisableClippy: false,
					},
					{
						Output: "gen/src/bar.clippy",
						Sources: []string{
							"../../src/bar/x.rs",
							"../../src/bar/y.rs",
						},
						DisableClippy: false,
					},
				},
			},
			expectedTargets: []string{"gen/src/foo.clippy"},
		},
		{
			name: "nonexistent tool",
			staticSpec: &fintpb.Static{
				Tools: []string{"tool1"},
			},
			expectErr: true,
		},
		{
			name: "tool not supported on current platform",
			staticSpec: &fintpb.Static{
				Tools: []string{"tool1"},
			},
			modules: fakeBuildModules{
				tools: makeTools(map[string][]string{
					"tool1": {"mac"},
				}),
			},
			expectErr: true,
		},
		{
			name: "include_archives not supported",
			staticSpec: &fintpb.Static{
				IncludeArchives: true,
			},
			expectErr: true,
		},
		{
			name:       "export debug symbols",
			staticSpec: &fintpb.Static{},
			modules: fakeBuildModules{
				args: build.Args{
					"output_breakpad_syms": jsonRawMessage(t, "true"),
				},
			},
			buildDirFiles: []string{"debug_symbols.json"},
		},
	}
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			resetDirs(t)

			for _, relpath := range tc.buildDirFiles {
				createEmptyFile(t, filepath.Join(buildDir, relpath))
			}

			defaultContextSpec := &fintpb.Context{
				SkipNinjaNoopCheck: !tc.ninjaNoopCheck,
				CheckoutDir:        checkoutDir,
				BuildDir:           buildDir,
			}
			proto.Merge(defaultContextSpec, tc.contextSpec)
			tc.contextSpec = defaultContextSpec
			runner := &fakeSubprocessRunner{run: tc.runnerFunc}

			// Write out a ninja_subbuilds.json file
			// and construct a map of each subbuild's freshness.
			subbuildFreshnessMap := make(map[string]bool)
			if len(tc.subbuildDirs) > 0 {
				subbuildJSONFile := filepath.Join(buildDir, subbuildsJSONName)
				var subbuildEntries []subbuildEntry
				for _, dir := range tc.subbuildDirs {
					subbuildEntries = append(subbuildEntries, subbuildEntry{BuildDir: dir.dir})
					subbuildFreshnessMap[dir.dir] = dir.fresh
				}
				contents, err := json.Marshal(subbuildEntries)
				if err != nil {
					t.Fatalf("Failed to marshal to JSON: %v", subbuildEntries)
				}
				if err := os.WriteFile(subbuildJSONFile, contents, 0o600); err != nil {
					t.Fatalf("Got unexpected error writing JSON file: %s", err)
				}
			}

			// Fake function for determining which subbuild dirs have a fresh ninja log.
			subninjaLogIsRecent := func(log string, ninjaStart time.Time) (bool, error) {
				baseDir := filepath.Dir(log)
				subdir := strings.TrimPrefix(baseDir, buildDir+"/")
				return subbuildFreshnessMap[subdir], nil
			}

			fileExists := func(_ string) bool { return true }
			tc.modules.buildDir = tc.contextSpec.BuildDir
			tc.modules.tools = append(tc.modules.tools, makeTools(
				map[string][]string{
					"gn":                  {"linux", "mac"},
					"ninja":               {"linux", "mac"},
					"buildstats_prebuilt": {"linux", "mac"},
					"ninjatrace_prebuilt": {"linux", "mac"},
				},
			)...)
			ctx := context.Background()
			client := tc.buildAPIClient
			if client == nil {
				client = fakeBuildAPIClient{}
			}
			artifacts, err := buildImpl(
				ctx, runner, subninjaLogIsRecent, fileExists, tc.staticSpec, tc.contextSpec, tc.modules, client, platform)
			if err != nil {
				if !tc.expectErr {
					t.Fatalf("Got unexpected error: %s", err)
				}
			} else if tc.expectErr {
				t.Fatal("Expected an error but got nil")
			}

			// Check the status of the build success stamp file.
			expectedBuildStampStatus := tc.expectBuildSuccessStamp
			if expectedBuildStampStatus == BuildSuccessStampAuto {
				if tc.expectErr {
					expectedBuildStampStatus = BuildSuccessStampMissing
				} else {
					expectedBuildStampStatus = BuildSuccessStampExists
				}
			}
			if checkFileExists(filepath.Join(buildDir, buildSuccessStampName)) {
				if expectedBuildStampStatus == BuildSuccessStampMissing {
					t.Errorf("Build success stamp file present in failed build!")
				} else {
					// Check the presence of the last ninja targets list file.
					if !checkFileExists(filepath.Join(buildDir, lastNinjaBuildTargetsName)) {
						t.Errorf("Last ninja build targets list file missing!")
					}
				}
			} else {
				if expectedBuildStampStatus == BuildSuccessStampExists {
					t.Errorf("Build success stamp file missing after successful build!")
				}
			}

			ninjaTargets := findNinjaTargets(runner.commandsRun)

			sortSlicesOpt := cmpopts.SortSlices(func(a, b string) bool { return a < b })

			if diff := cmp.Diff(tc.expectedTargets, ninjaTargets, sortSlicesOpt, cmpopts.EquateEmpty()); diff != "" {
				t.Errorf("Wrong ninja targets build (-want +got):\n%s", diff)
			}

			if tc.expectedArtifacts == nil {
				tc.expectedArtifacts = &fintpb.BuildArtifacts{}
			}
			opts := cmp.Options{
				protocmp.Transform(),
				// Ordering of the repeated artifact fields doesn't matter.
				sortSlicesOpt,
				protocmp.SortRepeated(func(a, b *fintpb.DebugFile) bool { return a.Path < b.Path }),
			}
			if diff := cmp.Diff(tc.expectedArtifacts, artifacts, opts...); diff != "" {
				t.Errorf("Got wrong artifacts (-want +got):\n%s", diff)
			}

			for _, s := range tc.mustRun {
				re, err := regexp.Compile(s)
				if err != nil {
					t.Fatal(err)
				}
				found := false
				for _, cmd := range runner.commandsRun {
					if re.MatchString(strings.Join(cmd, " ")) {
						found = true
						break
					}
				}
				if !found {
					t.Errorf("No command was run matching %q. Commands run: %s", s, pretty.Sprint(runner.commandsRun))
				}
			}
		})
	}
}

// findNinjaTargets is a hacky utility to return a list of built ninja targets,
// given a list of command line invocations that is assumed to contain a ninja
// invocation.
//
// It returns an empty slice if it fails to find a ninja invocation in the list
// of commands.
func findNinjaTargets(cmds [][]string) []string {
	for _, cmd := range cmds {
		// Ignore non-ninja commands and ninja tool invocations.
		prog := filepath.Base(cmd[0])
		if (prog != "ninja" && prog != "rs-sub-ninja") || slices.Contains(cmd, "-t") {
			continue
		}
		// Skip over each `-flag value` pair until we reach the list of targets.
		for i := 1; i < len(cmd); i++ {
			if strings.HasPrefix(cmd[i], "-") {
				// The current token appears to be an option.
				if !strings.Contains(cmd[i], "=") {
					// It doesn't contain an '=' to separate the value from the key,
					// so assume the following token is the value for this argument
					// and skip it.
					i++
				}
			} else {
				// We've reached the start of the list of targets, assumed to
				// come after all flags.
				return cmd[i:]
			}
		}
		// Found a ninja command that specified zero targets, implying the
		// default target.
		return nil
	}
	// Failed to find a full-build ninja command (likely because something
	// failed before ninja was run). We don't bother to distinguish between this
	// case and the zero-targets case because tests separately check if a
	// failure occurred.
	return nil
}

func makeTools(supportedOSes map[string][]string) build.Tools {
	var res build.Tools
	for toolName, systems := range supportedOSes {
		for _, os := range systems {
			res = append(res, build.Tool{
				Name: toolName,
				OS:   os,
				CPU:  "x64",
				Path: fmt.Sprintf("%s_x64/%s", os, toolName),
			})
		}
	}
	return res
}

func createEmptyFile(t *testing.T, path string) {
	t.Helper()

	f, err := osmisc.CreateFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
}

// mustStructPB converts a Go struct to a protobuf Struct, failing the test in
// case of failure.
func mustStructPB(t *testing.T, s any) *structpb.Struct {
	t.Helper()

	ret, err := toStructPB(s)
	if err != nil {
		t.Fatal(err)
	}
	return ret
}

func jsonRawMessage(t *testing.T, data string) json.RawMessage {
	t.Helper()
	var m json.RawMessage
	if err := m.UnmarshalJSON([]byte(data)); err != nil {
		t.Fatal(err)
	}
	return m
}

func Test_gnCheckGenerated(t *testing.T) {
	ctx := context.Background()
	runner := fakeSubprocessRunner{
		mockStdout: []byte("check error\n"),
		fail:       true,
	}
	output, err := gnCheckGenerated(ctx, &runner, "gn", t.TempDir(), t.TempDir())
	if err == nil {
		t.Fatalf("Expected gn check to fail")
	}
	if diff := cmp.Diff(string(runner.mockStdout), output); diff != "" {
		t.Errorf("Got wrong gn check output (-want +got):\n%s", diff)
	}
}

func Test_writeBuildDirManifest(t *testing.T) {
	buildDir := t.TempDir()

	regularFile := filepath.Join(buildDir, "dir", "foo")
	createEmptyFile(t, regularFile)

	symlink := filepath.Join(buildDir, "bar-link")
	// The symlink target doesn't actually exist, which is fine - we should be
	// able to construct the manifest anyway, including the invalid symlink.
	if err := os.Symlink(filepath.Join("..", "..", "bar"), symlink); err != nil {
		t.Fatal(err)
	}

	for _, path := range []string{regularFile, symlink} {
		tv := unix.Timeval{Sec: 123, Usec: 456789}
		// Use unix-specific Lutimes instead of os.Chtimes to ensure we update
		// the mtime of the symlink itself rather than its target.
		if err := unix.Lutimes(path, []unix.Timeval{tv, tv}); err != nil {
			t.Fatal(err)
		}
	}

	want := []buildDirManifestElement{
		{
			Path:                "dir/foo",
			ModTimeMilliseconds: 123456.789,
		},
		{
			Path:                "bar-link",
			SymlinkTarget:       "../../bar",
			ModTimeMilliseconds: 123456.789,
		},
	}

	outputPath := filepath.Join(t.TempDir(), "output.json")
	if err := writeBuildDirManifest(buildDir, outputPath); err != nil {
		t.Fatal(err)
	}

	var got []buildDirManifestElement
	if err := jsonutil.ReadFromFile(outputPath, &got); err != nil {
		t.Fatalf("Failed to read output file: %s", err)
	}

	opts := []cmp.Option{
		cmpopts.SortSlices(func(x, y buildDirManifestElement) bool {
			return x.Path < y.Path
		}),
		cmpopts.EquateEmpty(),
	}

	if diff := cmp.Diff(want, got, opts...); diff != "" {
		t.Errorf("Build dir manifest is wrong (-want +got):\n%s", diff)
	}
}

type mockSubprocessRunner struct {
	commandsRun [][]string
}

func (r *mockSubprocessRunner) Run(ctx context.Context, cmd []string, options subprocess.RunOptions) error {
	r.commandsRun = append(r.commandsRun, cmd)
	return nil
}

func TestBuildBazelTests(t *testing.T) {
	checkoutDir := t.TempDir()
	buildDir := t.TempDir()

	testSpecs := []build.TestSpec{
		{
			Test: build.Test{
				Label: "@//src/foo:foo_test",
				Path:  "bazel-out/foo",
			},
		},
		{
			Test: build.Test{
				Label: "@//src/bar:bar_test",
				Path:  "bazel-out/bar",
			},
		},
		{
			Test: build.Test{
				Label: "//src/gn:gn_test",
				Path:  "host_x64/gn_test",
			},
		},
	}

	// Create bazel_top_dir config in checkoutDir
	topDirConfigDir := filepath.Join(checkoutDir, "build", "bazel", "config")
	if err := os.MkdirAll(topDirConfigDir, 0o700); err != nil {
		t.Fatal(err)
	}
	topDirConfigPath := filepath.Join(topDirConfigDir, "bazel_top_dir")
	if err := os.WriteFile(topDirConfigPath, []byte("custom/bazel/dir\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	// Create a mock bazel launcher binary
	bazelLauncherDir := filepath.Join(buildDir, "custom", "bazel", "dir")
	if err := os.MkdirAll(bazelLauncherDir, 0o700); err != nil {
		t.Fatal(err)
	}
	bazelLauncherPath := filepath.Join(bazelLauncherDir, "bazel")
	if err := os.WriteFile(bazelLauncherPath, []byte(""), 0o700); err != nil {
		t.Fatal(err)
	}

	ctx := context.Background()
	runner := &mockSubprocessRunner{}
	if _, err := buildBazelTests(ctx, runner, checkoutDir, buildDir, testSpecs); err != nil {
		t.Fatal(err)
	}

	if len(runner.commandsRun) != 1 {
		t.Fatalf("expected 1 command run, got %d", len(runner.commandsRun))
	}

	gotCmd := runner.commandsRun[0]
	wantCmd := []string{
		bazelLauncherPath,
		"build",
		"--config=host",
		"--build_runfile_links=true",
		"--enable_runfiles=true",
		"@//src/bar:bar_test",
		"@//src/foo:foo_test",
	}
	// Sort the labels at the end of wantCmd and gotCmd to ensure deterministic comparison
	labelsStartIdx := 5
	if len(gotCmd) > labelsStartIdx {
		slices.Sort(gotCmd[labelsStartIdx:])
	}
	if diff := cmp.Diff(wantCmd, gotCmd); diff != "" {
		t.Errorf("command differs (-want +got):\n%s", diff)
	}
}

func TestParseBazelError(t *testing.T) {
	stderr := `Computing main repo mapping:
Loading:
Loading: 6 packages loaded
WARNING: Build options --define, --enable_runfiles, --features, and 1 more have changed, discarding analysis cache (this can be expensive, see https://bazel.build/advanced/performance/iteration-speed).
Analyzing: 36 targets (19 packages loaded, 9 targets configured)
INFO: Analyzed 36 targets (302 packages loaded, 18934 targets configured).
[509 / 812] Action build/bazel/rules/host_tests/tests/py/test_with_unittests; 0s local ... (24 actions, 22 running)
ERROR: /foo/bar.bazel:55:18: Linking failed
FAILED
[542 / 812] GoToolchainBinaryBuild ...
ERROR: Build did NOT complete successfully
Use --verbose_failures to see the command lines of failed build steps.`

	want := `ERROR: /foo/bar.bazel:55:18: Linking failed
FAILED`

	if got := parseBazelError(stderr); got != want {
		t.Errorf("parseBazelError() = %q, want %q", got, want)
	}
}
