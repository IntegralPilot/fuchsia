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

func TestFixerRenderer_CopyrightFix(t *testing.T) {
	tempDir := t.TempDir()
	sourceFile := filepath.Join(tempDir, "main.cc")
	if err := os.WriteFile(sourceFile, []byte("int main() { return 0; }\n"), 0644); err != nil {
		t.Fatalf("Failed to write test source file: %v", err)
	}

	cfg := config.NewMasterConfig(tempDir)
	renderer := NewFixerRenderer(tempDir, cfg)
	errors := []pipeline.ComplianceError{
		{
			CheckName: PolicyCheckAllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders,
			FilePath:  sourceFile,
		},
	}

	if err := renderer.Run(context.Background(), nil, errors); err != nil {
		t.Fatalf("Expected Run to succeed, got: %v", err)
	}

	content, err := os.ReadFile(sourceFile)
	if err != nil {
		t.Fatalf("Failed to read modified source file: %v", err)
	}

	if !strings.Contains(string(content), "Copyright") || !strings.Contains(string(content), "The Fuchsia Authors") {
		t.Errorf("Expected copyright header to be inserted, got content:\n%s", string(content))
	}

	if len(renderer.FixedCount["Copyright Headers"]) != 1 {
		t.Errorf("Expected 1 fixed copyright header record, got %d", len(renderer.FixedCount["Copyright Headers"]))
	}
}

func TestFixerRenderer_PolicyExceptionFix(t *testing.T) {
	tempDir := t.TempDir()
	cfg := config.NewMasterConfig(tempDir)
	renderer := NewFixerRenderer(tempDir, cfg)

	errors := []pipeline.ComplianceError{
		{
			CheckName: PolicyCheckAllProjectsMustHaveALicense,
			Project:   "src/test/my_project",
		},
	}

	if err := renderer.Run(context.Background(), nil, errors); err != nil {
		t.Fatalf("Expected Run to succeed, got: %v", err)
	}

	if len(renderer.NewConfigFiles) != 1 {
		t.Fatalf("Expected 1 new config file generated, got %d", len(renderer.NewConfigFiles))
	}

	configPath := renderer.NewConfigFiles[0]
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		t.Errorf("Expected generated config file to exist on disk at %s", configPath)
	}

	content, err := os.ReadFile(configPath)
	if err != nil {
		t.Fatalf("Failed to read generated policy exception config: %v", err)
	}

	if !strings.Contains(string(content), "src/test/my_project") {
		t.Errorf("Expected policy exception file to reference project path, got:\n%s", string(content))
	}
}

func TestFixerRenderer_AllowlistFix(t *testing.T) {
	tempDir := t.TempDir()
	cfg := config.NewMasterConfig(tempDir)
	cfg.Classify.LicenseCategories = map[string]string{
		"GPL-2.0": "Restricted",
	}
	renderer := NewFixerRenderer(tempDir, cfg)

	errors := []pipeline.ComplianceError{
		{
			CheckName: CheckNameAllLicensePatternUsagesMustBeApproved,
			LicenseID: "GPL-2.0",
			Project:   "third_party/some_lib",
		},
	}

	if err := renderer.Run(context.Background(), nil, errors); err != nil {
		t.Fatalf("Expected Run to succeed, got: %v", err)
	}

	if len(renderer.NewConfigFiles) != 1 {
		t.Fatalf("Expected 1 new allowlist config file generated, got %d", len(renderer.NewConfigFiles))
	}

	configPath := renderer.NewConfigFiles[0]
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		t.Errorf("Expected generated allowlist config file to exist on disk at %s", configPath)
	}

	content, err := os.ReadFile(configPath)
	if err != nil {
		t.Fatalf("Failed to read generated allowlist config: %v", err)
	}

	if !strings.Contains(string(content), "GPL-2.0") || !strings.Contains(string(content), "third_party/some_lib") {
		t.Errorf("Expected allowlist config file to reference license and project, got:\n%s", string(content))
	}
}
