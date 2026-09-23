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
	"time"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

func TestMultiRenderer_RunSuccess(t *testing.T) {
	outDir := t.TempDir()
	renderers := pipeline.MultiRenderer{
		NewNoticeRenderer(outDir),
		NewSpdxRenderer(outDir),
		NewMetricsRenderer(outDir),
	}

	projects := []*pipeline.Project{
		{
			RootPath: "third_party/foo",
			ClassifiedFiles: []pipeline.ClassifiedFile{
				{
					Path:          "third_party/foo/LICENSE",
					ProjectRoot:   "third_party/foo",
					IsLicenseFile: true,
					Matches: []pipeline.LicenseMatch{
						{SPDXID: "MIT", Text: []byte("MIT License Text A")},
					},
				},
			},
		},
		{
			RootPath: "third_party/bar",
			ClassifiedFiles: []pipeline.ClassifiedFile{
				{
					Path:          "third_party/bar/LICENSE",
					ProjectRoot:   "third_party/bar",
					IsLicenseFile: true,
					Matches: []pipeline.LicenseMatch{
						{SPDXID: "MIT", Text: []byte("MIT License Text A")},
						{SPDXID: "Apache-2.0", Text: []byte("Apache License Text B")},
					},
				},
			},
		},
	}

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	err := renderers.Run(ctx, projects, nil)
	if err != nil {
		t.Fatalf("Expected successful run, got error: %v", err)
	}

	noticeBytes, err := os.ReadFile(filepath.Join(outDir, "NOTICE.txt"))
	if err != nil {
		t.Fatal("Failed to read NOTICE.txt")
	}
	notice := string(noticeBytes)

	if !strings.Contains(notice, "MIT License Text A") {
		t.Error("NOTICE.txt missing MIT text")
	}

	spdxBytes, err := os.ReadFile(filepath.Join(outDir, "SPDX.json"))
	if err != nil {
		t.Fatal("Failed to read SPDX.json")
	}
	spdx := string(spdxBytes)

	if !strings.Contains(spdx, "SPDXRef-DOCUMENT") {
		t.Error("SPDX.json missing Document Ref")
	}
}

func TestConsoleErrorReporter_RunFailure(t *testing.T) {
	reporter := NewConsoleErrorReporter(t.TempDir())

	errors := []pipeline.ComplianceError{
		{
			CheckName: "UnallowedLicense",
			Project:   "third_party/bad",
			FilePath:  "third_party/bad/LICENSE",
			Issue:     "Unallowed license GPL-2.0",
		},
	}

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	err := reporter.Run(ctx, nil, errors)
	if err == nil {
		t.Fatal("Expected error due to compliance violation, got nil")
	}

	if !strings.Contains(err.Error(), "1 error") {
		t.Errorf("Expected error to contain error count, got: %v", err)
	}
}
