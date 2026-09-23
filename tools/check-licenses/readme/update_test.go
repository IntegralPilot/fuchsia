// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

func TestUpdateWithClassifiedFiles_DryRun(t *testing.T) {
	tempDir := t.TempDir()

	r := &Readme{
		Name:     "test_project",
		FilePath: filepath.Join(tempDir, "README.fuchsia"),
	}

	sourceFilePath := filepath.Join(tempDir, "source.cc")
	cf := pipeline.ClassifiedFile{
		Path:          sourceFilePath,
		IsLicenseFile: false,
		Matches: []pipeline.LicenseMatch{
			{
				SPDXID:    "MIT",
				MatchType: "Approved",
				Text:      []byte("Permission is hereby granted, free of charge..."),
			},
		},
		AnalyzedText: []byte("/* MIT License text */\nint main() {}"),
	}

	// 1. Dry run should generate notices in memory without writing to disk
	notices := UpdateWithClassifiedFilesDryRun(tempDir, tempDir, []*Readme{r}, []pipeline.ClassifiedFile{cf}, false)

	noticePath := filepath.Join(tempDir, "NOTICE.fuchsia")
	if _, ok := notices[noticePath]; !ok {
		t.Fatalf("Expected notices map to contain %s, got: %v", noticePath, notices)
	}

	content := notices[noticePath]
	if !strings.Contains(content, "source.cc") || !strings.Contains(content, "Permission is hereby granted") {
		t.Errorf("Generated notice content missing expected information, got:\n%s", content)
	}

	if len(r.Licenses) != 1 || r.Licenses[0] != "MIT" {
		t.Errorf("Expected r.Licenses to contain 'MIT', got: %v", r.Licenses)
	}

	if len(r.GeneratedNoticeFiles) != 1 || r.GeneratedNoticeFiles[0] != "NOTICE.fuchsia" {
		t.Errorf("Expected r.GeneratedNoticeFiles to be ['NOTICE.fuchsia'], got: %v", r.GeneratedNoticeFiles)
	}

	if _, err := os.Stat(noticePath); !os.IsNotExist(err) {
		t.Errorf("Expected NOTICE.fuchsia NOT to exist on disk in dry run mode")
	}

	// 2. Non-dry run should write NOTICE.fuchsia to disk
	writtenNotices := UpdateWithClassifiedFiles(tempDir, tempDir, []*Readme{r}, []pipeline.ClassifiedFile{cf}, false)
	if _, ok := writtenNotices[noticePath]; !ok {
		t.Fatalf("Expected written notices map to contain %s", noticePath)
	}

	diskBytes, err := os.ReadFile(noticePath)
	if err != nil {
		t.Fatalf("Failed to read generated NOTICE.fuchsia from disk: %v", err)
	}
	if string(diskBytes) != content {
		t.Errorf("Disk content does not match dry-run notice content.\nDisk:\n%s\nDryRun:\n%s", string(diskBytes), content)
	}
}

func TestUpdateWithClassifiedFiles_PrimaryLicenseFile(t *testing.T) {
	tempDir := t.TempDir()

	r := &Readme{
		Name:     "test_project",
		FilePath: filepath.Join(tempDir, "README.fuchsia"),
	}

	licenseFilePath := filepath.Join(tempDir, "LICENSE")
	cf := pipeline.ClassifiedFile{
		Path:          licenseFilePath,
		IsLicenseFile: true,
		Matches: []pipeline.LicenseMatch{
			{
				SPDXID:    "Apache-2.0",
				MatchType: "Approved",
			},
		},
	}

	notices := UpdateWithClassifiedFiles(tempDir, tempDir, []*Readme{r}, []pipeline.ClassifiedFile{cf}, false)

	if len(notices) != 0 {
		t.Errorf("Expected no NOTICE.fuchsia generated for primary license file, got: %v", notices)
	}

	if len(r.LicenseFiles) != 1 || r.LicenseFiles[0] != "LICENSE" {
		t.Errorf("Expected r.LicenseFiles to be ['LICENSE'], got: %v", r.LicenseFiles)
	}

	if len(r.Licenses) != 1 || r.Licenses[0] != "Apache-2.0" {
		t.Errorf("Expected r.Licenses to be ['Apache-2.0'], got: %v", r.Licenses)
	}
}
