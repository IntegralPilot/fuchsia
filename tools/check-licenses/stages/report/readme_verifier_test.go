// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/readme"
	"go.fuchsia.dev/fuchsia/tools/readme_fuchsia"
)

func TestReadmeVerifier_Success(t *testing.T) {
	tempDir := t.TempDir()
	readmePath := filepath.Join(tempDir, "README.fuchsia")

	orig := &readme_fuchsia.Readme{
		Name:             "foo",
		URL:              "https://foo",
		Version:          "1.0",
		SecurityCritical: "no",
	}
	clone := *orig

	proj := &pipeline.Project{
		RootPath: tempDir,
		Readme: &pipeline.ReadmeFile{
			Path: readmePath,
			Segments: []*pipeline.ReadmeSegment{
				{Original: orig, Updated: &clone},
			},
		},
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

	// Format accurately according to readme package
	readme.UpdateWithClassifiedFiles(tempDir, tempDir, proj.Readme.UpdatedSegments(), proj.FoundLicenses(), false)
	content := readme.Format(proj.Readme.UpdatedSegments())

	if err := os.WriteFile(readmePath, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	verifier := NewReadmeVerifier(tempDir)
	if err := verifier.Run(context.Background(), []*pipeline.Project{proj}, nil); err != nil {
		t.Errorf("Expected success, got: %v", err)
	}
}

func TestReadmeVerifier_OutOfDate(t *testing.T) {
	tempDir := t.TempDir()
	readmePath := filepath.Join(tempDir, "README.fuchsia")

	orig := &readme_fuchsia.Readme{
		Name:             "foo",
		URL:              "https://foo",
		Version:          "1.0",
		SecurityCritical: "no",
	}
	clone := *orig

	content := "Name: foo\nURL: https://foo\nVersion: 1.0\nSecurity Critical: no\n"
	if err := os.WriteFile(readmePath, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	proj := &pipeline.Project{
		RootPath: tempDir,
		Readme: &pipeline.ReadmeFile{
			Path: readmePath,
			Segments: []*pipeline.ReadmeSegment{
				{Original: orig, Updated: &clone},
			},
		},
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

	verifier := NewReadmeVerifier(tempDir)
	if err := verifier.Run(context.Background(), []*pipeline.Project{proj}, nil); err == nil {
		t.Errorf("Expected error for out-of-date README.fuchsia, got nil")
	}
}
