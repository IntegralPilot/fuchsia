// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"

	"go.fuchsia.dev/fuchsia/tools/build"
	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder"
	"go.fuchsia.dev/fuchsia/tools/integration/testsharder/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"

	"google.golang.org/protobuf/types/known/durationpb"
)

var (
	updateGoldens = flag.Bool("update-goldens", false, "Whether to update goldens")
	goldensDir    = flag.String("goldens-dir", "testdata", "Directory containing goldens")
)

const testListPath = "fake-test-list.json"

type mockFFX struct {
	ffxutil.MockFFXInstance
}

func (m *mockFFX) GetPBArtifacts(ctx context.Context, pbPath, group string) ([]string, error) {
	return []string{"zbi"}, nil
}

// TestExecute runs golden tests for the execute() function.
//
// To add a new test case:
//  1. Add an entry to the `testCases` slice here.
//  2. Run `tools/integration/testsharder/update_goldens.sh` to generate the new
//     golden file.
//  3. Add the new golden file as a dependency of the test executable in
//     testsharder's BUILD.gn file.
func TestExecute(t *testing.T) {
	ctx := context.Background()

	// Clear pre-existing golden files to avoid leaving stale ones around.
	if *updateGoldens {
		files, err := filepath.Glob(filepath.Join(*goldensDir, "*.golden.json"))
		if err != nil {
			t.Fatal(err)
		}
		for _, f := range files {
			if err := os.Remove(f); err != nil {
				t.Fatal(err)
			}
		}
	}

	testCases := []struct {
		name          string
		flags         testsharderFlags
		params        *proto.Params
		testSpecs     []build.TestSpec
		testDurations []build.TestDuration
		testList      []build.TestListEntry
		modifiers     []testsharder.TestModifier
		packageRepos  []build.PackageRepo
		affectedTests []string
	}{
		{
			name: "no tests",
		},
		{
			name: "mixed device types",
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("foo"),
				hostTestSpec("bar"),
			},
		},
		{
			name: "disabled device types",
			params: &proto.Params{
				DisabledDeviceTypes: []string{"QEMU"},
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("foo", "AEMU", "QEMU"),
				hostTestSpec("bar"),
			},
		},
		{
			name: "allowed device types",
			flags: testsharderFlags{
				allowedDeviceTypes: []string{"QEMU", "NUC"},
			},
			params: &proto.Params{
				DisabledDeviceTypes: []string{"NUC"},
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("foo", "AEMU", "QEMU", "NUC"),
				hostTestSpec("bar"),
			},
		},
		{
			// Two tests whose dimensions differ only by some random dimension
			// ("other_dimension") should still be sharded separately.
			name: "arbitrary dimensions",
			params: &proto.Params{
				Pool: "other.pool",
			},
			testSpecs: []build.TestSpec{
				{
					Test: build.Test{
						Name:        "host_x64/foo.sh",
						Path:        "host_x64/foo.sh",
						OS:          "linux",
						CPU:         "x64",
						Label:       "//tools/other:foo(//build/toolchain/host_x64)",
						SourceLabel: "//tools/other:foo",
					},
					Envs: []build.Environment{
						{
							Dimensions: build.DimensionSet{
								"cpu":             "x64",
								"os":              "Linux",
								"other_dimension": "foo",
							},
						},
						{
							Dimensions: build.DimensionSet{
								"device_type": "AEMU",
							},
							Emulator: build.EmulatorInfo{
								Name:   "emu-min",
								Device: "min",
							},
						},
					},
				},
				{
					Test: build.Test{
						Name:        "host_x64/bar.sh",
						Path:        "host_x64/bar.sh",
						OS:          "linux",
						CPU:         "x64",
						Label:       "//tools/other:bar(//build/toolchain/host_x64)",
						SourceLabel: "//tools/other:bar",
					},
					Envs: []build.Environment{
						{
							Dimensions: build.DimensionSet{
								"cpu":             "x64",
								"os":              "Linux",
								"other_dimension": "bar",
							},
						},
						{
							Dimensions: build.DimensionSet{
								"device_type": "AEMU",
							},
							Emulator: build.EmulatorInfo{
								Name:   "emu-min-tcg",
								Device: "min",
								Accel:  "none",
							},
						},
					},
				},
			},
		},
		{
			name: "multiply",
			params: &proto.Params{
				TargetDuration: durationpb.New(5 * time.Second),
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("foo"),
				fuchsiaTestSpec("bar"),
			},
			modifiers: []testsharder.TestModifier{
				{
					Name:      "foo",
					TotalRuns: 50,
				},
				{
					Name: "bar",
				},
			},
			testDurations: []build.TestDuration{
				{
					Name:           "*",
					MedianDuration: time.Millisecond,
				},
			},
		},
		{
			name: "affected tests",
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("affected-hermetic"),
				fuchsiaTestSpec("not-affected"),
			},
			testList: []build.TestListEntry{
				testListEntry("affected-hermetic", true),
				testListEntry("not-affected", false),
			},
			affectedTests: []string{
				packageURL("affected-hermetic"),
			},
		},
		{
			name: "affected nonhermetic tests",
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("affected-nonhermetic"),
				fuchsiaTestSpec("not-affected"),
			},
			affectedTests: []string{
				packageURL("affected-nonhermetic"),
			},
		},
		{
			name: "target test count",
			params: &proto.Params{
				TargetTestCount: 2,
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("foo1"),
				fuchsiaTestSpec("foo2"),
				fuchsiaTestSpec("foo3"),
				fuchsiaTestSpec("foo4"),
			},
		},
		{
			name: "sharding by time",
			params: &proto.Params{
				TargetDuration: durationpb.New(4 * time.Minute),
				PerTestTimeout: durationpb.New(10 * time.Minute),
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("slow"),
				fuchsiaTestSpec("fast1"),
				fuchsiaTestSpec("fast2"),
				fuchsiaTestSpec("fast3"),
			},
			testDurations: []build.TestDuration{
				{
					Name:           "*",
					MedianDuration: 2 * time.Second,
				},
				{
					Name:           packageURL("slow"),
					MedianDuration: 5 * time.Minute,
				},
			},
		},
		{
			name: "max shards per env",
			flags: testsharderFlags{
				skipUnaffected: true,
			},
			params: &proto.Params{
				// Given expected test durations of 4 minutes for each test it's
				// impossible to satisfy both the target shard duration and the
				// max shards per environment, so the target shard duration
				// should effectively be ignored.
				TargetDuration:  durationpb.New(5 * time.Minute),
				MaxShardsPerEnv: 2,
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("affected1"),
				fuchsiaTestSpec("affected2"),
				fuchsiaTestSpec("affected3"),
				fuchsiaTestSpec("affected4"),
				fuchsiaTestSpec("unaffected1"),
				fuchsiaTestSpec("unaffected2"),
				fuchsiaTestSpec("nonhermetic1"),
				fuchsiaTestSpec("nonhermetic2"),
			},
			testDurations: []build.TestDuration{
				{
					Name:           "*",
					MedianDuration: 4 * time.Minute,
				},
			},
			affectedTests: []string{
				packageURL("affected1"),
				packageURL("affected2"),
				packageURL("affected3"),
				packageURL("affected4"),
			},
			testList: []build.TestListEntry{
				testListEntry("affected1", true),
				testListEntry("affected2", true),
				testListEntry("affected3", true),
				testListEntry("affected4", true),
				testListEntry("unaffected1", true),
				testListEntry("unaffected2", true),
				testListEntry("nonhermetic1", false),
				testListEntry("nonhermetic2", false),
			},
		},
		{
			name: "hermetic deps",
			params: &proto.Params{
				HermeticDeps: true,
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("foo"),
				fuchsiaTestSpec("bar"),
				fuchsiaTestSpec("baz"),
			},
			packageRepos: []build.PackageRepo{
				{
					Path:    "pkg_repo1",
					Blobs:   filepath.Join("pkg_repo1", "blobs"),
					Targets: filepath.Join("pkg_repo1", "targets.json"),
				},
			},
		},
		{
			name: "multiply affected test",
			params: &proto.Params{
				AffectedTestsMultiplyThreshold: 3,
				TargetDuration:                 durationpb.New(2 * time.Minute),
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("multiplied-affected-test"),
				fuchsiaTestSpec("affected-test"),
				fuchsiaTestSpec("unaffected-test"),
			},
			testDurations: []build.TestDuration{
				{
					Name:           "*",
					MedianDuration: time.Second,
				},
			},
			affectedTests: []string{
				packageURL("multiplied-affected-test"),
				packageURL("affected-test"),
			},
			modifiers: []testsharder.TestModifier{
				{
					Name:      "multiplied-affected-test",
					TotalRuns: 100,
				},
			},
		},
		{
			name: "multiply affected tests with large number of runs",
			params: &proto.Params{
				AffectedTestsMultiplyThreshold: 3,
				TargetDuration:                 durationpb.New(5 * time.Minute),
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("affected-test1"),
				fuchsiaTestSpec("affected-test2"),
				fuchsiaTestSpec("affected-test3"),
			},
			testDurations: []build.TestDuration{
				{
					Name: "*",
					// Test duration is very short relative to the target shard
					// duration, so the tests should get multiplied many times.
					MedianDuration: time.Millisecond,
				},
			},
			affectedTests: []string{
				packageURL("affected-test1"),
				packageURL("affected-test2"),
				packageURL("affected-test3"),
			},
		},
		{
			name: "test list with tags",
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("hermetic-test"),
				fuchsiaTestSpec("nonhermetic-test"),
			},
			testList: []build.TestListEntry{
				testListEntry("hermetic-test", true),
				testListEntry("nonhermetic-test", false),
			},
		},
		{
			name: "skip unaffected tests",
			flags: testsharderFlags{
				skipUnaffected: true,
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("affected-hermetic-test"),
				fuchsiaTestSpec("unaffected-hermetic-test"),
				fuchsiaTestSpec("affected-nonhermetic-test"),
				fuchsiaTestSpec("unaffected-nonhermetic-test"),
			},
			testList: []build.TestListEntry{
				testListEntry("affected-hermetic-test", true),
				testListEntry("unaffected-hermetic-test", true),
				testListEntry("affected-nonhermetic-test", false),
				testListEntry("unaffected-nonhermetic-test", false),
			},
			affectedTests: []string{
				fuchsiaTestSpec("affected-hermetic-test").Name,
				fuchsiaTestSpec("affected-nonhermetic-test").Name,
			},
		},
		{
			name: "run all tests if no affected tests",
			flags: testsharderFlags{
				skipUnaffected: true,
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("affected-hermetic-test"),
				fuchsiaTestSpec("unaffected-hermetic-test"),
				fuchsiaTestSpec("affected-nonhermetic-test"),
				fuchsiaTestSpec("unaffected-nonhermetic-test"),
			},
			testList: []build.TestListEntry{
				testListEntry("affected-hermetic-test", true),
				testListEntry("unaffected-hermetic-test", true),
				testListEntry("affected-nonhermetic-test", false),
				testListEntry("unaffected-nonhermetic-test", false),
			},
		},
		{
			name: "run all tests if empty affected tests",
			flags: testsharderFlags{
				skipUnaffected: true,
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("affected-hermetic-test"),
				fuchsiaTestSpec("unaffected-hermetic-test"),
				fuchsiaTestSpec("affected-nonhermetic-test"),
				fuchsiaTestSpec("unaffected-nonhermetic-test"),
			},
			testList: []build.TestListEntry{
				testListEntry("affected-hermetic-test", true),
				testListEntry("unaffected-hermetic-test", true),
				testListEntry("affected-nonhermetic-test", false),
				testListEntry("unaffected-nonhermetic-test", false),
			},
			affectedTests: []string{""},
		},
		{
			name: "run all tests if no affected and affected only",
			flags: testsharderFlags{
				affectedOnly:   true,
				skipUnaffected: true,
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("affected-hermetic-test"),
				fuchsiaTestSpec("unaffected-hermetic-test"),
				fuchsiaTestSpec("affected-nonhermetic-test"),
				fuchsiaTestSpec("unaffected-nonhermetic-test"),
			},
			testList: []build.TestListEntry{
				testListEntry("affected-hermetic-test", true),
				testListEntry("unaffected-hermetic-test", true),
				testListEntry("affected-nonhermetic-test", false),
				testListEntry("unaffected-nonhermetic-test", false),
			},
		},
		{
			name: "multiply unaffected hermetic tests",
			flags: testsharderFlags{
				skipUnaffected: true,
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("unaffected-hermetic-test"),
				fuchsiaTestSpec("affected-nonhermetic-test"),
				fuchsiaTestSpec("unaffected-hermetic-multiplied-test"),
			},
			testList: []build.TestListEntry{
				testListEntry("unaffected-hermetic-test", true),
				testListEntry("affected-nonhermetic-test", false),
				testListEntry("unaffected-hermetic-multiplied-test", true),
			},
			affectedTests: []string{
				fuchsiaTestSpec("affected-nonhermetic-test").Name,
			},
			modifiers: []testsharder.TestModifier{
				{
					Name:      "unaffected-hermetic-multiplied-test",
					TotalRuns: 100,
				},
			},
		},
		{
			name: "boot test with modifiers",
			params: &proto.Params{
				TargetDuration: durationpb.New(5 * time.Second),
			},
			testSpecs: []build.TestSpec{
				bootTestSpec("boot-test"),
				bootTestSpec("another-boot-test"),
			},
			modifiers: []testsharder.TestModifier{
				{
					Name:        "*",
					TotalRuns:   -1,
					MaxAttempts: 1,
				},
			},
			testList: []build.TestListEntry{
				testListEntry("boot-test", true),
				testListEntry("another-boot-test", true),
			},
		},
		{
			name: "various modifiers",
			params: &proto.Params{
				TargetDuration: durationpb.New(5 * time.Second),
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("foo"),
				fuchsiaTestSpec("bar"),
				fuchsiaTestSpec("baz"),
			},
			modifiers: []testsharder.TestModifier{
				// default modifier
				{
					Name:        "*",
					TotalRuns:   -1,
					MaxAttempts: 2,
				},
				// multiplier
				{
					Name:        "foo",
					MaxAttempts: 1,
				},
				// change maxAttempts (but multiplier takes precedence)
				{
					Name:        "foo",
					TotalRuns:   -1,
					MaxAttempts: 1,
				},
				// change maxAttempts, set affected
				{
					Name:        "bar",
					Affected:    true,
					TotalRuns:   -1,
					MaxAttempts: 1,
				},
			},
			testList: []build.TestListEntry{
				testListEntry("foo", false),
				testListEntry("bar", true),
				testListEntry("baz", false),
			},
			testDurations: []build.TestDuration{
				{
					Name:           "*",
					MedianDuration: time.Millisecond,
				},
			},
		},
		{
			name: "deps file",
			flags: testsharderFlags{
				depsFile: "deps-file.json",
			},
			testSpecs: []build.TestSpec{
				fuchsiaTestSpec("foo"),
				fuchsiaTestSpec("bar"),
				fuchsiaTestSpec("baz"),
			},
		},
		{
			// Tests marked as `isolated` should be given their own shards even
			// if it ends up exceeding max_shards_per_env, since `isolated`
			// indicates that the test may be incorrect (or cause incorrectness
			// in later tests) if it shares a shard.
			name: "isolated tests",
			testSpecs: []build.TestSpec{
				isolated(fuchsiaTestSpec("foo")),
				isolated(fuchsiaTestSpec("bar")),
				isolated(fuchsiaTestSpec("baz")),
			},
			params: &proto.Params{
				MaxShardsPerEnv: 1,
			},
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			goldenBasename := strings.ReplaceAll(tc.name, " ", "_") + ".golden.json"
			goldenFile := filepath.Join(*goldensDir, goldenBasename)

			if tc.params == nil {
				tc.params = &proto.Params{}
			}

			// testsharder assumes the build dir is two subdirectories down from the checkout root.
			tc.flags.checkoutDir = t.TempDir()
			tc.flags.buildDir = filepath.Join(tc.flags.checkoutDir, "out", "temp")
			if tc.flags.depsFile != "" {
				tc.flags.depsFile = filepath.Join(t.TempDir(), tc.flags.depsFile)
				// When using depsFile, the outputFile must be within the checkoutDir.
				tc.flags.outputFile = filepath.Join(tc.flags.checkoutDir, goldenBasename)
			} else {
				tc.flags.outputFile = filepath.Join(t.TempDir(), goldenBasename)
			}

			if *updateGoldens {
				defer func() {
					if err := osmisc.CopyFile(tc.flags.outputFile, goldenFile); err != nil {
						t.Fatal(err)
					}
				}()
			}

			tc.params.ProductBundleName = "core.x64"
			tc.params.Pave = true
			if len(tc.modifiers) > 0 {
				tc.flags.modifiersPath = writeTempJSONFile(t, tc.modifiers)
			}
			if len(tc.affectedTests) > 0 {
				// Add a newline to the end of the file to test that it still calculates the
				// correct number of affected tests even with extra whitespace.
				tc.flags.affectedTestsPath = writeTempFile(t, strings.Join(tc.affectedTests, "\n")+"\n")
			}
			if err := os.MkdirAll(tc.flags.buildDir, os.ModePerm); err != nil {
				t.Fatal(err)
			}
			// Write test-list.json.
			if err := jsonutil.WriteToFile(
				filepath.Join(tc.flags.buildDir, testListPath),
				build.TestList{Data: tc.testList, SchemaID: "experimental"},
			); err != nil {
				t.Fatal(err)
			}
			origGetHostPlatform := getHostPlatform
			origGetFFX := testsharder.GetFFX
			defer func() {
				getHostPlatform = origGetHostPlatform
				testsharder.GetFFX = origGetFFX
			}()
			getHostPlatform = func() (string, error) {
				return "linux-x64", nil
			}
			testsharder.GetFFX = func(ctx context.Context, ffxPath, outputsDir string) (testsharder.FFXInterface, error) {
				return &mockFFX{}, nil
			}
			writeDeps(t, tc.flags.buildDir, tc.testSpecs)
			for _, repo := range tc.packageRepos {
				if err := os.MkdirAll(filepath.Join(tc.flags.buildDir, repo.Path), 0o700); err != nil {
					t.Fatal(err)
				}
			}

			m := &fakeModules{
				testSpecs:           tc.testSpecs,
				testDurations:       tc.testDurations,
				packageRepositories: tc.packageRepos,
				productBundles: []build.ProductBundle{
					{Name: "core.x64", Path: "product_bundle"},
					{Name: "boot-test_product_bundle", Path: "boot-test_product_bundle"},
				},
			}
			if err := execute(ctx, tc.flags, tc.params, m); err != nil {
				t.Fatal(err)
			}

			if !*updateGoldens {
				want := readShards(t, goldenFile)
				got := readShards(t, tc.flags.outputFile)
				if diff := cmp.Diff(want, got, cmp.FilterValues(func(s1, s2 fintpb.SetArtifacts_Metadata) bool {
					return true
				}, cmp.Ignore())); diff != "" {
					t.Errorf(strings.Join([]string{
						"Golden file mismatch!",
						"To fix, run `tools/integration/testsharder/update_goldens.sh",
						diff,
					}, "\n"))
				}
			}
		})
	}
}

type fakeModules struct {
	testSpecs           []build.TestSpec
	testList            string
	testDurations       []build.TestDuration
	packageRepositories []build.PackageRepo
	productBundles      []build.ProductBundle
}

func (m *fakeModules) Platforms() []build.DimensionSet {
	return []build.DimensionSet{
		{
			"device_type": "AEMU",
		},
		{
			"device_type": "QEMU",
		},
		{
			"device_type": "NUC",
		},
		{
			"cpu": "x64",
			"os":  "Linux",
		},
		{
			"cpu":             "x64",
			"os":              "Linux",
			"other_dimension": "foo",
		},
		{
			"cpu":             "x64",
			"os":              "Linux",
			"other_dimension": "bar",
		},
	}
}

func (m *fakeModules) Args() build.Args {
	return build.Args{
		"build_info_product": json.RawMessage(`"core"`),
		"build_info_board":   json.RawMessage(`"x64"`),
		"target_cpu":         json.RawMessage(`"x64"`),
		"compilation_mode":   json.RawMessage(`"debug"`),
		"select_variant":     json.RawMessage(`["coverage"]`),
	}
}

func (m *fakeModules) ModulePaths() ([]string, error)           { return []string{}, nil }
func (m *fakeModules) TestListLocation() []string               { return []string{testListPath} }
func (m *fakeModules) TestSpecs() []build.TestSpec              { return m.testSpecs }
func (m *fakeModules) TestDurations() []build.TestDuration      { return m.testDurations }
func (m *fakeModules) PackageRepositories() []build.PackageRepo { return m.packageRepositories }
func (m *fakeModules) PrebuiltVersions() ([]build.PrebuiltVersion, error) {
	return []build.PrebuiltVersion{
		{
			Name:    "fuchsia/third_party/android/aemu/release-gfxstream/${platform}",
			Version: "aemu_version",
		},
		{
			Name:    "fuchsia/third_party/qemu/${platform}",
			Version: "qemu_version",
		},
		{
			Name:    "fuchsia/third_party/crosvm/${platform}",
			Version: "crosvm_version",
		},
		{
			Name:    "fuchsia/third_party/edk2",
			Version: "edk2_version",
		},
		{
			Name:    "fuchsia/firmware/arm-trusted-firmware-qemu-bios",
			Version: "arm-trusted-firmware-qemu-bios_version",
		},
	}, nil
}
func (m *fakeModules) ProductBundles() []build.ProductBundle { return m.productBundles }
func (m *fakeModules) TefmocheckRulesManifest() []string     { return []string{} }
func (m *fakeModules) Tools() build.Tools {
	var tools build.Tools
	for _, tool := range []string{
		"ffx", "botanist", "ssh", "llvm-profdata",
		"fvm", "zbi", "llvm-symbolizer", "symbolizer",
		"tefmocheck", "triage", "resultdb", "perfcompare",
		"test_filterer", "resummarize", "test-pilot",
	} {
		tools = append(tools, build.Tool{
			Name: tool,
			Path: fmt.Sprintf("host_x64/%s", tool),
			OS:   "linux",
			CPU:  "x64",
		})
	}
	return tools
}
func (m *fakeModules) TriageSources() []string { return []string{} }

func packageURL(basename string) string {
	return fmt.Sprintf("fuchsia-pkg://fuchsia.com/%s#meta/%s.cm", basename, basename)
}

func fuchsiaTestSpec(basename string, deviceTypes ...string) build.TestSpec {
	var envs []build.Environment
	if len(deviceTypes) == 0 {
		deviceTypes = []string{"AEMU"}
	}
	for _, d := range deviceTypes {
		envs = append(envs, build.Environment{
			Dimensions: build.DimensionSet{
				"device_type": d,
			},
		})
	}
	return build.TestSpec{
		Test: build.Test{
			Name:        packageURL(basename),
			PackageURL:  packageURL(basename),
			OS:          "fuchsia",
			CPU:         "x64",
			Label:       fmt.Sprintf("//src/something:%s(//build/toolchain/fuchsia:x64)", basename),
			SourceLabel: fmt.Sprintf("//src/something:%s", basename),
		},
		Envs:       envs,
		ExpectsSSH: true,
	}
}

func isolated(spec build.TestSpec) build.TestSpec {
	spec.Isolated = true
	return spec
}

func bootTestSpec(basename string) build.TestSpec {
	return build.TestSpec{
		Test: build.Test{
			Name:        packageURL(basename),
			PackageURL:  packageURL(basename),
			OS:          "fuchsia",
			CPU:         "x64",
			Label:       fmt.Sprintf("//src/something:%s(//build/toolchain/fuchsia:x64)", basename),
			SourceLabel: fmt.Sprintf("//tools/other:%s", basename),
		},
		Envs: []build.Environment{
			{
				Dimensions: build.DimensionSet{
					"device_type": "AEMU",
				},
			},
		},
		ProductBundle: "boot-test_product_bundle",
		IsBootTest:    true,
	}
}

func hostTestSpec(basename string) build.TestSpec {
	testPath := fmt.Sprintf("host_x64/%s", basename)
	return build.TestSpec{
		Test: build.Test{
			Name:            testPath,
			Path:            testPath,
			OS:              "linux",
			CPU:             "x64",
			Label:           fmt.Sprintf("//tools/other:%s(//build/toolchain/host_x64)", basename),
			SourceLabel:     fmt.Sprintf("//tools/other:%s", basename),
			RuntimeDepsFile: filepath.Join("runtime_deps", basename+".json"),
		},
		Envs: []build.Environment{
			{
				Dimensions: build.DimensionSet{
					"cpu": "x64",
					"os":  "Linux",
				},
			},
		},
	}
}

func testListEntry(basename string, hermetic bool) build.TestListEntry {
	return build.TestListEntry{
		Name: packageURL(basename),
		Tags: []build.TestTag{
			{Key: "hermetic", Value: strconv.FormatBool(hermetic)},
		},
	}
}

func writeDeps(t *testing.T, buildDir string, testSpecs []build.TestSpec) {
	t.Helper()
	// Write runtime deps files.
	for _, testSpec := range testSpecs {
		if testSpec.Path != "" {
			touchFile(t, filepath.Join(buildDir, testSpec.Path))
		}
		if testSpec.RuntimeDepsFile == "" {
			continue
		}
		absPath := filepath.Join(buildDir, testSpec.RuntimeDepsFile)
		if err := os.MkdirAll(filepath.Dir(absPath), 0o700); err != nil {
			t.Fatal(err)
		}
		runtimeDeps := []string{"host_x64/dep1", "host_x64/dep2"}
		if err := jsonutil.WriteToFile(absPath, runtimeDeps); err != nil {
			t.Fatal(err)
		}
		for _, dep := range runtimeDeps {
			touchFile(t, filepath.Join(buildDir, dep))
		}
	}
}

func touchFile(t *testing.T, path string) {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, nil, 0o600); err != nil {
		t.Fatal(err)
	}
}

// readShards deserializes testsharder output from a JSON file.
func readShards(t *testing.T, path string) []testsharder.Shard {
	var shards []testsharder.Shard
	if err := jsonutil.ReadFromFile(path, &shards); err != nil {
		if errors.Is(err, os.ErrNotExist) && strings.HasPrefix(path, *goldensDir) {
			t.Fatalf("Golden file for case %q does not exist. To create it, run tools/integration/testsharder/update_goldens.sh", t.Name())
		}
		t.Fatal(err)
	}
	return shards
}

func writeTempJSONFile(t *testing.T, obj any) string {
	path := filepath.Join(t.TempDir(), "temp.json")
	if err := jsonutil.WriteToFile(path, obj); err != nil {
		t.Fatal(err)
	}
	return path
}

func writeTempFile(t *testing.T, contents string) string {
	path := filepath.Join(t.TempDir(), "temp.txt")
	if err := os.WriteFile(path, []byte(contents), 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}
