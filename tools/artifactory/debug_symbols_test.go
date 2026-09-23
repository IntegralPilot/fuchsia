// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifactory

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"

	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
)

func TestDebugSymbolUploads(t *testing.T) {
	checkout := t.TempDir()
	outputDir := filepath.Join(checkout, "out")
	if err := os.Mkdir(outputDir, 0o700); err != nil {
		t.Fatal(err)
	}

	debugSymbols := []ExportedDebugSymbol{
		{
			Debug:    filepath.Join(".build-id", "pr", "ebuiltA.debug"),
			Breakpad: filepath.Join(".build-id", "pr", "ebuiltA.sym"),
			GSYM:     filepath.Join(".build-id", "pr", "ebuiltA.gsym"),
			BuildID:  "b0000001",
			OS:       "fuchsia",
			CPU:      "arm64",
			Label:    "//prebuilt",
		},
		{
			Debug:    filepath.Join(".build-id", "pr", "ebuiltB.debug"),
			Breakpad: filepath.Join(".build-id", "pr", "ebuiltB.sym"),
			BuildID:  "b0000002",
			OS:       "linux",
			CPU:      "arm64",
			Label:    "//prebuilt",
		},
		{
			Debug:    filepath.Join(".build-id", "fi", "rst.debug"),
			DestPath: filepath.Join(".build-id", "fi", "rst"),
			Breakpad: filepath.Join(".build-id", "fi", "rst.sym"),
			GSYM:     filepath.Join(".build-id", "fi", "rst.gsym"),
			BuildID:  "b0000003",
			OS:       "fuchsia",
			CPU:      "arm64",
			Label:    "//first",
		},
		{
			Debug:    filepath.Join(".build-id", "se", "cond.debug"),
			Breakpad: filepath.Join(".build-id", "se", "cond.sym"),
			BuildID:  "b0000004",
			OS:       "linux",
			CPU:      "x64",
			Label:    "//second",
		},
		{
			Debug:    filepath.Join(".build-id", "th", "ird.debug"),
			DestPath: filepath.Join(".build-id", "th", "ird"),
			BuildID:  "b0000005",
			OS:       "linux",
			CPU:      "x64",
			Label:    "//third",
		},
	}
	if err := jsonutil.WriteToFile(filepath.Join(outputDir, "debug_symbols.json"), debugSymbols); err != nil {
		t.Fatal(err)
	}

	// Mapping from each file's local filepath to the locations in GCS to which
	// the file should be uploaded.
	expectedUploadDestinations := map[string][]string{
		filepath.Join(outputDir, "build-ids.txt"): {
			"TOP_NAMESPACE/build-ids.txt",
		},
		filepath.Join(outputDir, "build-ids.json"): {
			"TOP_NAMESPACE/build-ids.json",
		},
		filepath.Join(outputDir, "debug_symbols.json"): {
			"TOP_NAMESPACE/debug_symbols.json",
		},
		filepath.Join(outputDir, ".build-id", "fi", "rst.debug"): {
			"DEBUG_NAMESPACE/b0000003.debug",
			"BUILDID_NAMESPACE/b0000003/debuginfo",
			"BUILDID_NAMESPACE/b0000003/executable",
		},
		filepath.Join(outputDir, ".build-id", "fi", "rst.sym"): {
			"DEBUG_NAMESPACE/b0000003.sym",
			"BUILDID_NAMESPACE/b0000003/breakpad",
		},
		filepath.Join(outputDir, ".build-id", "fi", "rst.gsym"): {
			"DEBUG_NAMESPACE/b0000003.gsym",
			"BUILDID_NAMESPACE/b0000003/gsym",
		},
		filepath.Join(outputDir, ".build-id", "pr", "ebuiltA.debug"): {
			"DEBUG_NAMESPACE/b0000001.debug",
			"BUILDID_NAMESPACE/b0000001/debuginfo",
			"BUILDID_NAMESPACE/b0000001/executable",
		},
		filepath.Join(outputDir, ".build-id", "pr", "ebuiltA.sym"): {
			"DEBUG_NAMESPACE/b0000001.sym",
			"BUILDID_NAMESPACE/b0000001/breakpad",
		},
		filepath.Join(outputDir, ".build-id", "pr", "ebuiltA.gsym"): {
			"DEBUG_NAMESPACE/b0000001.gsym",
			"BUILDID_NAMESPACE/b0000001/gsym",
		},
		filepath.Join(outputDir, ".build-id", "pr", "ebuiltB.debug"): {
			"DEBUG_NAMESPACE/b0000002.debug",
			"BUILDID_NAMESPACE/b0000002/debuginfo",
			"BUILDID_NAMESPACE/b0000002/executable",
		},
		filepath.Join(outputDir, ".build-id", "pr", "ebuiltB.sym"): {
			"DEBUG_NAMESPACE/b0000002.sym",
			"BUILDID_NAMESPACE/b0000002/breakpad",
		},
		filepath.Join(outputDir, ".build-id", "se", "cond.debug"): {
			"DEBUG_NAMESPACE/b0000004.debug",
			"BUILDID_NAMESPACE/b0000004/debuginfo",
			"BUILDID_NAMESPACE/b0000004/executable",
		},
		filepath.Join(outputDir, ".build-id", "se", "cond.sym"): {
			"DEBUG_NAMESPACE/b0000004.sym",
			"BUILDID_NAMESPACE/b0000004/breakpad",
		},
		filepath.Join(outputDir, ".build-id", "th", "ird.debug"): {
			"DEBUG_NAMESPACE/b0000005.debug",
			"BUILDID_NAMESPACE/b0000005/debuginfo",
			"BUILDID_NAMESPACE/b0000005/executable",
		},
	}

	// All uploads should be compressed, except those in the following set.
	// See https://fxbug.dev/498773554 and https://fxbug.dev/42155140 for details.
	expectedUncompressedUploads := map[string]bool{
		filepath.Join(outputDir, "build-ids.txt"):      true,
		filepath.Join(outputDir, "debug_symbols.json"): true,
	}

	var expectedUploads []Upload
	for src, destinations := range expectedUploadDestinations {
		_, uncompress := expectedUncompressedUploads[src]
		for _, dest := range destinations {
			expectedUploads = append(expectedUploads, Upload{
				Source:      src,
				Destination: dest,
				Compress:    !uncompress,
				Deduplicate: true,
			})
		}
	}

	actualUploads, err := debugSymbolUploads(outputDir, "TOP_NAMESPACE", "DEBUG_NAMESPACE", "BUILDID_NAMESPACE")
	if err != nil {
		t.Fatalf("failed to generate debug binary uploads: %v", err)
	}
	opts := cmpopts.SortSlices(func(a, b Upload) bool { return a.Destination < b.Destination })
	if diff := cmp.Diff(expectedUploads, actualUploads, opts); diff != "" {
		t.Fatalf("unexpected debug binary uploads (-want +got):\n%s", diff)
	}
}
