// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"os"
	"path/filepath"
	"testing"
)

func TestLocator_FirstPartyVirtualReadme(t *testing.T) {
	tempDir := t.TempDir()

	// 1. Create root virtual README
	virtualDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "readmes")
	if err := os.MkdirAll(virtualDir, 0755); err != nil {
		t.Fatal(err)
	}
	virtualReadmePath := filepath.Join(virtualDir, "README.fuchsia")
	virtualContent := `Name: Fuchsia
Security Critical: yes
First Party: yes

License File: LICENSE
  License: BSD-2-Clause, Copyright
`
	if err := os.WriteFile(virtualReadmePath, []byte(virtualContent), 0644); err != nil {
		t.Fatal(err)
	}

	outOfTreeReadmes := map[string]string{
		".": virtualReadmePath,
	}

	// 2. Test 1st-party file under src/lib/foo/bar.cc
	srcFile := filepath.Join(tempDir, "src", "lib", "foo", "bar.cc")
	if err := os.MkdirAll(filepath.Dir(srcFile), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(srcFile, []byte("int main() {}"), 0644); err != nil {
		t.Fatal(err)
	}

	r, bestReadmePath, err := FindProjectReadme(srcFile, tempDir, outOfTreeReadmes)
	if err != nil {
		t.Fatalf("FindProjectReadme failed: %v", err)
	}
	if r == nil {
		t.Fatal("Expected non-nil Readme for 1st-party file")
	}
	if r.Name != "Fuchsia" {
		t.Errorf("Expected Readme name 'Fuchsia', got %q", r.Name)
	}
	if r.FirstParty != "yes" {
		t.Errorf("Expected FirstParty 'yes', got %q", r.FirstParty)
	}
	if bestReadmePath != virtualReadmePath {
		t.Errorf("Expected bestReadmePath %q, got %q", virtualReadmePath, bestReadmePath)
	}

	// 3. Test ResolveProjectRoot returns tempDir (fuchsiaDir)
	root := ResolveProjectRoot(r, bestReadmePath, tempDir, outOfTreeReadmes)
	if root != tempDir {
		t.Errorf("Expected project root %q, got %q", tempDir, root)
	}
}

func TestLocator_SubdirectoryFirstPartyVirtualReadme(t *testing.T) {
	tempDir := t.TempDir()

	// Virtual README for prebuilt/cts/canary/linux-x64
	relProject := filepath.Join("prebuilt", "cts", "canary", "linux-x64")
	virtualDir := filepath.Join(tempDir, "tools", "check-licenses", "assets", "readmes", relProject)
	if err := os.MkdirAll(virtualDir, 0755); err != nil {
		t.Fatal(err)
	}
	virtualReadmePath := filepath.Join(virtualDir, "README.fuchsia")
	virtualContent := `Name: Fuchsia Compatibility Test Suite
First Party: yes

License File: ../../../../LICENSE
`
	if err := os.WriteFile(virtualReadmePath, []byte(virtualContent), 0644); err != nil {
		t.Fatal(err)
	}

	outOfTreeReadmes := map[string]string{
		relProject: virtualReadmePath,
	}

	// File inside the prebuilt CTS directory
	ctsFile := filepath.Join(tempDir, relProject, "cts", "test_package.json")
	if err := os.MkdirAll(filepath.Dir(ctsFile), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(ctsFile, []byte("{}"), 0644); err != nil {
		t.Fatal(err)
	}

	r, bestReadmePath, err := FindProjectReadme(ctsFile, tempDir, outOfTreeReadmes)
	if err != nil {
		t.Fatalf("FindProjectReadme failed: %v", err)
	}
	if r == nil {
		t.Fatal("Expected non-nil Readme for CTS prebuilt")
	}
	if r.Name != "Fuchsia Compatibility Test Suite" {
		t.Errorf("Expected Readme name 'Fuchsia Compatibility Test Suite', got %q", r.Name)
	}
	if r.FirstParty != "yes" {
		t.Errorf("Expected FirstParty 'yes', got %q", r.FirstParty)
	}

	// ResolveProjectRoot must resolve to the subdirectory project root, NOT tempDir (fuchsiaDir)
	expectedRoot := filepath.Join(tempDir, relProject)
	root := ResolveProjectRoot(r, bestReadmePath, tempDir, outOfTreeReadmes)
	if root != expectedRoot {
		t.Errorf("Expected project root %q, got %q", expectedRoot, root)
	}
}
