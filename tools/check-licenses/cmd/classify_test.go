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

func TestClassifyCommand_Execute(t *testing.T) {
	tempDir := t.TempDir()

	// Scaffold the fake assets/patterns directory
	patternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	if err := os.MkdirAll(patternDir, 0755); err != nil {
		t.Fatal(err)
	}
	patternContent := []byte(`Permission is hereby granted, free of charge, to any person obtaining a copy`)
	if err := os.WriteFile(filepath.Join(patternDir, "mit.txt"), patternContent, 0644); err != nil {
		t.Fatal(err)
	}

	testFilePath := filepath.Join(tempDir, "test_license.txt")
	content := []byte("Some random text.\nPermission is hereby granted, free of charge, to any person obtaining a copy\nMore random text.")
	if err := os.WriteFile(testFilePath, content, 0644); err != nil {
		t.Fatal(err)
	}

	cmd := &ClassifyCommand{
		fuchsiaDir: tempDir,
	}

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"-fuchsia_dir", tempDir, testFilePath})

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

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
		t.Errorf("Expected ExitSuccess (0) for classifying file, got %v", status)
	}

	if !strings.Contains(output, "SPDX ID:      MIT") || !strings.Contains(output, "Pattern Name: mit.txt") {
		t.Errorf("Expected output to contain SPDX ID and Pattern Name, got: %s", output)
	}
}

func TestClassifyCommand_NoMatches(t *testing.T) {
	tempDir := t.TempDir()

	patternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	if err := os.MkdirAll(patternDir, 0755); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(filepath.Join(patternDir, "mit.txt"), []byte(`Permission is hereby granted`), 0644)

	testFilePath := filepath.Join(tempDir, "test_clean.txt")
	if err := os.WriteFile(testFilePath, []byte("Just clean text\n"), 0644); err != nil {
		t.Fatal(err)
	}

	cmd := &ClassifyCommand{
		fuchsiaDir: tempDir,
	}

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"-fuchsia_dir", tempDir, testFilePath})

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
		t.Errorf("Expected ExitSuccess (0), got %v", status)
	}

	if !strings.Contains(output, "No license patterns found.") {
		t.Errorf("Expected 'No license patterns found.', got: %s", output)
	}
}

func TestClassifyCommand_WorkspaceEnforcement(t *testing.T) {
	tempDir := t.TempDir()
	outerDir := t.TempDir()

	testFilePath := filepath.Join(outerDir, "external.txt")
	if err := os.WriteFile(testFilePath, []byte("Some text\n"), 0644); err != nil {
		t.Fatal(err)
	}

	cmd := &ClassifyCommand{
		fuchsiaDir: tempDir,
	}

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"-fuchsia_dir", tempDir, testFilePath})

	ctx := context.Background()
	status := cmd.Execute(ctx, fs)

	if status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure (1) for file outside workspace, got %v", status)
	}
}
