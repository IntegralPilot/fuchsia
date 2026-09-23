// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme_fuchsia

import (
	"testing"
)

func TestParse(t *testing.T) {
	readmeText := `
Name: awesome_lib
URL: https://github.com/awesome/lib
Version: 1.2.3
Security Critical: no
License: MIT, Apache-2.0
License File: LICENSE, third_party/NOTICE
Description: An awesome library for doing awesome things.
Local Modifications: None.
`

	readmes, err := Parse([]byte(readmeText))
	if err != nil {
		t.Fatalf("Failed to parse: %v", err)
	}

	if len(readmes) != 1 {
		t.Fatalf("Expected 1 readme, got %d", len(readmes))
	}
	readme := readmes[0]

	if readme.Name != "awesome_lib" {
		t.Errorf("Expected Name 'awesome_lib', got %q", readme.Name)
	}
	if readme.Version != "1.2.3" {
		t.Errorf("Expected Version '1.2.3', got %q", readme.Version)
	}
	if readme.SecurityCritical != "no" {
		t.Errorf("Expected Security Critical 'no', got %q", readme.SecurityCritical)
	}
	if readme.Description != "An awesome library for doing awesome things." {
		t.Errorf("Expected Description 'An awesome library...', got %q", readme.Description)
	}

	if len(readme.LicenseFiles) != 2 {
		t.Fatalf("Expected 2 LicenseFiles, got %d", len(readme.LicenseFiles))
	}
	if readme.LicenseFiles[0] != "LICENSE" {
		t.Errorf("Expected lf1 Path 'LICENSE', got %q", readme.LicenseFiles[0])
	}
	if readme.LicenseFiles[1] != "third_party/NOTICE" {
		t.Errorf("Expected lf2 Path 'third_party/NOTICE', got %q", readme.LicenseFiles[1])
	}
}
