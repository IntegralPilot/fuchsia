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
	"time"

	"github.com/google/subcommands"
)

func TestReadmeCommand_Format(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)
	testFilePath := filepath.Join(tempDir, "README.fuchsia")

	// Messy unformatted content
	content := []byte(`
Name: test_project
URL: https://test
Version: 1.0
Revision: abc
Security Critical: no
License: MIT
License File: LICENSE
Description:
  A test project
`)
	if err := os.WriteFile(testFilePath, content, 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(tempDir, "LICENSE"), []byte("MIT"), 0644); err != nil {
		t.Fatal(err)
	}

	cmd := &ReadmeCommand{}
	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"-fuchsia_dir", tempDir, "format", testFilePath})

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	status := cmd.Execute(ctx, fs)
	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess (0) for format action, got %v", status)
	}

	formattedBytes, err := os.ReadFile(testFilePath)
	if err != nil {
		t.Fatal(err)
	}
	formatted := string(formattedBytes)

	// It should now be canonically formatted (e.g. no weird leading newlines, fixed indents)
	if !strings.Contains(formatted, "Name: test_project") || !strings.Contains(formatted, "License: MIT") {
		t.Errorf("File was not correctly formatted: %s", formatted)
	}
}

func TestReadmeCommand_Check(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)
	testFilePath := filepath.Join(tempDir, "README.fuchsia")

	// Clean, canonical content
	content := []byte("Name: test_project\nURL: https://test\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: LICENSE\n\nDescription:\n  A test project\n")
	if err := os.WriteFile(testFilePath, content, 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(tempDir, "LICENSE"), []byte("MIT"), 0644); err != nil {
		t.Fatal(err)
	}

	cmd := &ReadmeCommand{}
	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"-fuchsia_dir", tempDir, "check", testFilePath})

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	status := cmd.Execute(ctx, fs)
	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess (0) for checking perfectly formatted file, got %v", status)
	}

	// Now introduce an unknown field to make it fail the check
	badContent := []byte("Name: test_project\nURL: https://test\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: LICENSE\nUnknown Field: foo\nDescription:\n  A test project\n")
	if err := os.WriteFile(testFilePath, badContent, 0644); err != nil {
		t.Fatal(err)
	}

	fs.Parse([]string{"-fuchsia_dir", tempDir, "check", testFilePath})
	status = cmd.Execute(ctx, fs)
	if status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure (1) for checking file with unknown field, got %v", status)
	}

	// Test missing Location for sub-project
	badSubProjectContent := []byte("Name: test_project\nURL: https://test\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: LICENSE\n\n-------------------- DEPENDENCY DIVIDER --------------------\n\nName: sub\nURL: https://sub\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: sub/LICENSE\n")
	if err := os.WriteFile(testFilePath, badSubProjectContent, 0644); err != nil {
		t.Fatal(err)
	}
	os.MkdirAll(filepath.Join(tempDir, "sub"), 0755)
	if err := os.WriteFile(filepath.Join(tempDir, "sub", "LICENSE"), []byte("MIT"), 0644); err != nil {
		t.Fatal(err)
	}
	fs.Parse([]string{"-fuchsia_dir", tempDir, "check", testFilePath})
	status = cmd.Execute(ctx, fs)
	if status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure (1) for checking file with missing Location on sub-project, got %v", status)
	}
	// Test missing License File existence
	missingLicenseContent := []byte("Name: test_project\nURL: https://test\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: NON_EXISTENT_FILE\n")
	if err := os.WriteFile(testFilePath, missingLicenseContent, 0644); err != nil {
		t.Fatal(err)
	}
	fs.Parse([]string{"-fuchsia_dir", tempDir, "check", testFilePath})
	status = cmd.Execute(ctx, fs)
	if status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure (1) for checking file with missing license file, got %v", status)
	}
}

func TestReadmeCommand_Stdout(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)
	testFilePath := filepath.Join(tempDir, "README.fuchsia")

	// Messy content
	messyContent := []byte("Name: test\nURL: https://test\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: LICENSE\n")
	if err := os.WriteFile(testFilePath, messyContent, 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(tempDir, "LICENSE"), []byte("MIT"), 0644); err != nil {
		t.Fatal(err)
	}

	cmd := &ReadmeCommand{}
	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"-fuchsia_dir", tempDir, "format", "-stdout", testFilePath})

	// Capture stdout
	oldStdout := os.Stdout
	r, w, _ := os.Pipe()
	os.Stdout = w

	ctx := context.Background()
	status := cmd.Execute(ctx, fs)

	w.Close()
	os.Stdout = oldStdout

	var buf bytes.Buffer
	buf.ReadFrom(r)
	output := buf.String()

	if status != 0 {
		t.Errorf("Expected ExitSuccess (0), got %v", status)
	}

	if !strings.Contains(output, "License: MIT") {
		t.Errorf("Expected output to be formatted, got: %s", output)
	}

	// Verify file was NOT modified
	currentContent, _ := os.ReadFile(testFilePath)
	if !bytes.Equal(currentContent, messyContent) {
		t.Error("Expected file to remain unmodified in stdout mode")
	}
}

func TestReadmeCommand_AllowlistedProject(t *testing.T) {
	tempDir := t.TempDir()

	// Setup config with policy exception
	configDir := filepath.Join(tempDir, "tools", "check-licenses")
	if err := os.MkdirAll(configDir, 0755); err != nil {
		t.Fatal(err)
	}
	configBytes := []byte(`{
		"policy_exceptions": {
			"AllProjectsMustHaveALicense": [
				{
					"bug": "b/123",
					"paths": ["allowlisted"]
				}
			]
		}
	}`)
	if err := os.WriteFile(filepath.Join(configDir, "config.json"), configBytes, 0644); err != nil {
		t.Fatal(err)
	}

	projDir := filepath.Join(tempDir, "allowlisted")
	if err := os.MkdirAll(projDir, 0755); err != nil {
		t.Fatal(err)
	}
	testFilePath := filepath.Join(projDir, "README.fuchsia")

	// Content without License File and without Version
	content := []byte("Name: allowlisted\nURL: https://test\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\n")
	if err := os.WriteFile(testFilePath, content, 0644); err != nil {
		t.Fatal(err)
	}

	cmd := &ReadmeCommand{}
	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"-fuchsia_dir", tempDir, "check", testFilePath})

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	status := cmd.Execute(ctx, fs)
	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess (0) for allowlisted project without License File, got %v", status)
	}
}
