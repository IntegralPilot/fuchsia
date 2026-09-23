// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/report"
)

const mockMITLicenseText = `Permission is hereby granted, free of charge, to any person obtaining a copy`

const mockBSDLicenseText = `Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:
	1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
	2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
	3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.
	THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.`

func scaffoldV2Config(t *testing.T, tempDir string) {
	t.Helper()
	seedConfig := filepath.Join(tempDir, "tools", "check-licenses", "config.json")
	if err := os.MkdirAll(filepath.Dir(seedConfig), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(seedConfig, []byte(`{"includes":["tools/check-licenses/assets","vendor/google/tools/check-licenses/assets"]}`), 0644); err != nil {
		t.Fatal(err)
	}
}

func TestProjectCommand_Check(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	// 1. Scaffold the fake assets/patterns directory so the v2 Classifier can load it
	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	if err := os.MkdirAll(mitPatternDir, 0755); err != nil {
		t.Fatal(err)
	}
	// A simple recognizable MIT text snippet for the classifier
	if err := os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644); err != nil {
		t.Fatal(err)
	}

	bsdPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "BSD")
	if err := os.MkdirAll(bsdPatternDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bsdPatternDir, "bsd.txt"), []byte("Redistribution and use in source and binary forms"), 0644); err != nil {
		t.Fatal(err)
	}

	// Scaffold configs dir to avoid Assemble errors
	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	// 2. Create a third_party project with a README
	projectDir := filepath.Join(tempDir, "third_party", "foo")
	if err := os.MkdirAll(projectDir, 0755); err != nil {
		t.Fatal(err)
	}
	readmeContent := []byte("Name: foo\nURL: http://foo\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: declared.cc\n")
	if err := os.WriteFile(filepath.Join(projectDir, "README.fuchsia"), readmeContent, 0644); err != nil {
		t.Fatal(err)
	}

	// 3. Create a declared file with a license
	declaredFile := filepath.Join(projectDir, "declared.cc")
	if err := os.WriteFile(declaredFile, []byte("/* Permission is hereby granted, free of charge, to any person obtaining a copy */\nint main() {}"), 0644); err != nil {
		t.Fatal(err)
	}

	// 4. Create an undeclared file with a license
	undeclaredFile := filepath.Join(projectDir, "undeclared.cc")
	if err := os.WriteFile(undeclaredFile, []byte("/* Redistribution and use in source and binary forms */\nint helper() {}"), 0644); err != nil {
		t.Fatal(err)
	}

	// Initialize the command
	cmd := &ProjectCommand{
		fuchsiaDir: tempDir,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Test 1: Declared file should pass
	fs1 := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs1)
	fs1.Parse([]string{"--fuchsia_dir", tempDir, "check", declaredFile})
	status := cmd.Execute(ctx, fs1)
	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for declared file, got %v", status)
	}

	// Test 2: Undeclared file should fail
	fs2 := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs2)
	fs2.Parse([]string{"--fuchsia_dir", tempDir, "check", undeclaredFile})
	status = cmd.Execute(ctx, fs2)
	if status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure for undeclared file, got %v", status)
	}

	// Test 3: Fast mode on declared file should pass
	fs3 := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs3)
	fs3.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", declaredFile})
	status = cmd.Execute(ctx, fs3)
	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for declared file in fast mode, got %v", status)
	}

	// Test 4: Fast mode on undeclared file should fail
	fs4 := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs4)
	fs4.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", undeclaredFile})
	status = cmd.Execute(ctx, fs4)
	if status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure for undeclared file in fast mode, got %v", status)
	}

	// Test 5: Fast mode with // workspace-relative path should pass
	fs5 := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs5)
	fs5.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", "//third_party/foo/declared.cc"})
	status = cmd.Execute(ctx, fs5)
	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for // path in fast mode, got %v", status)
	}

	// Test 6: Fast mode with a file in a skipped directory should be ignored and pass
	skippedDir := filepath.Join(tempDir, "build", "bazel_sdk", "tests")
	if err := os.MkdirAll(skippedDir, 0755); err != nil {
		t.Fatal(err)
	}
	skippedFile := filepath.Join(skippedDir, "test.txt")
	if err := os.WriteFile(skippedFile, []byte("/* Redistribution and use in source and binary forms */"), 0644); err != nil {
		t.Fatal(err)
	}
	skipsConfig := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "skips", "default.json")
	if err := os.MkdirAll(filepath.Dir(skipsConfig), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(skipsConfig, []byte(`{"paths":["build/bazel_sdk/tests"]}`), 0644); err != nil {
		t.Fatal(err)
	}

	fs6 := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs6)
	fs6.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", skippedFile})
	status = cmd.Execute(ctx, fs6)
	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for skipped file in fast mode, got %v", status)
	}
}

func TestProjectCommand_Update(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	if err := os.MkdirAll(mitPatternDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644); err != nil {
		t.Fatal(err)
	}

	bsdPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "BSD")
	if err := os.MkdirAll(bsdPatternDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bsdPatternDir, "bsd.txt"), []byte(mockBSDLicenseText), 0644); err != nil {
		t.Fatal(err)
	}

	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	projectDir := filepath.Join(tempDir, "third_party", "foo")
	if err := os.MkdirAll(projectDir, 0755); err != nil {
		t.Fatal(err)
	}

	readmeContent := []byte("Name: foo\nURL: http://foo\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\n\nNon-License File: ignored.cc\n")
	if err := os.WriteFile(filepath.Join(projectDir, "README.fuchsia"), readmeContent, 0644); err != nil {
		t.Fatal(err)
	}

	os.WriteFile(filepath.Join(projectDir, "ignored.cc"), []byte("/* Permission is hereby granted, free of charge, to any person obtaining a copy */\n"), 0644)
	os.WriteFile(filepath.Join(projectDir, "found.cc"), append([]byte("/*\n"), append([]byte(mockBSDLicenseText), []byte("\n*/\n")...)...), 0644)
	os.WriteFile(filepath.Join(projectDir, "LICENSE"), []byte("Permission is hereby granted, free of charge, to any person obtaining a copy\n"), 0644)

	cmd := &ProjectCommand{
		fuchsiaDir: tempDir,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "update", projectDir})
	status := cmd.Execute(ctx, fs)

	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for update, got %v", status)
	}

	updatedReadme, err := os.ReadFile(filepath.Join(projectDir, "README.fuchsia"))
	if err != nil {
		t.Fatal(err)
	}
	content := string(updatedReadme)

	if !strings.Contains(content, "License File: LICENSE") {
		t.Errorf("Expected LICENSE to be added to License File list, got:\n%s", content)
	}
	if !strings.Contains(content, "Non-License File: ignored.cc") {
		t.Errorf("Expected ignored.cc to be preserved in Non-License File list, got:\n%s", content)
	}
}

func TestProjectCommand_Update_Fast(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	if err := os.MkdirAll(mitPatternDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644); err != nil {
		t.Fatal(err)
	}

	bsdPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "BSD")
	if err := os.MkdirAll(bsdPatternDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bsdPatternDir, "bsd.txt"), []byte(mockBSDLicenseText), 0644); err != nil {
		t.Fatal(err)
	}

	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	projectDir := filepath.Join(tempDir, "third_party", "foo")
	if err := os.MkdirAll(projectDir, 0755); err != nil {
		t.Fatal(err)
	}

	// Project has existing README with Generated Notice File and License File
	readmeContent := []byte("Name: foo\nURL: http://foo\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: LICENSE\nGenerated Notice File: NOTICE.fuchsia\n")
	if err := os.WriteFile(filepath.Join(projectDir, "README.fuchsia"), readmeContent, 0644); err != nil {
		t.Fatal(err)
	}

	licensePath := filepath.Join(projectDir, "LICENSE")
	os.WriteFile(licensePath, []byte("Permission is hereby granted, free of charge, to any person obtaining a copy\n"), 0644)

	noticePath := filepath.Join(projectDir, "NOTICE.fuchsia")
	noticeContent := "Pre-existing custom notice content\n"
	os.WriteFile(noticePath, []byte(noticeContent), 0644)

	// Add a new license file
	newLicensePath := filepath.Join(projectDir, "LICENSE.bsd")
	os.WriteFile(newLicensePath, []byte(mockBSDLicenseText), 0644)

	cmd := &ProjectCommand{
		fuchsiaDir: tempDir,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Use // workspace-relative path with --fast
	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "update", "--fast", "//third_party/foo/LICENSE.bsd"})
	status := cmd.Execute(ctx, fs)

	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for fast update, got %v", status)
	}

	// Verify NOTICE.fuchsia was NOT deleted and content was preserved
	readNotice, err := os.ReadFile(noticePath)
	if err != nil {
		t.Fatalf("NOTICE.fuchsia was unexpectedly deleted or unreadable: %v", err)
	}
	if string(readNotice) != noticeContent {
		t.Errorf("Expected NOTICE.fuchsia content %q, got %q", noticeContent, string(readNotice))
	}

	updatedReadme, err := os.ReadFile(filepath.Join(projectDir, "README.fuchsia"))
	if err != nil {
		t.Fatal(err)
	}
	content := string(updatedReadme)

	if !strings.Contains(content, "Generated Notice File: NOTICE.fuchsia") {
		t.Errorf("Expected Generated Notice File to be preserved in fast mode, got:\n%s", content)
	}
	if !strings.Contains(content, "License File: LICENSE") {
		t.Errorf("Expected LICENSE to be preserved in License File list, got:\n%s", content)
	}
	if !strings.Contains(content, "License File: LICENSE.bsd") {
		t.Errorf("Expected LICENSE.bsd to be added to License File list in fast mode, got:\n%s", content)
	}
	if !strings.Contains(content, "BSD-3-Clause") && !strings.Contains(content, "BSD") {
		t.Errorf("Expected BSD license to be added, got:\n%s", content)
	}
}

func TestProjectCommand_Check_MultiProject(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	if err := os.MkdirAll(mitPatternDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644); err != nil {
		t.Fatal(err)
	}

	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	// Create project A
	projADir := filepath.Join(tempDir, "third_party", "proj_a")
	os.MkdirAll(projADir, 0755)
	os.WriteFile(filepath.Join(projADir, "README.fuchsia"), []byte("Name: proj_a\nURL: http://a\nVersion: 1.0\nRevision: 1\nSecurity Critical: no\nLicense: MIT\nLicense File: declared.cc\n"), 0644)
	projAFile := filepath.Join(projADir, "declared.cc")
	os.WriteFile(projAFile, []byte("/* Permission is hereby granted, free of charge, to any person obtaining a copy */\nint a() {}"), 0644)

	// Create project B
	projBDir := filepath.Join(tempDir, "third_party", "proj_b")
	os.MkdirAll(projBDir, 0755)
	os.WriteFile(filepath.Join(projBDir, "README.fuchsia"), []byte("Name: proj_b\nURL: http://b\nVersion: 1.0\nRevision: 2\nSecurity Critical: no\nLicense: MIT\nLicense File: declared.cc\n"), 0644)
	projBFile := filepath.Join(projBDir, "declared.cc")
	os.WriteFile(projBFile, []byte("/* Permission is hereby granted, free of charge, to any person obtaining a copy */\nint b() {}"), 0644)

	cmd := &ProjectCommand{
		fuchsiaDir: tempDir,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", projAFile, projBFile})
	status := cmd.Execute(ctx, fs)
	if status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for multi-project fast check, got %v", status)
	}
}

func TestProjectCommand_ListAndInfo(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	os.MkdirAll(mitPatternDir, 0755)
	os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644)

	projectDir := filepath.Join(tempDir, "third_party", "foo")
	if err := os.MkdirAll(projectDir, 0755); err != nil {
		t.Fatal(err)
	}
	readmeContent := []byte("Name: foo\nURL: http://foo\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: declared.cc\n")
	if err := os.WriteFile(filepath.Join(projectDir, "README.fuchsia"), readmeContent, 0644); err != nil {
		t.Fatal(err)
	}

	cmd := &ProjectCommand{
		fuchsiaDir: tempDir,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Test list
	fsList := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fsList)
	fsList.Parse([]string{"--fuchsia_dir", tempDir, "list", tempDir})
	if status := cmd.Execute(ctx, fsList); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for list, got %v", status)
	}

	// Test info
	fsInfo := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fsInfo)
	fsInfo.Parse([]string{"--fuchsia_dir", tempDir, "info", filepath.Join(projectDir, "declared.cc")})
	if status := cmd.Execute(ctx, fsInfo); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for info, got %v", status)
	}
}

func TestBelongsToProject(t *testing.T) {
	tempDir := t.TempDir()
	fuchsiaDir := tempDir

	// Setup a fake project structure
	// third_party/foo
	// third_party/foo/vendor/bar

	fooDir := filepath.Join(tempDir, "third_party", "foo")
	if err := os.MkdirAll(fooDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(fooDir, "README.fuchsia"), []byte("Name: foo\n"), 0644); err != nil {
		t.Fatal(err)
	}

	barDir := filepath.Join(fooDir, "vendor", "bar")
	if err := os.MkdirAll(barDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(barDir, "README.fuchsia"), []byte("Name: bar\n"), 0644); err != nil {
		t.Fatal(err)
	}

	grouper := boundary.NewGrouper(fuchsiaDir, boundary.Config{})

	tests := []struct {
		name        string
		policyPath  string
		projectRoot string
		expected    bool
	}{
		{
			name:        "Exact match",
			policyPath:  "third_party/foo",
			projectRoot: "third_party/foo",
			expected:    true,
		},
		{
			name:        "Policy on parent directory is not inherited by sub-project",
			policyPath:  "third_party",
			projectRoot: "third_party/foo",
			expected:    false,
		},
		{
			name:        "Policy on unrelated directory is false",
			policyPath:  "src/other",
			projectRoot: "third_party/foo",
			expected:    false,
		},
		{
			name:        "Policy on sub-project should NOT apply to parent project",
			policyPath:  "third_party/foo/vendor/bar",
			projectRoot: "third_party/foo",
			expected:    false,
		},
		{
			name:        "Policy on sub-project should apply to sub-project itself",
			policyPath:  "third_party/foo/vendor/bar",
			projectRoot: "third_party/foo/vendor/bar",
			expected:    true,
		},
		{
			name:        "Policy on regular sub-directory without boundary SHOULD apply to parent",
			policyPath:  "third_party/foo/src",
			projectRoot: "third_party/foo",
			expected:    true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cleanP := strings.TrimPrefix(tt.policyPath, "//")
			result := grouper.BelongsToProject(cleanP, tt.projectRoot)
			if result != tt.expected {
				t.Errorf("BelongsToProject(%q, %q) = %v, want %v", cleanP, tt.projectRoot, result, tt.expected)
			}
		})
	}
}

func TestProjectCommand_Update_UnclassifiedLicense(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	projectDir := filepath.Join(tempDir, "third_party", "bar")
	if err := os.MkdirAll(projectDir, 0755); err != nil {
		t.Fatal(err)
	}

	os.WriteFile(filepath.Join(projectDir, "LICENSE"), []byte("Some completely unclassified proprietary text\n"), 0644)

	readmeContent := []byte("Name: bar\nURL: http://bar\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\n\nLicense File: LICENSE\n")
	if err := os.WriteFile(filepath.Join(projectDir, "README.fuchsia"), readmeContent, 0644); err != nil {
		t.Fatal(err)
	}

	cmd := &ProjectCommand{
		fuchsiaDir: tempDir,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "update", projectDir})
	if status := cmd.Execute(ctx, fs); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for update, got %v", status)
	}

	updatedReadme, err := os.ReadFile(filepath.Join(projectDir, "README.fuchsia"))
	if err != nil {
		t.Fatal(err)
	}
	content := string(updatedReadme)

	if !strings.Contains(content, "License File: LICENSE") || !strings.Contains(content, "License: Unclassified") {
		t.Errorf("Expected LICENSE to be retained with Unclassified license, got:\n%s", content)
	}
}

func TestProjectCommand_Update_FileListAndMultiTarget(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	os.MkdirAll(mitPatternDir, 0755)
	os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644)
	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	projA := filepath.Join(tempDir, "third_party", "projA")
	projB := filepath.Join(tempDir, "third_party", "projB")
	os.MkdirAll(projA, 0755)
	os.MkdirAll(projB, 0755)

	os.WriteFile(filepath.Join(projA, "LICENSE"), []byte(mockMITLicenseText), 0644)
	os.WriteFile(filepath.Join(projA, "README.fuchsia"), []byte("Name: projA\nURL: http://a\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\n"), 0644)

	os.WriteFile(filepath.Join(projB, "LICENSE"), []byte(mockMITLicenseText), 0644)
	os.WriteFile(filepath.Join(projB, "README.fuchsia"), []byte("Name: projB\nURL: http://b\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\n"), 0644)

	fileListPath := filepath.Join(tempDir, "update_list.txt")
	os.WriteFile(fileListPath, []byte("third_party/projA\n"), 0644)

	cmd := &ProjectCommand{fuchsiaDir: tempDir}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "update", "-file-list", fileListPath, projB})
	if status := cmd.Execute(ctx, fs); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for multi-target update, got %v", status)
	}

	for _, p := range []string{projA, projB} {
		content, err := os.ReadFile(filepath.Join(p, "README.fuchsia"))
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(string(content), "License File: LICENSE") {
			t.Errorf("Expected LICENSE to be added to %s README, got:\n%s", p, string(content))
		}
	}
}

func TestProjectCommand_Check_FileListAndMultiTarget(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	os.MkdirAll(mitPatternDir, 0755)
	os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644)
	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	projA := filepath.Join(tempDir, "third_party", "projA")
	projB := filepath.Join(tempDir, "third_party", "projB")
	os.MkdirAll(projA, 0755)
	os.MkdirAll(projB, 0755)

	fileA := filepath.Join(projA, "declared.cc")
	fileB := filepath.Join(projB, "declared.cc")
	os.WriteFile(fileA, []byte("/* "+mockMITLicenseText+" */\nint a() {}"), 0644)
	os.WriteFile(fileB, []byte("/* "+mockMITLicenseText+" */\nint b() {}"), 0644)

	os.WriteFile(filepath.Join(projA, "README.fuchsia"), []byte("Name: projA\nURL: http://a\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: declared.cc\n"), 0644)
	os.WriteFile(filepath.Join(projB, "README.fuchsia"), []byte("Name: projB\nURL: http://b\nVersion: 1.0\nRevision: abc\nSecurity Critical: no\nLicense: MIT\nLicense File: declared.cc\n"), 0644)

	fileListPath := filepath.Join(tempDir, "check_list.txt")
	os.WriteFile(fileListPath, []byte(fileA+"\n"), 0644)

	cmd := &ProjectCommand{fuchsiaDir: tempDir}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "check", "-file-list", fileListPath, fileB})
	if status := cmd.Execute(ctx, fs); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for multi-target check, got %v", status)
	}
}

func TestProjectCommand_Check_FirstParty(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	// 1. Setup BSD and MIT patterns
	bsdPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "BSD")
	os.MkdirAll(bsdPatternDir, 0755)
	os.WriteFile(filepath.Join(bsdPatternDir, "bsd.txt"), []byte("Redistribution and use in source and binary forms"), 0644)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	os.MkdirAll(mitPatternDir, 0755)
	os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644)

	// 2. Setup configs (including copyright_extensions)
	copyrightExtDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "copyright_extensions")
	os.MkdirAll(copyrightExtDir, 0755)
	os.WriteFile(filepath.Join(copyrightExtDir, "default.json"), []byte(`{"copyright_extensions": {"extensions": [".cc", ".h"]}}`), 0644)

	// 3. Setup root virtual README with First Party: yes
	virtualDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "readmes")
	os.MkdirAll(virtualDir, 0755)
	virtualContent := `Name: Fuchsia
Security Critical: yes
First Party: yes

License File: LICENSE
  License: BSD-2-Clause, Copyright
`
	os.WriteFile(filepath.Join(virtualDir, "README.fuchsia"), []byte(virtualContent), 0644)

	// Root LICENSE
	os.WriteFile(filepath.Join(tempDir, "LICENSE"), []byte(mockBSDLicenseText), 0644)

	// 4. Create 1st-party files
	srcDir := filepath.Join(tempDir, "src", "lib", "foo")
	os.MkdirAll(srcDir, 0755)

	validHeader := `// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
int valid() { return 1; }
`
	validFile := filepath.Join(srcDir, "valid.cc")
	os.WriteFile(validFile, []byte(validHeader), 0644)

	missingCopyrightFile := filepath.Join(srcDir, "missing_copyright.cc")
	os.WriteFile(missingCopyrightFile, []byte("int missing() { return 0; }"), 0644)

	undeclaredLicenseContent := `// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
/* ` + mockMITLicenseText + ` */
int undeclared() { return 2; }
`
	undeclaredLicenseFile := filepath.Join(srcDir, "undeclared_license.cc")
	os.WriteFile(undeclaredLicenseFile, []byte(undeclaredLicenseContent), 0644)

	cmd := &ProjectCommand{fuchsiaDir: tempDir}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Case 1: Valid 1st-party file passes in normal mode
	fs1 := flag.NewFlagSet("test1", flag.ContinueOnError)
	cmd.SetFlags(fs1)
	fs1.Parse([]string{"--fuchsia_dir", tempDir, "check", validFile})
	if status := cmd.Execute(ctx, fs1); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for valid 1st-party file, got %v", status)
	}

	// Case 2: Valid 1st-party file passes in --fast mode
	fs2 := flag.NewFlagSet("test2", flag.ContinueOnError)
	cmd.SetFlags(fs2)
	fs2.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", validFile})
	if status := cmd.Execute(ctx, fs2); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for valid 1st-party file with --fast, got %v", status)
	}

	// Case 3: Missing copyright header fails
	fs3 := flag.NewFlagSet("test3", flag.ContinueOnError)
	cmd.SetFlags(fs3)
	fs3.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", missingCopyrightFile})
	if status := cmd.Execute(ctx, fs3); status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure for missing copyright header, got %v", status)
	}

	// Case 4: Undeclared 3rd-party license in 1st-party file fails
	fs4 := flag.NewFlagSet("test4", flag.ContinueOnError)
	cmd.SetFlags(fs4)
	fs4.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", undeclaredLicenseFile})
	if status := cmd.Execute(ctx, fs4); status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure for undeclared license in 1st-party file, got %v", status)
	}
}

func TestProjectCommand_Update_FirstParty(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	virtualDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "readmes")
	os.MkdirAll(virtualDir, 0755)
	virtualContent := `Name: Fuchsia
Security Critical: yes
First Party: yes

License File: LICENSE
  License: BSD-2-Clause, Copyright
`
	virtualReadmePath := filepath.Join(virtualDir, "README.fuchsia")
	os.WriteFile(virtualReadmePath, []byte(virtualContent), 0644)
	os.WriteFile(filepath.Join(tempDir, "LICENSE"), []byte(mockBSDLicenseText), 0644)

	srcDir := filepath.Join(tempDir, "src", "lib", "foo")
	os.MkdirAll(srcDir, 0755)
	validHeader := `// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
int valid() { return 1; }
`
	validFile := filepath.Join(srcDir, "valid.cc")
	os.WriteFile(validFile, []byte(validHeader), 0644)

	cmd := &ProjectCommand{fuchsiaDir: tempDir}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Run project update on 1st party file
	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "update", validFile})
	if status := cmd.Execute(ctx, fs); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for 1st-party update (skipped), got %v", status)
	}

	// Verify no in-tree README.fuchsia was created in src/lib/foo or tempDir
	if _, err := os.Stat(filepath.Join(srcDir, "README.fuchsia")); !os.IsNotExist(err) {
		t.Errorf("Expected no README.fuchsia in srcDir, but found one")
	}
	if _, err := os.Stat(filepath.Join(tempDir, "README.fuchsia")); !os.IsNotExist(err) {
		t.Errorf("Expected no README.fuchsia in tempDir, but found one")
	}

	// Verify virtual README was not modified
	readContent, err := os.ReadFile(virtualReadmePath)
	if err != nil {
		t.Fatal(err)
	}
	if string(readContent) != virtualContent {
		t.Errorf("Expected virtual README to remain unchanged, got:\n%s", string(readContent))
	}
}

func TestProjectCommand_Check_StructuredFindings(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	os.MkdirAll(mitPatternDir, 0755)
	os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644)

	bsdPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "BSD")
	os.MkdirAll(bsdPatternDir, 0755)
	os.WriteFile(filepath.Join(bsdPatternDir, "bsd.txt"), []byte("Redistribution and use in source and binary forms"), 0644)

	copyrightExtDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "copyright_extensions")
	os.MkdirAll(copyrightExtDir, 0755)
	os.WriteFile(filepath.Join(copyrightExtDir, "default.json"), []byte(`{"copyright_extensions": {"extensions": [".cc", ".h"]}}`), 0644)

	barrierDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs", "barriers")
	os.MkdirAll(barrierDir, 0755)
	os.WriteFile(filepath.Join(barrierDir, "default.json"), []byte(`{"barriers": [{"paths": ["third_party", "vendor", "prebuilt"]}]}`), 0644)

	virtualDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "readmes")
	os.MkdirAll(virtualDir, 0755)
	virtualContent := `Name: Fuchsia
Security Critical: yes
First Party: yes

License File: LICENSE
  License: BSD-2-Clause, Copyright
`
	os.WriteFile(filepath.Join(virtualDir, "README.fuchsia"), []byte(virtualContent), 0644)
	os.WriteFile(filepath.Join(tempDir, "LICENSE"), []byte(mockBSDLicenseText), 0644)

	srcDir := filepath.Join(tempDir, "src", "lib", "foo")
	os.MkdirAll(srcDir, 0755)

	missingCopyrightFile := filepath.Join(srcDir, "missing.cc")
	os.WriteFile(missingCopyrightFile, []byte("int missing() { return 0; }\n"), 0644)

	validHeader := `// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
int valid() { return 1; }
`
	validFile := filepath.Join(srcDir, "valid.cc")
	os.WriteFile(validFile, []byte(validHeader), 0644)

	// Third party project with outdated README
	tpDir := filepath.Join(tempDir, "third_party", "tp")
	os.MkdirAll(tpDir, 0755)
	tpReadme := `Name: tp
URL: https://example.com
Version: 1.0
Security Critical: no
License: MIT
License File: declared.cc
`
	os.WriteFile(filepath.Join(tpDir, "README.fuchsia"), []byte(tpReadme), 0644)
	os.WriteFile(filepath.Join(tpDir, "declared.cc"), []byte("/* Permission is hereby granted, free of charge, to any person obtaining a copy */\nint d() {}\n"), 0644)
	os.WriteFile(filepath.Join(tpDir, "undeclared.cc"), []byte("/* Redistribution and use in source and binary forms */\nint u() {}\n"), 0644)

	cmd := &ProjectCommand{fuchsiaDir: tempDir}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Case 1: Missing copyright in 1st party file produces structured finding with replacement
	findings1File := filepath.Join(tempDir, "findings1.json")
	fs1 := flag.NewFlagSet("test1", flag.ContinueOnError)
	cmd.SetFlags(fs1)
	fs1.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", "--format=json", "-findings_file", findings1File, missingCopyrightFile})
	if status := cmd.Execute(ctx, fs1); status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure for missing copyright, got %v", status)
	}

	data1, err := os.ReadFile(findings1File)
	if err != nil {
		t.Fatalf("Failed to read findings file: %v", err)
	}
	var findings1 []report.Finding
	if err := json.Unmarshal(data1, &findings1); err != nil {
		t.Fatalf("Failed to parse JSON: %v", err)
	}
	if len(findings1) != 1 {
		t.Fatalf("Expected 1 finding, got %d", len(findings1))
	}
	if findings1[0].CheckName != "AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders" {
		t.Errorf("Expected CheckName AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders, got %s", findings1[0].CheckName)
	}
	if findings1[0].FilePath != "src/lib/foo/missing.cc" {
		t.Errorf("Expected FilePath src/lib/foo/missing.cc, got %s", findings1[0].FilePath)
	}
	if len(findings1[0].Replacements) != 1 {
		t.Fatalf("Expected 1 replacement, got %d", len(findings1[0].Replacements))
	}
	if !strings.Contains(findings1[0].Replacements[0], "Copyright 2026 The Fuchsia Authors") {
		t.Errorf("Expected copyright header in replacement, got:\n%s", findings1[0].Replacements[0])
	}
	if !strings.Contains(findings1[0].Replacements[0], "int missing() { return 0; }") {
		t.Errorf("Expected original code in replacement, got:\n%s", findings1[0].Replacements[0])
	}

	// Case 2: Valid 1st party file produces exit 0 and empty JSON array
	findings2File := filepath.Join(tempDir, "findings2.json")
	fs2 := flag.NewFlagSet("test2", flag.ContinueOnError)
	cmd.SetFlags(fs2)
	fs2.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", "--format=json", "-findings_file", findings2File, validFile})
	if status := cmd.Execute(ctx, fs2); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess for valid file, got %v", status)
	}
	data2, err := os.ReadFile(findings2File)
	if err != nil {
		t.Fatalf("Failed to read findings file: %v", err)
	}
	var findings2 []report.Finding
	if err := json.Unmarshal(data2, &findings2); err != nil {
		t.Fatalf("Failed to parse JSON: %v", err)
	}
	if len(findings2) != 0 {
		t.Errorf("Expected 0 findings for valid file, got %d", len(findings2))
	}

	// Case 3: Out of date 3rd-party project README produces ReadmeDeclarationOutOfDate with updated README replacement
	findings3File := filepath.Join(tempDir, "findings3.json")
	fs3 := flag.NewFlagSet("test3", flag.ContinueOnError)
	cmd.SetFlags(fs3)
	fs3.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", "--format=json", "-findings_file", findings3File, tpDir})
	if status := cmd.Execute(ctx, fs3); status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure for outdated README, got %v", status)
	}
	data3, err := os.ReadFile(findings3File)
	if err != nil {
		t.Fatalf("Failed to read findings file: %v", err)
	}
	var findings3 []report.Finding
	if err := json.Unmarshal(data3, &findings3); err != nil {
		t.Fatalf("Failed to parse JSON: %v", err)
	}
	if len(findings3) == 0 {
		t.Fatalf("Expected findings for outdated README, got 0")
	}
	var foundReadmeOutOfDate bool
	for _, f := range findings3 {
		if f.CheckName == "ReadmeDeclarationOutOfDate" {
			foundReadmeOutOfDate = true
			if f.FilePath != "third_party/tp/README.fuchsia" {
				t.Errorf("Expected FilePath third_party/tp/README.fuchsia, got %s", f.FilePath)
			}
			if !strings.Contains(f.Replacements[0], "License: BSD, MIT") || !strings.Contains(f.Replacements[0], "Generated Notice File: NOTICE.fuchsia") {
				t.Errorf("Expected updated README content in replacement, got:\n%s", f.Replacements[0])
			}
		}
	}
	if !foundReadmeOutOfDate {
		t.Errorf("Expected ReadmeDeclarationOutOfDate finding in findings3: %+v", findings3)
	}

	var foundNoticeOutOfDate bool
	for _, f := range findings3 {
		if f.CheckName == "NoticeFileOutOfDate" {
			foundNoticeOutOfDate = true
			if f.FilePath != "third_party/tp/NOTICE.fuchsia" {
				t.Errorf("Expected FilePath third_party/tp/NOTICE.fuchsia, got %s", f.FilePath)
			}
			if len(f.Replacements) != 1 || !strings.Contains(f.Replacements[0], "undeclared.cc") {
				t.Errorf("Expected notice replacement covering undeclared.cc, got: %v", f.Replacements)
			}
		}
	}
	if !foundNoticeOutOfDate {
		t.Errorf("Expected NoticeFileOutOfDate finding in findings3: %+v", findings3)
	}

	// Case 4: Target a single relative file path via -file-list in 3rd party project produces ReadmeDeclarationOutOfDate and NoticeFileOutOfDate
	fileListPath := filepath.Join(tempDir, "file_list.txt")
	os.WriteFile(fileListPath, []byte("third_party/tp/undeclared.cc\n"), 0644)
	findings4File := filepath.Join(tempDir, "findings4.json")
	fs4 := flag.NewFlagSet("test4", flag.ContinueOnError)
	cmd.SetFlags(fs4)
	fs4.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", "--format=json", "-findings_file", findings4File, "-file-list", fileListPath})
	if status := cmd.Execute(ctx, fs4); status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure for single file target in outdated 3rd-party project, got %v", status)
	}
	data4, err := os.ReadFile(findings4File)
	if err != nil {
		t.Fatalf("Failed to read findings file: %v", err)
	}
	var findings4 []report.Finding
	if err := json.Unmarshal(data4, &findings4); err != nil {
		t.Fatalf("Failed to parse JSON: %v", err)
	}
	if len(findings4) == 0 {
		t.Fatalf("Expected findings for single file target, got 0")
	}
	var foundReadmeInCase4, foundNoticeInCase4 bool
	for _, f := range findings4 {
		if f.CheckName == "ReadmeDeclarationOutOfDate" {
			foundReadmeInCase4 = true
		}
		if f.CheckName == "NoticeFileOutOfDate" {
			foundNoticeInCase4 = true
		}
	}
	if !foundReadmeInCase4 {
		t.Errorf("Expected ReadmeDeclarationOutOfDate in findings4: %+v", findings4)
	}
	if !foundNoticeInCase4 {
		t.Errorf("Expected NoticeFileOutOfDate in findings4: %+v", findings4)
	}

	// Case 5: Target a file in a new third-party project without a README.fuchsia produces AllProjectsMustHaveAReadme
	newThirdPartyDir := filepath.Join(tempDir, "third_party", "new_foo", "nested")
	os.MkdirAll(newThirdPartyDir, 0755)
	newThirdPartyFile := filepath.Join(newThirdPartyDir, "bar.cc")
	os.WriteFile(newThirdPartyFile, []byte("int bar() {}\n"), 0644)

	findings5File := filepath.Join(tempDir, "findings5.json")
	fs5 := flag.NewFlagSet("test5", flag.ContinueOnError)
	cmd.SetFlags(fs5)
	fs5.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", "--format=json", "-findings_file", findings5File, newThirdPartyFile})
	if status := cmd.Execute(ctx, fs5); status != subcommands.ExitFailure {
		t.Errorf("Expected ExitFailure for file in new 3rd-party project missing README, got %v", status)
	}
	data5, err := os.ReadFile(findings5File)
	if err != nil {
		t.Fatalf("Failed to read findings file: %v", err)
	}
	var findings5 []report.Finding
	if err := json.Unmarshal(data5, &findings5); err != nil {
		t.Fatalf("Failed to parse JSON: %v", err)
	}
	var foundReadmeError, foundLicenseError bool
	for _, f := range findings5 {
		if f.CheckName == "AllProjectsMustHaveAReadme" {
			foundReadmeError = true
			if f.FilePath != "third_party/new_foo/nested/bar.cc" {
				t.Errorf("Expected FilePath third_party/new_foo/nested/bar.cc, got %s", f.FilePath)
			}
			if !strings.Contains(f.Message, "Project: third_party/new_foo") {
				t.Errorf("Expected finding message to identify Project: third_party/new_foo, got:\n%s", f.Message)
			}
		}
		if f.CheckName == "AllProjectsMustHaveALicense" {
			foundLicenseError = true
			if f.FilePath != "third_party/new_foo/nested/bar.cc" {
				t.Errorf("Expected FilePath third_party/new_foo/nested/bar.cc, got %s", f.FilePath)
			}
			if !strings.Contains(f.Message, "Project: third_party/new_foo") {
				t.Errorf("Expected finding message to identify Project: third_party/new_foo, got:\n%s", f.Message)
			}
		}
	}
	if !foundReadmeError {
		t.Errorf("Expected AllProjectsMustHaveAReadme in findings5: %+v", findings5)
	}
	if !foundLicenseError {
		t.Errorf("Expected AllProjectsMustHaveALicense in findings5: %+v", findings5)
	}
}

func TestProjectCommand_Check_PartialPassOutput(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	os.MkdirAll(mitPatternDir, 0755)
	os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644)

	bsdPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "BSD")
	os.MkdirAll(bsdPatternDir, 0755)
	os.WriteFile(filepath.Join(bsdPatternDir, "bsd.txt"), []byte("Redistribution and use in source and binary forms"), 0644)

	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	// Project 1: Compliant project
	passProjDir := filepath.Join(tempDir, "third_party", "pass_proj")
	os.MkdirAll(passProjDir, 0755)
	os.WriteFile(filepath.Join(passProjDir, "README.fuchsia"), []byte("Name: pass_proj\nURL: http://pass\nVersion: 1.0\nSecurity Critical: no\nLicense: MIT\nLicense File: LICENSE\n"), 0644)
	os.WriteFile(filepath.Join(passProjDir, "LICENSE"), []byte(mockMITLicenseText), 0644)
	passFile := filepath.Join(passProjDir, "valid.cc")
	os.WriteFile(passFile, []byte("int valid() { return 1; }"), 0644)

	// Project 2: Failing project (undeclared BSD license in source file)
	failProjDir := filepath.Join(tempDir, "third_party", "fail_proj")
	os.MkdirAll(failProjDir, 0755)
	os.WriteFile(filepath.Join(failProjDir, "README.fuchsia"), []byte("Name: fail_proj\nURL: http://fail\nVersion: 1.0\nSecurity Critical: no\nLicense: MIT\nLicense File: LICENSE\n"), 0644)
	os.WriteFile(filepath.Join(failProjDir, "LICENSE"), []byte(mockMITLicenseText), 0644)
	failFile := filepath.Join(failProjDir, "undeclared.cc")
	os.WriteFile(failFile, []byte("/* Redistribution and use in source and binary forms */\nint helper() {}"), 0644)

	cmd := &ProjectCommand{fuchsiaDir: tempDir}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	fs := flag.NewFlagSet("test_partial", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "check", passFile, failFile})

	oldStdout := os.Stdout
	r, w, _ := os.Pipe()
	os.Stdout = w

	status := cmd.Execute(ctx, fs)

	w.Close()
	os.Stdout = oldStdout

	var buf bytes.Buffer
	buf.ReadFrom(r)
	output := buf.String()

	if status != subcommands.ExitFailure {
		t.Fatalf("Expected ExitFailure due to undeclared.cc, got %v", status)
	}
	if !strings.Contains(output, "Passed: pass_proj") {
		t.Fatalf("Expected pass confirmation for pass_proj in output, got:\n%s", output)
	}
	if strings.Contains(output, "Passed: fail_proj") {
		t.Fatalf("Did not expect pass confirmation for fail_proj in output, got:\n%s", output)
	}
}

func TestProjectCommand_Check_FailingProjectDirectoryNoPassOutput(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	os.MkdirAll(mitPatternDir, 0755)
	os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644)

	bsdPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "BSD")
	os.MkdirAll(bsdPatternDir, 0755)
	os.WriteFile(filepath.Join(bsdPatternDir, "bsd.txt"), []byte("Redistribution and use in source and binary forms"), 0644)

	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	// Failing project (undeclared BSD license in source file)
	failProjDir := filepath.Join(tempDir, "third_party", "fail_proj")
	os.MkdirAll(failProjDir, 0755)
	os.WriteFile(filepath.Join(failProjDir, "README.fuchsia"), []byte("Name: fail_proj\nURL: http://fail\nVersion: 1.0\nSecurity Critical: no\nLicense: MIT\nLicense File: LICENSE\n"), 0644)
	os.WriteFile(filepath.Join(failProjDir, "LICENSE"), []byte(mockMITLicenseText), 0644)
	os.WriteFile(filepath.Join(failProjDir, "undeclared.cc"), []byte("/* Redistribution and use in source and binary forms */\nint helper() {}"), 0644)

	cmd := &ProjectCommand{fuchsiaDir: tempDir}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	fs := flag.NewFlagSet("test_failing_dir", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "check", failProjDir})

	oldStdout := os.Stdout
	r, w, _ := os.Pipe()
	os.Stdout = w

	status := cmd.Execute(ctx, fs)

	w.Close()
	os.Stdout = oldStdout

	var buf bytes.Buffer
	buf.ReadFrom(r)
	output := buf.String()

	if status != subcommands.ExitFailure {
		t.Fatalf("Expected ExitFailure, got %v", status)
	}
	if strings.Contains(output, "Passed:") {
		t.Fatalf("Expected no pass confirmation for failing project directory, got:\n%s", output)
	}
}

func TestProjectCommand_Check_JSONFormat_Stdout(t *testing.T) {
	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	mitPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "MIT")
	os.MkdirAll(mitPatternDir, 0755)
	os.WriteFile(filepath.Join(mitPatternDir, "mit.txt"), []byte(mockMITLicenseText), 0644)

	bsdPatternDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "patterns", "Permissive", "BSD")
	os.MkdirAll(bsdPatternDir, 0755)
	os.WriteFile(filepath.Join(bsdPatternDir, "bsd.txt"), []byte("Redistribution and use in source and binary forms"), 0644)

	os.MkdirAll(filepath.Join(tempDir, "tools", "check-licenses", "assets", "configs"), 0755)

	tpDir := filepath.Join(tempDir, "third_party", "tp")
	os.MkdirAll(tpDir, 0755)
	os.WriteFile(filepath.Join(tpDir, "README.fuchsia"), []byte("Name: tp\nURL: http://tp\nVersion: 1.0\nSecurity Critical: no\nLicense: MIT\nLicense File: LICENSE\n"), 0644)
	os.WriteFile(filepath.Join(tpDir, "LICENSE"), []byte(mockMITLicenseText), 0644)
	os.WriteFile(filepath.Join(tpDir, "undeclared.cc"), []byte("/* Redistribution and use in source and binary forms */\nint helper() {}"), 0644)

	cmd := &ProjectCommand{fuchsiaDir: tempDir}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	fs := flag.NewFlagSet("test_json_stdout", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"--fuchsia_dir", tempDir, "check", "--fast", "--format=json", filepath.Join(tpDir, "undeclared.cc")})

	oldStdout := os.Stdout
	r, w, _ := os.Pipe()
	os.Stdout = w

	status := cmd.Execute(ctx, fs)

	w.Close()
	os.Stdout = oldStdout

	var buf bytes.Buffer
	buf.ReadFrom(r)
	output := buf.String()

	if status != subcommands.ExitFailure {
		t.Fatalf("Expected ExitFailure, got %v", status)
	}

	var findings []report.Finding
	if err := json.Unmarshal([]byte(output), &findings); err != nil {
		t.Fatalf("Expected valid JSON findings on stdout, got error: %v\nOutput was:\n%s", err, output)
	}
	if len(findings) == 0 {
		t.Fatalf("Expected non-empty findings on stdout, got 0")
	}
}
