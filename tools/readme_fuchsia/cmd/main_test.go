// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/readme_fuchsia"
)

func TestRunValidate_FailureMessage(t *testing.T) {
	tmpDir := t.TempDir()
	readmePath := filepath.Join(tmpDir, "README.fuchsia")

	// Missing required field 'Security Critical'
	content := `Name: test_lib
URL: https://example.com
Revision: 12345
License: MIT
License File: LICENSE
`
	if err := os.WriteFile(readmePath, []byte(content), 0644); err != nil {
		t.Fatalf("failed to write test file: %v", err)
	}
	licensePath := filepath.Join(tmpDir, "LICENSE")
	if err := os.WriteFile(licensePath, []byte("MIT License"), 0644); err != nil {
		t.Fatalf("failed to write license file: %v", err)
	}

	oldStderr := os.Stderr
	r, w, _ := os.Pipe()
	os.Stderr = w

	err := runValidate([]string{readmePath})
	w.Close()
	os.Stderr = oldStderr

	var buf bytes.Buffer
	buf.ReadFrom(r)
	output := buf.String()

	if err == nil {
		t.Fatal("expected validation error, got nil")
	}
	if err.Error() != "" {
		t.Errorf("expected empty error string on validation failure, got: %q", err.Error())
	}

	lines := strings.Split(strings.TrimSpace(output), "\n")
	if len(lines) < 2 {
		t.Fatalf("expected at least 2 lines of stderr output, got %d: %q", len(lines), output)
	}
	if !strings.HasPrefix(lines[0], "validation failed for") {
		t.Errorf("expected first line to start with 'validation failed for', got: %q", lines[0])
	}
	if !strings.Contains(lines[1], "Missing required field 'Security Critical'") {
		t.Errorf("expected second line to contain \"Missing required field 'Security Critical'\", got: %q", lines[1])
	}
	if !strings.Contains(lines[1], "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata#security-critical") {
		t.Errorf("expected second line to contain link 'https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata#security-critical', got: %q", lines[1])
	}
}

func TestRunValidate_Success(t *testing.T) {
	tmpDir := t.TempDir()
	readmePath := filepath.Join(tmpDir, "README.fuchsia")

	content := `Name: test_lib
URL: https://example.com
Revision: 12345
Security Critical: no
License: MIT
License File: LICENSE
`
	if err := os.WriteFile(readmePath, []byte(content), 0644); err != nil {
		t.Fatalf("failed to write test file: %v", err)
	}
	licensePath := filepath.Join(tmpDir, "LICENSE")
	if err := os.WriteFile(licensePath, []byte("MIT License"), 0644); err != nil {
		t.Fatalf("failed to write license file: %v", err)
	}

	if err := runValidate([]string{readmePath}); err != nil {
		t.Fatalf("expected validation success, got: %v", err)
	}
}

func TestRunAddAndSet_CreateFromScratch(t *testing.T) {
	tmpDir := t.TempDir()
	readmePath := filepath.Join(tmpDir, "new_dir", "README.fuchsia")

	if err := runSet([]string{"Name", "main_pkg", readmePath}); err != nil {
		t.Fatalf("runSet failed to create file from scratch: %v", err)
	}

	if err := runAdd([]string{"License File", "LICENSE", readmePath}); err != nil {
		t.Fatalf("runAdd failed: %v", err)
	}
	if err := runAdd([]string{"License File", "NOTICE", readmePath}); err != nil {
		t.Fatalf("runAdd second call failed: %v", err)
	}

	content, err := os.ReadFile(readmePath)
	if err != nil {
		t.Fatalf("failed to read created readme: %v", err)
	}

	strContent := string(content)
	if !strings.Contains(strContent, "Name: main_pkg") {
		t.Errorf("expected content to contain Name: main_pkg, got %q", strContent)
	}
	if !strings.Contains(strContent, "License File: LICENSE") || !strings.Contains(strContent, "License File: NOTICE") {
		t.Errorf("expected content to contain both LICENSE and NOTICE, got %q", strContent)
	}
}

func TestRunSetAndAdd_BlockOption(t *testing.T) {
	tmpDir := t.TempDir()
	readmePath := filepath.Join(tmpDir, "README.fuchsia")

	// Create first block
	if err := runSet([]string{"--block=0", "Name", "first_project", readmePath}); err != nil {
		t.Fatalf("runSet block=0 failed: %v", err)
	}
	if err := runAdd([]string{"--block=0", "License", "MIT", readmePath}); err != nil {
		t.Fatalf("runAdd block=0 failed: %v", err)
	}

	// Create second block by index
	if err := runSet([]string{"--block=1", "Name", "second_project", readmePath}); err != nil {
		t.Fatalf("runSet block=1 failed: %v", err)
	}
	if err := runSet([]string{"--block=1", "Location", "vendor/second", readmePath}); err != nil {
		t.Fatalf("runSet block=1 Location failed: %v", err)
	}

	// Target second block by Name
	if err := runAdd([]string{"--block=second_project", "License File", "SECOND_LICENSE", readmePath}); err != nil {
		t.Fatalf("runAdd block=second_project failed: %v", err)
	}

	content, err := os.ReadFile(readmePath)
	if err != nil {
		t.Fatalf("failed to read multi-block readme: %v", err)
	}

	strContent := string(content)
	if !strings.Contains(strContent, "Name: first_project") || !strings.Contains(strContent, "Name: second_project") {
		t.Errorf("expected both project names in content, got: %q", strContent)
	}
	if !strings.Contains(strContent, "-------------------- DEPENDENCY DIVIDER --------------------") {
		t.Errorf("expected dependency divider in content, got: %q", strContent)
	}
	if !strings.Contains(strContent, "Location: vendor/second") {
		t.Errorf("expected Location in second project, got: %q", strContent)
	}

	// Test runGet on specific block
	oldStdout := os.Stdout
	r, w, _ := os.Pipe()
	os.Stdout = w

	if err := runGet([]string{"--block=1", "Location", readmePath}); err != nil {
		t.Fatalf("runGet block=1 failed: %v", err)
	}
	w.Close()
	os.Stdout = oldStdout

	var buf bytes.Buffer
	buf.ReadFrom(r)
	if strings.TrimSpace(buf.String()) != "vendor/second" {
		t.Errorf("expected runGet to return vendor/second, got: %q", buf.String())
	}
}

func TestResolveReadmePath(t *testing.T) {
	oldFuchsiaDir := os.Getenv("FUCHSIA_DIR")
	defer os.Setenv("FUCHSIA_DIR", oldFuchsiaDir)

	os.Setenv("FUCHSIA_DIR", "/fake/fuchsia/root")

	got := resolveReadmePath("//tools/readme_fuchsia/README.fuchsia")
	want := filepath.Join("/fake/fuchsia/root", "tools/readme_fuchsia/README.fuchsia")
	if got != want {
		t.Errorf("resolveReadmePath(//...) = %q, want %q", got, want)
	}

	relPath := "some/local/README.fuchsia"
	if got := resolveReadmePath(relPath); got != relPath {
		t.Errorf("resolveReadmePath(%q) = %q, want %q", relPath, got, relPath)
	}
}

func TestRunValidate_FindingsFile(t *testing.T) {
	tmpDir := t.TempDir()
	readmePath := filepath.Join(tmpDir, "README.fuchsia")
	findingsPath := filepath.Join(tmpDir, "findings.json")

	content := `Name: test_lib
URL: https://example.com
Revision: 12345
Security Critical: false
License: MIT
License File: LICENSE
`
	if err := os.WriteFile(readmePath, []byte(content), 0644); err != nil {
		t.Fatalf("failed to write test file: %v", err)
	}
	licensePath := filepath.Join(tmpDir, "LICENSE")
	if err := os.WriteFile(licensePath, []byte("MIT License"), 0644); err != nil {
		t.Fatalf("failed to write license file: %v", err)
	}

	oldStderr := os.Stderr
	r, w, _ := os.Pipe()
	os.Stderr = w

	err := runValidate([]string{"--findings_file", findingsPath, readmePath})
	w.Close()
	os.Stderr = oldStderr

	var buf bytes.Buffer
	buf.ReadFrom(r)

	if err == nil {
		t.Fatal("expected validation error, got nil")
	}

	data, err := os.ReadFile(findingsPath)
	if err != nil {
		t.Fatalf("failed to read findings file: %v", err)
	}

	var findings []readme_fuchsia.Finding
	if err := json.Unmarshal(data, &findings); err != nil {
		t.Fatalf("failed to unmarshal findings JSON: %v", err)
	}

	if len(findings) != 1 {
		t.Fatalf("expected 1 finding, got %d: %+v", len(findings), findings)
	}

	f := findings[0]
	if f.Line != 4 || f.EndLine != 4 {
		t.Errorf("expected finding on line 4, got %d-%d", f.Line, f.EndLine)
	}
	if len(f.Replacements) == 0 || f.Replacements[0] != "Security Critical: no" {
		t.Errorf("expected replacement 'Security Critical: no', got %v", f.Replacements)
	}
	if f.Level != "error" {
		t.Errorf("expected level 'error', got %q", f.Level)
	}
}
