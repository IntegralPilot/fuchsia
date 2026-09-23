// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"os"
	"path/filepath"
	"testing"
)

func TestParsePubspecYaml(t *testing.T) {
	tempDir := t.TempDir()
	pubspecPath := filepath.Join(tempDir, "pubspec.yaml")

	content := `name: image
version: 3.3.0
description: some description
homepage: https://github.com/brendan-duncan/image
`
	if err := os.WriteFile(pubspecPath, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	readmes, err := ParsePubspecYaml(pubspecPath)
	if err != nil {
		t.Fatalf("ParsePubspecYaml failed: %v", err)
	}

	if len(readmes) != 1 {
		t.Errorf("Expected 1 readme, got %d", len(readmes))
	}

	r := readmes[0]
	if r.Name != "image" {
		t.Errorf("Expected name 'image', got %q", r.Name)
	}
	if r.Version != "3.3.0" {
		t.Errorf("Expected version '3.3.0', got %q", r.Version)
	}
	if r.URL != "https://github.com/brendan-duncan/image" {
		t.Errorf("Expected URL 'https://github.com/brendan-duncan/image', got %q", r.URL)
	}
}
