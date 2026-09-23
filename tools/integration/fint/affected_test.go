// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fint

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
	"go.fuchsia.dev/fuchsia/tools/build"
	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
)

type mockBuildAPIClient struct {
	affectedTests []string
	recordedFiles []string
	err           error
}

func (m *mockBuildAPIClient) ExportDebugSymbols(ctx context.Context, outputDir string, withBreakpad bool) error {
	return nil
}

func (m *mockBuildAPIClient) AffectedTests(ctx context.Context, filesListPath string) ([]string, error) {
	if m.err != nil {
		return nil, m.err
	}
	content, err := os.ReadFile(filesListPath)
	if err == nil && len(content) > 0 {
		m.recordedFiles = filepath.SplitList(string(content))
	}
	return m.affectedTests, nil
}

func TestResolveAffectedTestNames(t *testing.T) {
	testSpecs := []build.TestSpec{
		{
			Test: build.Test{
				Name:  "gn_test_name",
				Label: "//src/foo:foo_test(//build/toolchain/fuchsia:arm64)",
			},
		},
		{
			Test: build.Test{
				Name:        "bazel_test_name",
				Label:       "@@//src/bazel:bar_test",
				SourceLabel: "//src/bazel:bar_test",
			},
		},
		{
			Test: build.Test{
				Name:  "recovery_sim_test",
				Label: "//src/recovery/simulator:recovery_simulator_boot_test(//build/toolchain:x64)",
			},
		},
		{
			Test: build.Test{
				Name:  "unaffected_test",
				Label: "//src/other:other_test(//build/toolchain/fuchsia:arm64)",
			},
		},
	}

	clientOutput := []string{
		"//src/foo:foo_test(//build/toolchain/fuchsia:arm64),device",
		"@@//src/bazel:bar_test,host",
		"//src/recovery/simulator:recovery_simulator_boot_test(//build/toolchain:x64),device",
	}

	got := resolveAffectedTestNames(testSpecs, clientOutput)
	want := []string{"bazel_test_name", "gn_test_name"}

	if diff := cmp.Diff(want, got); diff != "" {
		t.Errorf("resolveAffectedTestNames mismatch (-want +got):\n%s", diff)
	}
}

func TestWriteChangedFilesList(t *testing.T) {
	files := []*fintpb.Context_ChangedFile{
		{Path: "src/foo.cc"},
		{Path: "src/bar.py"},
	}

	artifactDir := t.TempDir()
	path, cleanup, err := writeChangedFilesList(context.Background(), artifactDir, files)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if !strings.HasPrefix(path, artifactDir) {
		t.Errorf("expected path to be inside artifactDir %q, got %q", artifactDir, path)
	}

	content, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("failed to read written file: %v", err)
	}

	expected := "src/foo.cc\nsrc/bar.py\n"
	if string(content) != expected {
		t.Errorf("writeChangedFilesList content got %q, want %q", string(content), expected)
	}

	cleanup()
	if _, err := os.Stat(path); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("expected file %q to be deleted after cleanup, got err: %v", path, err)
	}
}

type fakeRunnerForDualRun struct {
	mockStdout []byte
}

func (r *fakeRunnerForDualRun) Run(ctx context.Context, cmd []string, options subprocess.RunOptions) error {
	if options.Stdout != nil {
		options.Stdout.Write(r.mockStdout)
	}
	return nil
}

func TestAffectedImplDualRun(t *testing.T) {
	checkoutDir := t.TempDir()
	artifactDir := t.TempDir()
	buildDir := t.TempDir()

	testFile := "src/foo.cc"
	absPath := filepath.Join(checkoutDir, testFile)
	if err := os.MkdirAll(filepath.Dir(absPath), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(absPath, []byte("content"), 0o600); err != nil {
		t.Fatal(err)
	}

	contextSpec := &fintpb.Context{
		CheckoutDir: checkoutDir,
		BuildDir:    buildDir,
		ArtifactDir: artifactDir,
		ChangedFiles: []*fintpb.Context_ChangedFile{
			{Path: testFile},
		},
	}

	testSpecs := []build.TestSpec{
		{
			Test: build.Test{
				Name:  "gn_test",
				Label: "//src/foo:gn_test(//build/toolchain:arm64)",
			},
		},
		{
			Test: build.Test{
				Name:  "bazel_test",
				Label: "@@//src/bazel:bazel_test",
			},
		},
	}

	modules := fakeBuildModules{
		buildDir:  buildDir,
		testSpecs: testSpecs,
		tools: build.Tools{
			{
				Name: "ninja",
				Path: "ninja",
				OS:   "linux",
				CPU:  "x64",
			},
		},
	}

	runner := &fakeRunnerForDualRun{
		mockStdout: []byte("ninja explain: obj/src/foo/package_manifest.json is dirty\n"),
	}

	client := &mockBuildAPIClient{
		affectedTests: []string{
			"//src/foo:gn_test(//build/toolchain:arm64),device",
			"@@//src/bazel:bazel_test,host",
		},
	}

	platform := "linux-x64"
	targets := []string{"default"}

	artifacts, err := affectedImpl(context.Background(), runner, client, contextSpec, modules, platform, targets)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Verify comparison log was saved
	comparisonLogPath, ok := artifacts.LogFiles["affected_tests_comparison.json"]
	if !ok {
		t.Fatalf("expected affected_tests_comparison.json in artifacts.LogFiles")
	}

	comparisonContent, err := os.ReadFile(comparisonLogPath)
	if err != nil {
		t.Fatalf("failed to read comparison log: %v", err)
	}

	var report map[string]any
	if err := json.Unmarshal(comparisonContent, &report); err != nil {
		t.Fatalf("failed to parse comparison log JSON: %v", err)
	}

	if report["new_affected_tests"] == nil {
		t.Errorf("expected new_affected_tests in report")
	}
	if report["legacy_affected_tests"] == nil {
		t.Errorf("expected legacy_affected_tests in report")
	}
	if report["legacy_duration_seconds"] == nil {
		t.Errorf("expected legacy_duration_seconds in report")
	} else if dur, ok := report["legacy_duration_seconds"].(float64); !ok || dur < 0 {
		t.Errorf("expected legacy_duration_seconds to be non-negative float, got %v", report["legacy_duration_seconds"])
	}
	if report["new_duration_seconds"] == nil {
		t.Errorf("expected new_duration_seconds in report")
	} else if dur, ok := report["new_duration_seconds"].(float64); !ok || dur < 0 {
		t.Errorf("expected new_duration_seconds to be non-negative float, got %v", report["new_duration_seconds"])
	}
}
