// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"os"
	"path/filepath"
	"testing"

	"github.com/google/subcommands"
)

func TestAllowlistCommand_Execute(t *testing.T) {
	tempDir := t.TempDir()

	origEnv := os.Getenv("FUCHSIA_DIR")
	os.Setenv("FUCHSIA_DIR", tempDir)
	defer os.Setenv("FUCHSIA_DIR", origEnv)

	origWd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	defer os.Chdir(origWd)
	if err := os.Chdir(tempDir); err != nil {
		t.Fatal(err)
	}

	// Scaffold the recursive config system
	seedConfig := filepath.Join(tempDir, "tools", "check-licenses", "config.json")
	os.MkdirAll(filepath.Dir(seedConfig), 0755)
	os.WriteFile(seedConfig, []byte(`{"includes": ["tools/check-licenses/assets"]}`), 0644)

	// Create command
	cmd := &AllowlistCommand{
		fuchsiaDir: tempDir,
	}

	ctx := context.Background()

	// Create dummy category and directories
	catDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "allowed_licenses", "Restricted", "GPL-2.0")
	if err := os.MkdirAll(catDir, 0755); err != nil {
		t.Fatal(err)
	}
	os.MkdirAll(filepath.Join(tempDir, "src", "foo", "bar"), 0755)
	os.MkdirAll(filepath.Join(tempDir, "vendor", "my_private_proj"), 0755)
	os.WriteFile(filepath.Join(tempDir, "vendor", "my_private_proj", "LICENSE"), []byte("license text"), 0644)

	// Test 1: Missing arguments
	f1 := flag.NewFlagSet("test1", flag.ContinueOnError)
	f1.Parse([]string{})
	if status := cmd.Execute(ctx, f1); status != subcommands.ExitUsageError {
		t.Errorf("Expected ExitUsageError for missing args, got %v", status)
	}

	// Test 2: Public project
	f2 := flag.NewFlagSet("test2", flag.ContinueOnError)
	cmd.SetFlags(f2)
	f2.Parse([]string{"add", "-bug", "b/123", "GPL-2.0", "src/foo/bar"})
	if status := cmd.Execute(ctx, f2); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for public project, got %v", status)
	}

	publicConfigPath := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "allowed_licenses", "Restricted", "GPL-2.0", "foo.json")
	if _, err := os.Stat(publicConfigPath); os.IsNotExist(err) {
		t.Errorf("Expected config file to be created at %s", publicConfigPath)
	}

	// Create mock private manifest for Test 3
	integrationDir := filepath.Join(tempDir, "integration", "internal", "vendor", "google")
	if err := os.MkdirAll(integrationDir, 0755); err != nil {
		t.Fatal(err)
	}
	privateManifest := filepath.Join(integrationDir, "third_party")
	privateContent := `<?xml version="1.0" encoding="UTF-8"?>
<manifest>
  <project name="my_private_proj" path="vendor/my_private_proj"/>
</manifest>`
	os.WriteFile(privateManifest, []byte(privateContent), 0644)

	// Test 3: Private project (vendor/...)
	f3 := flag.NewFlagSet("test3", flag.ContinueOnError)
	cmd.SetFlags(f3)
	f3.Parse([]string{"add", "-bug", "b/123", "GPL-2.0", "vendor/my_private_proj"})
	if status := cmd.Execute(ctx, f3); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for private project, got %v", status)
	}

	privateConfigPath := filepath.Join(tempDir, "vendor", "google", "tools", "check-licenses", "assets", "configs", "allowed_licenses", "Restricted", "GPL-2.0", "my_private_proj.json")
	if _, err := os.Stat(privateConfigPath); os.IsNotExist(err) {
		t.Errorf("Expected config file to be created at %s", privateConfigPath)
	}

	// Test 4: Already exists (should not fail, should exit success)
	f4 := flag.NewFlagSet("test4", flag.ContinueOnError)
	cmd.SetFlags(f4)
	f4.Parse([]string{"add", "-bug", "b/123", "GPL-2.0", "src/foo/bar"})
	if status := cmd.Execute(ctx, f4); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess when allowlist entry already exists, got %v", status)
	}

	// Test 5: Third party file grouping (should group by project name from manifest)
	f5 := flag.NewFlagSet("test5", flag.ContinueOnError)
	cmd.SetFlags(f5)
	f5.Parse([]string{"add", "-bug", "b/123", "GPL-2.0", "vendor/my_private_proj/LICENSE"})
	if status := cmd.Execute(ctx, f5); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for third party file, got %v", status)
	}

	thirdPartyConfigPath := filepath.Join(tempDir, "vendor", "google", "tools", "check-licenses", "assets", "configs", "allowed_licenses", "Restricted", "GPL-2.0", "my_private_proj.json")
	if _, err := os.Stat(thirdPartyConfigPath); os.IsNotExist(err) {
		t.Errorf("Expected config file to be created at %s", thirdPartyConfigPath)
	}

	// Test 6: Missing bug flag (should fail)
	f6 := flag.NewFlagSet("test6", flag.ContinueOnError)
	cmd.SetFlags(f6)
	f6.Parse([]string{"add", "GPL-2.0", "src/foo/bar"})
	if status := cmd.Execute(ctx, f6); status != subcommands.ExitUsageError {
		t.Errorf("Expected ExitUsageError for missing -bug flag, got %v", status)
	}

	// Test 7: Misplaced flags (UX Check, should fail)
	f7 := flag.NewFlagSet("test7", flag.ContinueOnError)
	cmd.SetFlags(f7)
	f7.Parse([]string{"add", "GPL-2.0", "src/foo/bar", "-bug", "b/123"})
	if status := cmd.Execute(ctx, f7); status != subcommands.ExitUsageError {
		t.Errorf("Expected ExitUsageError for misplaced flags, got %v", status)
	}
}

func TestAllowlistCommand_Execute_RelativePathFromSubdir(t *testing.T) {
	tempDir := t.TempDir()

	origEnv := os.Getenv("FUCHSIA_DIR")
	os.Setenv("FUCHSIA_DIR", tempDir)
	defer os.Setenv("FUCHSIA_DIR", origEnv)

	// Scaffold the recursive config system
	seedConfig := filepath.Join(tempDir, "tools", "check-licenses", "config.json")
	os.MkdirAll(filepath.Dir(seedConfig), 0755)
	os.WriteFile(seedConfig, []byte(`{"includes": ["tools/check-licenses/assets"]}`), 0644)

	cmd := &AllowlistCommand{
		fuchsiaDir: tempDir,
	}

	ctx := context.Background()

	// Create placeholder category
	catDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "allowed_licenses", "Restricted", "GPL-2.0")
	if err := os.MkdirAll(catDir, 0755); err != nil {
		t.Fatal(err)
	}

	// Simulate running from a subdirectory
	origWd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	defer os.Chdir(origWd)

	subdir := filepath.Join(tempDir, "src", "my_project")
	if err := os.MkdirAll(subdir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.Chdir(subdir); err != nil {
		t.Fatal(err)
	}

	f := flag.NewFlagSet("test_relative", flag.ContinueOnError)
	cmd.SetFlags(f)
	f.Parse([]string{"add", "-bug", "b/123", "GPL-2.0", "."}) // target is "." (src/my_project)

	if status := cmd.Execute(ctx, f); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess, got %v", status)
	}

	// Expected config file should be named "my_project.json" under the check name directory
	expectedConfigPath := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "allowed_licenses", "Restricted", "GPL-2.0", "my_project.json")
	if _, err := os.Stat(expectedConfigPath); os.IsNotExist(err) {
		t.Errorf("Expected config file to be created at %s", expectedConfigPath)
	}
}

func TestAllowlistCommand_Execute_UnknownLicenseName(t *testing.T) {
	tempDir := t.TempDir()

	origEnv := os.Getenv("FUCHSIA_DIR")
	os.Setenv("FUCHSIA_DIR", tempDir)
	defer os.Setenv("FUCHSIA_DIR", origEnv)

	cmd := &AllowlistCommand{
		fuchsiaDir: tempDir,
	}

	ctx := context.Background()
	f := flag.NewFlagSet("test_unknown_license", flag.ContinueOnError)
	cmd.SetFlags(f)
	f.Parse([]string{"add", "-bug", "b/123", "GPLO-2.0", "src/foo/bar"})

	if status := cmd.Execute(ctx, f); status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure for unknown license name, got %v", status)
	}
}
