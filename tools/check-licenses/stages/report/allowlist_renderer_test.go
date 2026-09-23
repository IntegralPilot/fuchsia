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

	"go.fuchsia.dev/fuchsia/tools/check-licenses/config"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

func TestAllowlistRenderer_Run(t *testing.T) {
	tempDir := t.TempDir()
	cfg := config.NewMasterConfig(tempDir)
	cfg.Classify.LicenseCategories = map[string]string{
		"MIT": "Restricted",
	}

	renderer := NewAllowlistRenderer(cfg)

	errors := []pipeline.ComplianceError{
		{
			CheckName: config.CheckNameAllLicensePatternUsagesMustBeApproved,
			LicenseID: "MIT",
			Project:   "src/foo/bar",
		},
	}

	if err := renderer.Run(context.Background(), nil, errors); err != nil {
		t.Fatalf("Expected Run to succeed, got: %v", err)
	}

	baseName := cfg.FindProjectBasename("src/foo/bar")
	expectedConfigPath := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "allowed_licenses", "Restricted", "MIT", baseName+".json")
	content, err := os.ReadFile(expectedConfigPath)
	if err != nil {
		t.Fatalf("Failed to read generated allowlist config at %s: %v", expectedConfigPath, err)
	}

	if !strings.Contains(string(content), "MIT") || !strings.Contains(string(content), "src/foo/bar") {
		t.Errorf("Expected allowlist config file to reference MIT and src/foo/bar, got:\n%s", string(content))
	}
}
