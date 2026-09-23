// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/readme_fuchsia"
)

func TestReadmeWriter_Run(t *testing.T) {
	tempDir := t.TempDir()
	readmePath := filepath.Join(tempDir, "README.fuchsia")

	orig := &readme_fuchsia.Readme{
		Name:             "foo",
		URL:              "https://foo",
		Version:          "1.0",
		SecurityCritical: "no",
	}
	clone := *orig

	readmeFile := &pipeline.ReadmeFile{
		Path: readmePath,
		Segments: []*pipeline.ReadmeSegment{
			{
				Original: orig,
				Updated:  &clone,
			},
		},
	}

	proj := &pipeline.Project{
		RootPath: tempDir,
		Readme:   readmeFile,
		ClassifiedFiles: []pipeline.ClassifiedFile{
			{
				Path:          filepath.Join(tempDir, "LICENSE"),
				ProjectRoot:   tempDir,
				IsLicenseFile: true,
				Matches: []pipeline.LicenseMatch{
					{SPDXID: "MIT", Text: []byte("MIT text")},
				},
			},
		},
	}

	writer := NewReadmeWriter(tempDir, false)
	if err := writer.Run(context.Background(), []*pipeline.Project{proj}, nil); err != nil {
		t.Fatalf("ReadmeWriter.Run failed: %v", err)
	}

	contentBytes, err := os.ReadFile(readmePath)
	if err != nil {
		t.Fatalf("Failed to read updated README.fuchsia: %v", err)
	}
	content := string(contentBytes)

	if !strings.Contains(content, "License File: LICENSE") {
		t.Errorf("Expected README.fuchsia to contain 'License File: LICENSE', got:\n%s", content)
	}
}

func TestReadmeWriter_PrintStdout(t *testing.T) {
	writer := NewReadmeWriter(t.TempDir(), true)
	orig := &readme_fuchsia.Readme{Name: "foo"}
	clone := *orig

	proj := &pipeline.Project{
		RootPath: t.TempDir(),
		Readme: &pipeline.ReadmeFile{
			Path: filepath.Join(t.TempDir(), "README.fuchsia"),
			Segments: []*pipeline.ReadmeSegment{
				{Original: orig, Updated: &clone},
			},
		},
	}

	if err := writer.Run(context.Background(), []*pipeline.Project{proj}, nil); err != nil {
		t.Errorf("Expected nil error for PrintStdout, got %v", err)
	}
}
