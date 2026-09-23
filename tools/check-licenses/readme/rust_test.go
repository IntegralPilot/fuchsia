// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"os"
	"path/filepath"
	"testing"
)

func TestParseCargoToml(t *testing.T) {
	tempDir := t.TempDir()
	cargoPath := filepath.Join(tempDir, "Cargo.toml")
	content := `[package]
name = "test-crate"
version = "0.1.0"
repository = "https://github.com/foo/bar"

[dependencies]
anyhow = "1.0"

[workspace]
members = [
    "sub-crate-1",
    "libs/sub-crate-2",
]
`
	if err := os.WriteFile(cargoPath, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	readmes, err := ParseCargoToml(cargoPath)
	if err != nil {
		t.Fatalf("ParseCargoToml failed: %v", err)
	}

	// Expect 1 package + 2 members = 3 readmes
	if len(readmes) != 3 {
		t.Errorf("Expected 3 readmes, got %d", len(readmes))
	}

	// 1. Verify package
	p := readmes[0]
	if p.Name != "test-crate" {
		t.Errorf("Expected name 'test-crate', got %q", p.Name)
	}
	if p.URL != "https://github.com/foo/bar" {
		t.Errorf("Expected URL 'https://github.com/foo/bar', got %q", p.URL)
	}
	if p.Location != "." {
		t.Errorf("Expected location '.', got %q", p.Location)
	}

	// 2. Verify workspace members
	m1 := readmes[1]
	if m1.Name != "sub-crate-1" || m1.Location != "sub-crate-1" {
		t.Errorf("Unexpected member 1: %+v", m1)
	}

	m2 := readmes[2]
	if m2.Name != "sub-crate-2" || m2.Location != "libs/sub-crate-2" {
		t.Errorf("Unexpected member 2: %+v", m2)
	}
}

func TestParseCargoToml_WorkspacePackage(t *testing.T) {
	tempDir := t.TempDir()
	mirrorsDir := filepath.Join(tempDir, "third_party/rust_crates/mirrors")
	if err := os.MkdirAll(filepath.Join(mirrorsDir, "google-cloud-rust"), 0755); err != nil {
		t.Fatal(err)
	}
	cargoPath := filepath.Join(mirrorsDir, "google-cloud-rust", "Cargo.toml")
	content := `# Copyright 2024 Google LLC
[workspace]
resolver = "2"

members = [
  "guide/samples",
  "src/auth",
  "src/generated/cloud/dataform/v1",
]

[workspace.package]
edition      = "2024"
license      = "Apache-2.0"
repository   = "https://github.com/googleapis/google-cloud-rust/tree/main"
`
	if err := os.WriteFile(cargoPath, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	readmes, err := ParseCargoToml(cargoPath)
	if err != nil {
		t.Fatalf("ParseCargoToml failed: %v", err)
	}

	// Expect 1 readme (the workspace root) because we are in mirrors!
	if len(readmes) != 1 {
		t.Errorf("Expected 1 readme, got %d", len(readmes))
	}

	m := readmes[0]
	if m.Name != "google-cloud-rust" || m.Location != "." {
		t.Errorf("Unexpected member: %+v", m)
	}
}
