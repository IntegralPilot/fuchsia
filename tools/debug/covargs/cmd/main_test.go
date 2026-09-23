// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/google/go-cmp/cmp"
	"go.fuchsia.dev/fuchsia/tools/debug/covargs"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

func TestSplitVersion(t *testing.T) {
	for _, tc := range []struct {
		name            string
		arg             string
		expectedVersion string
		expectedPath    string
	}{
		{
			name:            "arg with key",
			arg:             "llvm_profdata=key",
			expectedVersion: "key",
			expectedPath:    "llvm_profdata",
		}, {
			name:            "arg without key",
			arg:             "llvm_profdata",
			expectedVersion: "",
			expectedPath:    "llvm_profdata",
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			version, path := covargs.SplitVersion(tc.arg)
			if version != tc.expectedVersion {
				t.Errorf("got version %s, want %s", version, tc.expectedVersion)
			}
			if path != tc.expectedPath {
				t.Errorf("got path %s, want %s", path, tc.expectedPath)
			}
		})
	}
}

func TestReadSummary(t *testing.T) {
	tempDir := t.TempDir()
	var summaryFiles []string
	for i := 0; i < 3; i++ {
		summaryBytes, err := json.Marshal(runtests.TestSummary{
			Tests: []runtests.TestDetails{
				{
					Name:      fmt.Sprintf("foo%d", i),
					Status:    runtests.TestSuccess,
					DataSinks: runtests.DataSinkMap{"llvm-profile": []runtests.DataSink{{Name: fmt.Sprintf("profile%d", i), File: fmt.Sprintf("llvm-profile/profile%d", i)}}},
				},
			},
		})
		if err != nil {
			t.Fatalf("failed to marshal summary: %s", err)
		}
		summaryFile := filepath.Join(tempDir, fmt.Sprintf("summary%d.json", i))
		if err := os.WriteFile(summaryFile, summaryBytes, os.ModePerm); err != nil {
			t.Fatalf("failed to write summary file: %s", err)
		}
		if i > 0 {
			summaryFile += "=version"
		}
		summaryFiles = append(summaryFiles, summaryFile)
	}

	expected := map[string]string{
		filepath.Join(tempDir, "llvm-profile/profile0"): "",
		filepath.Join(tempDir, "llvm-profile/profile1"): "version",
		filepath.Join(tempDir, "llvm-profile/profile2"): "version",
	}

	actual, err := readSummary(summaryFiles)
	if err != nil {
		t.Errorf("failed to read summaries: %s", err)
	}

	if diff := cmp.Diff(actual, expected); diff != "" {
		t.Errorf("Unexpected sinks (-got +want):\n%s", diff)
	}
}
