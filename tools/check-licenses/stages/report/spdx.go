// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/metrics"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

// SpdxRenderer generates a minimal SPDX 2.3 JSON Software Bill of Materials (SBOM).
type SpdxRenderer struct {
	OutDir string
}

// NewSpdxRenderer creates a new SpdxRenderer.
func NewSpdxRenderer(outDir string) *SpdxRenderer {
	return &SpdxRenderer{
		OutDir: outDir,
	}
}

func (r *SpdxRenderer) Run(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
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

	spdxPath := filepath.Join(r.OutDir, "SPDX.json")
	spdxFile, err := os.Create(spdxPath)
	if err != nil {
		return fmt.Errorf("failed to create SPDX.json: %w", err)
	}
	defer spdxFile.Close()

	var hashes []string
	for hash := range uniqueLicenses {
		hashes = append(hashes, hash)
	}
	sort.Strings(hashes)

	var extracted []map[string]string
	for _, hash := range hashes {
		lic := uniqueLicenses[hash]
		extracted = append(extracted, map[string]string{
			"licenseId":     fmt.Sprintf("LicenseRef-%s", lic.Hash),
			"extractedText": string(lic.Text),
			"name":          lic.SPDXID,
		})
	}

	spdxData := map[string]interface{}{
		"SPDXID":                     "SPDXRef-DOCUMENT",
		"name":                       "Fuchsia Platform",
		"dataLicense":                "CC0-1.0",
		"hasExtractedLicensingInfos": extracted,
	}

	encoder := json.NewEncoder(spdxFile)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(spdxData); err != nil {
		return fmt.Errorf("failed to encode SPDX.json: %w", err)
	}

	return nil
}
