// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

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

	if len(readme.Licenses) != 2 {
		t.Fatalf("Expected 2 Licenses, got %d", len(readme.Licenses))
	}
	if readme.Licenses[0] != "Apache-2.0" {
		t.Errorf("Expected license 1 'Apache-2.0', got %q", readme.Licenses[0])
	}
	if readme.Licenses[1] != "MIT" {
		t.Errorf("Expected license 2 'MIT', got %q", readme.Licenses[1])
	}
}

func TestParse_MultiLineFields(t *testing.T) {
	readmeText := `
Name: multi_line_project
Description: This is a
very long description
that spans multiple lines.

Note: It has colons!
And it has empty lines!
Local Modifications:
- Added a cool feature
- Fixed a bug
License File: LICENSE
`

	readmes, err := Parse([]byte(readmeText))
	if err != nil {
		t.Fatalf("Failed to parse: %v", err)
	}

	if len(readmes) != 1 {
		t.Fatalf("Expected 1 readme, got %d", len(readmes))
	}
	readme := readmes[0]

	expectedDesc := "This is a\nvery long description\nthat spans multiple lines.\n\nNote: It has colons!\nAnd it has empty lines!"
	if readme.Description != expectedDesc {
		t.Errorf("Expected Description %q, got %q", expectedDesc, readme.Description)
	}

	expectedMods := "\n- Added a cool feature\n- Fixed a bug"
	if readme.LocalModifications != expectedMods {
		t.Errorf("Expected Local Modifications %q, got %q", expectedMods, readme.LocalModifications)
	}
}

func TestParse_EmptyLinesAndComments(t *testing.T) {
	readmeText := `
# This is a comment
Name: foo

Random Unfamiliar Field: some string value
Another Unfamiliar Field: bar

# Another comment
License File: LICENSE
`

	readmes, err := Parse([]byte(readmeText))
	if err != nil {
		t.Fatalf("Failed to parse: %v", err)
	}

	if len(readmes) != 1 {
		t.Fatalf("Expected 1 readme, got %d", len(readmes))
	}
	readme := readmes[0]

	if readme.Name != "foo" {
		t.Errorf("Expected Name 'foo', got %q", readme.Name)
	}
	if len(readme.LicenseFiles) != 1 || readme.LicenseFiles[0] != "LICENSE" {
		t.Errorf("Expected 1 License File 'LICENSE'")
	}
	if len(readme.UnknownFields) != 2 {
		t.Fatalf("Expected 2 UnknownFields, got %d", len(readme.UnknownFields))
	}
	if readme.UnknownFields[0].Key != "Random Unfamiliar Field" || readme.UnknownFields[0].Value != "some string value" {
		t.Errorf("Unexpected UnknownField 0: %+v", readme.UnknownFields[0])
	}
	if readme.UnknownFields[1].Key != "Another Unfamiliar Field" || readme.UnknownFields[1].Value != "bar" {
		t.Errorf("Unexpected UnknownField 1: %+v", readme.UnknownFields[1])
	}
}

func TestParse_DependencyDivider(t *testing.T) {
	readmeText := `
Name: Parent Project
URL: http://parent
License File: LICENSE

-------------------- DEPENDENCY DIVIDER --------------------

Name: Vendored Sub Project
URL: http://subproject
Location: third_party/sub
License File: third_party/sub/LICENSE
`
	readmes, err := Parse([]byte(readmeText))
	if err != nil {
		t.Fatalf("Failed to parse: %v", err)
	}

	if len(readmes) != 2 {
		t.Fatalf("Expected 2 readmes, got %d", len(readmes))
	}

	parent := readmes[0]
	if parent.Name != "Parent Project" {
		t.Errorf("Expected first readme Name 'Parent Project', got %q", parent.Name)
	}
	if len(parent.LicenseFiles) != 1 || parent.LicenseFiles[0] != "LICENSE" {
		t.Errorf("Expected 1 License File 'LICENSE' for parent")
	}

	child := readmes[1]
	if child.Name != "Vendored Sub Project" {
		t.Errorf("Expected second readme Name 'Vendored Sub Project', got %q", child.Name)
	}
	if child.Location != "third_party/sub" {
		t.Errorf("Expected second readme Location 'third_party/sub', got %q", child.Location)
	}
	if len(child.LicenseFiles) != 1 || child.LicenseFiles[0] != "third_party/sub/LICENSE" {
		t.Errorf("Expected 1 License File 'third_party/sub/LICENSE' for child")
	}
}

func TestParse_LicenseOrder(t *testing.T) {
	legacyText := `
Name: legacy
License: MIT
License File: LICENSE
`
	legacyReadmes, err := Parse([]byte(legacyText))
	if err != nil || len(legacyReadmes) != 1 {
		t.Fatalf("Failed to parse legacy: %v", err)
	}
	if len(legacyReadmes[0].LicenseFiles) != 1 || legacyReadmes[0].Licenses[0] != "MIT" {
		t.Errorf("Expected legacy License 'MIT', got %+v", legacyReadmes[0].Licenses)
	}
}
