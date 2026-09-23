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
)

func TestNoticeRenderer_Run(t *testing.T) {
	outDir := t.TempDir()
	r := NewNoticeRenderer(outDir)

	projects := []*pipeline.Project{
		{
			RootPath: "third_party/foo",
			ClassifiedFiles: []pipeline.ClassifiedFile{
				{
					Path:          "third_party/foo/LICENSE",
					ProjectRoot:   "third_party/foo",
					IsLicenseFile: true,
					Matches: []pipeline.LicenseMatch{
						{SPDXID: "MIT", Text: []byte("MIT License Content Foo")},
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
						{SPDXID: "MIT", Text: []byte("MIT License Content Foo")}, // Duplicate, must dedupe
						{SPDXID: "Apache-2.0", Text: []byte("Apache License Content Bar")},
					},
				},
			},
		},
	}

	if err := r.Run(context.Background(), projects, nil); err != nil {
		t.Fatalf("NoticeRenderer.Run failed: %v", err)
	}

	noticeBytes, err := os.ReadFile(filepath.Join(outDir, "NOTICE.txt"))
	if err != nil {
		t.Fatalf("Failed to read NOTICE.txt: %v", err)
	}
	notice := string(noticeBytes)

	if !strings.Contains(notice, "MIT License Content Foo") {
		t.Errorf("NOTICE.txt missing MIT text")
	}
	if !strings.Contains(notice, "Apache License Content Bar") {
		t.Errorf("NOTICE.txt missing Apache text")
	}
	if count := strings.Count(notice, "MIT License Content Foo"); count != 1 {
		t.Errorf("Expected exactly 1 instance of deduped MIT text, found %d", count)
	}
}

func TestNoticeRenderer_EmptyOutDir(t *testing.T) {
	r := NewNoticeRenderer("")
	if err := r.Run(context.Background(), nil, nil); err != nil {
		t.Errorf("Expected nil error for empty OutDir, got %v", err)
	}
}
