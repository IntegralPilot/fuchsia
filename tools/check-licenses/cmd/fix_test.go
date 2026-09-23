// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"bytes"
	"context"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/subcommands"
)

func TestFixCommand_Execute(t *testing.T) {
	tempDir := t.TempDir()
	allowlistPath := filepath.Join(tempDir, "vendor", "google", "tools", "check-licenses", "assets", "configs", "allowed_licenses", "Restricted", "GPL-2.0", "testproj.json")

	// 1. Scaffold the recursive config system
	seedConfig := filepath.Join(tempDir, "tools", "check-licenses", "config.json")
	os.MkdirAll(filepath.Dir(seedConfig), 0755)
	os.WriteFile(seedConfig, []byte(`{"includes": ["tools/check-licenses/assets"]}`), 0644)

	// Scaffold necessary assets for the v2 pipeline to run
	// Patterns for detection
	patternsBase := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns")
	copyrightPatternDir := filepath.Join(patternsBase, "_Header", "FuchsiaCopyright")
	os.MkdirAll(copyrightPatternDir, 0755)
	os.WriteFile(filepath.Join(copyrightPatternDir, "fuchsia.txt"), []byte("Copyright 2026 The Fuchsia Authors. All rights reserved."), 0644)

	mitPatternDir := filepath.Join(patternsBase, "Permissive", "MIT")
	os.MkdirAll(mitPatternDir, 0755)
	os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte("MIT License"), 0644)

	gplPatternDir := filepath.Join(patternsBase, "Restricted", "GPL-2.0")
	os.MkdirAll(gplPatternDir, 0755)
	os.WriteFile(filepath.Join(gplPatternDir, "gpl.txt"), []byte("GPL License"), 0644)

	// Configs for category discovery
	gplConfigDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "allowed_licenses", "Restricted", "GPL-2.0")
	os.MkdirAll(gplConfigDir, 0755)
	os.WriteFile(filepath.Join(gplConfigDir, "default.json"), []byte("{}"), 0644)

	copyrightExtDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "copyright_extensions")
	os.MkdirAll(copyrightExtDir, 0755)
	os.WriteFile(filepath.Join(copyrightExtDir, "default.json"), []byte(`{"copyright_extensions": {"extensions": [".cc"]}}`), 0644)

	// 2. Create a project with multiple issues
	// 1st project: Root (First-party)
	// Issue 1: Missing Copyright Header
	sourceFile := filepath.Join(tempDir, "main.cc")
	os.WriteFile(sourceFile, []byte("void main() {\n}\n"), 0644)

	// Issue 2: Out of date README (missing the LICENSE file we're about to add)
	readmePath := filepath.Join(tempDir, "README.fuchsia")
	os.WriteFile(readmePath, []byte("Name: fuchsia\nURL: http://fuchsia.dev\nVersion: 1.0\nSecurity Critical: yes\n"), 0644)

	licenseFile := filepath.Join(tempDir, "LICENSE")
	os.WriteFile(licenseFile, []byte("MIT License"), 0644)

	// 2nd project: vendor/testproj (Third-party)
	vendorDir := filepath.Join(tempDir, "vendor", "testproj")
	os.MkdirAll(vendorDir, 0755)
	os.WriteFile(filepath.Join(vendorDir, "README.fuchsia"), []byte("Name: testproj\nURL: http://test.com\nVersion: 1.0\nSecurity Critical: no\nLicense File: LICENSE\n"), 0644)
	os.WriteFile(filepath.Join(vendorDir, "LICENSE"), []byte("GPL License"), 0644)

	// Create mock private manifest for vendor/testproj
	integrationDir := filepath.Join(tempDir, "integration", "internal", "vendor", "google")
	if err := os.MkdirAll(integrationDir, 0755); err != nil {
		t.Fatal(err)
	}
	privateManifest := filepath.Join(integrationDir, "third_party")
	privateContent := `<?xml version="1.0" encoding="UTF-8"?>
<manifest>
  <project name="testproj" path="vendor/testproj"/>
</manifest>`
	os.WriteFile(privateManifest, []byte(privateContent), 0644)

	// 3. Initialize and run the FixCommand on tempDir
	cmd := &FixCommand{}
	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{tempDir})
	cmd.fuchsiaDir = tempDir
	ctx := context.Background()

	// Capture stdout
	oldStdout := os.Stdout
	r, w, _ := os.Pipe()
	os.Stdout = w

	status := cmd.Execute(ctx, fs)

	w.Close()
	os.Stdout = oldStdout

	var buf bytes.Buffer
	buf.ReadFrom(r)
	output := buf.String()

	if status != subcommands.ExitSuccess {
		t.Fatalf("Expected fix command to exit with success, got %v", status)
	}

	if !strings.Contains(output, "Please update the 'bug' field in these files:") {
		t.Errorf("Expected output to contain action required section, got: %s", output)
	}
	relAllowlist, _ := filepath.Rel(tempDir, allowlistPath)
	if !strings.Contains(output, relAllowlist) {
		t.Errorf("Expected output to contain new config file path %s, got: %s", relAllowlist, output)
	}

	// 4. Verify fixes

	// Verify Fix 1: Copyright header added to root file
	content, _ := os.ReadFile(sourceFile)
	if !strings.Contains(string(content), "The Fuchsia Authors") {
		t.Errorf("Expected copyright header to be added to %s. Content:\n%s", sourceFile, string(content))
	}

	// Verify Fix 2: README updated with license info
	readmeContent, _ := os.ReadFile(readmePath)
	if !strings.Contains(string(readmeContent), "License File: LICENSE") {
		t.Errorf("Expected README to be updated with license file info. Content:\n%s", string(readmeContent))
	}

	// Verify Fix 3: Allowlist entry created for GPL-2.0 in vendor project
	if _, err := os.Stat(allowlistPath); os.IsNotExist(err) {
		t.Errorf("Expected allowlist config file to be created at %s", allowlistPath)
	}
}

func TestFixCommand_Failure(t *testing.T) {
	tempDir := t.TempDir()

	// Scaffold minimal patterns for copyright
	patternsBase := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns")
	copyrightPatternDir := filepath.Join(patternsBase, "_Header", "FuchsiaCopyright")
	os.MkdirAll(copyrightPatternDir, 0755)
	os.WriteFile(filepath.Join(copyrightPatternDir, "fuchsia.txt"), []byte("Copyright 2026 The Fuchsia Authors. All rights reserved."), 0644)

	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)
	seedConfig := filepath.Join(tempDir, "tools", "check-licenses", "config.json")
	os.MkdirAll(filepath.Dir(seedConfig), 0755)
	os.WriteFile(seedConfig, []byte(`{"includes": ["tools/check-licenses/assets"]}`), 0644)

	copyrightExtDir2 := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "copyright_extensions")
	os.MkdirAll(copyrightExtDir2, 0755)
	os.WriteFile(filepath.Join(copyrightExtDir2, "default.json"), []byte(`{"copyright_extensions": {"extensions": [".cc"]}}`), 0644)

	// Create a minimal README.fuchsia to define the project
	readmePath := filepath.Join(tempDir, "README.fuchsia")
	os.WriteFile(readmePath, []byte("Name: fuchsia\nURL: http://fuchsia.dev\nVersion: 1.0\nSecurity Critical: yes\n"), 0644)

	// Create a file missing copyright
	sourceFile := filepath.Join(tempDir, "main.cc")
	os.WriteFile(sourceFile, []byte("void main() {}\n"), 0644)

	// Make it read-only!
	if err := os.Chmod(sourceFile, 0444); err != nil {
		t.Fatal(err)
	}
	defer os.Chmod(sourceFile, 0644) // restore after test

	cmd := &FixCommand{}
	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{tempDir})
	cmd.fuchsiaDir = tempDir

	// Capture stderr
	oldStderr := os.Stderr
	r, w, _ := os.Pipe()
	os.Stderr = w

	ctx := context.Background()
	status := cmd.Execute(ctx, fs)

	w.Close()
	os.Stderr = oldStderr

	var buf bytes.Buffer
	buf.ReadFrom(r)
	output := buf.String()

	// It should still exit success because it doesn't fail the command on fix failure
	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess, got %v", status)
	}

	if !strings.Contains(output, "Failed to apply copyright fix") {
		t.Errorf("Expected output to contain error message, got: %s", output)
	}
}
