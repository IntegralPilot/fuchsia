// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"crypto/sha256"
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/metrics"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

type dedupedLicense struct {
	SPDXID string
	Text   []byte
	Hash   string
}

// NoticeRenderer aggregates and deduplicates all license texts across projects
// and writes the canonical NOTICE.txt archive.
type NoticeRenderer struct {
	OutDir string
}

// NewNoticeRenderer creates a new NoticeRenderer.
func NewNoticeRenderer(outDir string) *NoticeRenderer {
	return &NoticeRenderer{
		OutDir: outDir,
	}
}

func (r *NoticeRenderer) Run(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
	defer metrics.SpdxGenerationDuration.Track()()

	if r.OutDir == "" {
		return nil
	}

	uniqueLicenses := make(map[string]dedupedLicense)

	for _, proj := range projects {
		for _, cf := range proj.ClassifiedFiles {
			for _, match := range cf.Matches {
				metrics.LicenseDeduplication.Inc("raw_texts")
				h := sha256.New()
				h.Write(match.Text)
				hashStr := fmt.Sprintf("%x", h.Sum(nil))

				if _, exists := uniqueLicenses[hashStr]; !exists {
					uniqueLicenses[hashStr] = dedupedLicense{
						SPDXID: match.SPDXID,
						Text:   match.Text,
						Hash:   hashStr,
					}
					metrics.LicenseDeduplication.Inc("unique_texts")
				}
			}
		}
	}

	if err := os.MkdirAll(r.OutDir, 0755); err != nil {
		return fmt.Errorf("failed to create output directory: %w", err)
	}

	noticePath := filepath.Join(r.OutDir, "NOTICE.txt")
	noticeFile, err := os.Create(noticePath)
	if err != nil {
		return fmt.Errorf("failed to create NOTICE.txt: %w", err)
	}
	defer noticeFile.Close()

	var hashes []string
	for hash := range uniqueLicenses {
		hashes = append(hashes, hash)
	}
	sort.Strings(hashes)

	for _, hash := range hashes {
		l := uniqueLicenses[hash]
		separator := fmt.Sprintf("================================================================================\nSPDX: %s\n================================================================================\n\n", l.SPDXID)
		if _, err := noticeFile.WriteString(separator); err != nil {
			return fmt.Errorf("failed to write to NOTICE.txt: %w", err)
		}
		if _, err := noticeFile.Write(l.Text); err != nil {
			return fmt.Errorf("failed to write license text: %w", err)
		}
		if _, err := noticeFile.WriteString("\n\n"); err != nil {
			return fmt.Errorf("failed to write newline: %w", err)
		}
	}

	return nil
}
