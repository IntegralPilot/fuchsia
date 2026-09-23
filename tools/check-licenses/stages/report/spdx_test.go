// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

func TestSpdxRenderer_Run(t *testing.T) {
	outDir := t.TempDir()
	r := NewSpdxRenderer(outDir)

	projects := []*pipeline.Project{
		{
			RootPath: "third_party/foo",
			ClassifiedFiles: []pipeline.ClassifiedFile{
				{
					Path:          "third_party/foo/LICENSE",
					ProjectRoot:   "third_party/foo",
					IsLicenseFile: true,
					Matches: []pipeline.LicenseMatch{
						{SPDXID: "MIT", Text: []byte("Sample MIT License")},
					},
				},
			},
		},
	}

	if err := r.Run(context.Background(), projects, nil); err != nil {
		t.Fatalf("SpdxRenderer.Run failed: %v", err)
	}

	spdxBytes, err := os.ReadFile(filepath.Join(outDir, "SPDX.json"))
	if err != nil {
		t.Fatalf("Failed to read SPDX.json: %v", err)
	}

	var data map[string]interface{}
	if err := json.Unmarshal(spdxBytes, &data); err != nil {
		t.Fatalf("SPDX.json is not valid JSON: %v", err)
	}

	if data["SPDXID"] != "SPDXRef-DOCUMENT" {
		t.Errorf("Expected SPDXRef-DOCUMENT, got %v", data["SPDXID"])
	}
	if data["name"] != "Fuchsia Platform" {
		t.Errorf("Expected Fuchsia Platform, got %v", data["name"])
	}
	infos, ok := data["hasExtractedLicensingInfos"].([]interface{})
	if !ok || len(infos) != 1 {
		t.Fatalf("Expected 1 extracted licensing info, got %v", infos)
	}
}

func TestSpdxRenderer_EmptyOutDir(t *testing.T) {
	r := NewSpdxRenderer("")
	if err := r.Run(context.Background(), nil, nil); err != nil {
		t.Errorf("Expected nil error for empty OutDir, got %v", err)
	}
}
