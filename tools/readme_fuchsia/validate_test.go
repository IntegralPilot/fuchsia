// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme_fuchsia

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestValidate_ErrorLinks(t *testing.T) {
	tests := []struct {
		name         string
		readme       Readme
		expectedLink string
	}{
		{
			name: "missing name",
			readme: Readme{
				URL:              "https://example.com",
				Revision:         "1234",
				SecurityCritical: "no",
				Licenses:         []string{"MIT"},
				LicenseFiles:     []string{"LICENSE"},
			},
			expectedLink: "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata#name",
		},
		{
			name: "missing url and cpe",
			readme: Readme{
				Name:             "test",
				SecurityCritical: "no",
				Licenses:         []string{"MIT"},
				LicenseFiles:     []string{"LICENSE"},
			},
			expectedLink: "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata#url",
		},
		{
			name: "missing security critical",
			readme: Readme{
				Name:         "test",
				URL:          "https://example.com",
				Revision:     "1234",
				Licenses:     []string{"MIT"},
				LicenseFiles: []string{"LICENSE"},
			},
			expectedLink: "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata#security-critical",
		},
		{
			name: "invalid security critical",
			readme: Readme{
				Name:             "test",
				URL:              "https://example.com",
				Revision:         "1234",
				SecurityCritical: "maybe",
				Licenses:         []string{"MIT"},
				LicenseFiles:     []string{"LICENSE"},
			},
			expectedLink: "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata#security-critical",
		},
		{
			name: "missing license",
			readme: Readme{
				Name:             "test",
				URL:              "https://example.com",
				Revision:         "1234",
				SecurityCritical: "no",
				LicenseFiles:     []string{"LICENSE"},
			},
			expectedLink: "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata#license",
		},
		{
			name: "missing license file",
			readme: Readme{
				Name:             "test",
				URL:              "https://example.com",
				Revision:         "1234",
				SecurityCritical: "no",
				Licenses:         []string{"MIT"},
			},
			expectedLink: "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata#license-file",
		},
		{
			name: "unknown fields",
			readme: Readme{
				Name:             "test",
				URL:              "https://example.com",
				Revision:         "1234",
				SecurityCritical: "no",
				Licenses:         []string{"MIT"},
				LicenseFiles:     []string{"LICENSE"},
				UnknownFields:    []UnknownField{{Key: "Foo", Value: "Bar"}},
			},
			expectedLink: "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata#syntax",
		},
		{
			name: "invalid first party",
			readme: Readme{
				Name:             "test",
				URL:              "https://example.com",
				Revision:         "1234",
				SecurityCritical: "no",
				FirstParty:       "maybe",
				Licenses:         []string{"MIT"},
				LicenseFiles:     []string{"LICENSE"},
			},
			expectedLink: "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			errs := Validate("", []*Readme{&tc.readme})
			if len(errs) == 0 {
				t.Fatalf("expected validation error, got none")
			}
			found := false
			for _, err := range errs {
				if strings.Contains(err.Error(), tc.expectedLink) {
					found = true
					break
				}
			}
			if !found {
				t.Errorf("expected error containing %q, got: %v", tc.expectedLink, errs)
			}
		})
	}
}

func TestValidate_FirstParty_Success(t *testing.T) {
	tmpDir := t.TempDir()
	if err := os.WriteFile(filepath.Join(tmpDir, "LICENSE"), []byte("MIT License"), 0644); err != nil {
		t.Fatalf("failed to write license file: %v", err)
	}

	readme := Readme{
		Name:             "test",
		URL:              "https://example.com",
		Revision:         "1234",
		SecurityCritical: "no",
		FirstParty:       "yes",
		Licenses:         []string{"MIT"},
		LicenseFiles:     []string{"LICENSE"},
	}
	errs := Validate(tmpDir, []*Readme{&readme})
	if len(errs) != 0 {
		t.Fatalf("expected validation success for valid FirstParty, got: %v", errs)
	}

	// Minimal first-party README with only Name and FirstParty
	minimalReadme := Readme{
		Name:       "test",
		FirstParty: "yes",
	}
	minimalErrs := Validate(tmpDir, []*Readme{&minimalReadme})
	if len(minimalErrs) != 0 {
		t.Fatalf("expected validation success for minimal FirstParty, got: %v", minimalErrs)
	}
}

func TestValidate_LineSpansAndReplacements(t *testing.T) {
	tmpDir := t.TempDir()
	readmeContent := `Name: sample_lib
URL: https://example.com
Revision: 1234
Security Critical: false
License: MIT
License File: NONEXISTENT_LICENSE
Unknown Directive: value
`
	readmePath := filepath.Join(tmpDir, "README.fuchsia")
	if err := os.WriteFile(readmePath, []byte(readmeContent), 0644); err != nil {
		t.Fatalf("failed to write test file: %v", err)
	}

	readmes, err := ParseFile(readmePath)
	if err != nil {
		t.Fatalf("failed to parse file: %v", err)
	}

	errs := Validate(tmpDir, readmes)
	if len(errs) != 3 {
		t.Fatalf("expected 3 validation errors, got %d: %v", len(errs), errs)
	}

	var foundSecurityCritical, foundLicenseFile, foundUnknownField bool
	for _, err := range errs {
		f, ok := err.(Finding)
		if !ok {
			t.Fatalf("expected error to be of type Finding, got %T", err)
		}
		if f.FilePath != readmePath {
			t.Errorf("expected FilePath %q, got %q", readmePath, f.FilePath)
		}

		if strings.Contains(f.Message, "Security Critical") {
			foundSecurityCritical = true
			if f.Line != 4 || f.EndLine != 4 {
				t.Errorf("expected Security Critical line span 4-4, got %d-%d", f.Line, f.EndLine)
			}
			if len(f.Replacements) == 0 || f.Replacements[0] != "Security Critical: no" {
				t.Errorf("expected replacement 'Security Critical: no', got %v", f.Replacements)
			}
		} else if strings.Contains(f.Message, "License File does not exist") {
			foundLicenseFile = true
			if f.Line != 6 || f.EndLine != 6 {
				t.Errorf("expected License File line span 6-6, got %d-%d", f.Line, f.EndLine)
			}
		} else if strings.Contains(f.Message, "Found unknown/invalid fields") {
			foundUnknownField = true
			if f.Line != 7 || f.EndLine != 7 {
				t.Errorf("expected Unknown Directive line span 7-7, got %d-%d", f.Line, f.EndLine)
			}
		}
	}

	if !foundSecurityCritical || !foundLicenseFile || !foundUnknownField {
		t.Errorf("missing expected errors: sec_crit=%v, lic_file=%v, unknown=%v", foundSecurityCritical, foundLicenseFile, foundUnknownField)
	}
}

func TestValidate_GeneratedNoticeFile(t *testing.T) {
	tmpDir := t.TempDir()
	if err := os.WriteFile(filepath.Join(tmpDir, "LICENSE"), []byte("MIT License"), 0644); err != nil {
		t.Fatalf("failed to write license file: %v", err)
	}
	if err := os.WriteFile(filepath.Join(tmpDir, "NOTICE.fuchsia"), []byte("Notice content"), 0644); err != nil {
		t.Fatalf("failed to write notice file: %v", err)
	}

	readme := Readme{
		Name:                 "test",
		URL:                  "https://example.com",
		Revision:             "1234",
		SecurityCritical:     "no",
		Licenses:             []string{"MIT"},
		LicenseFiles:         []string{"LICENSE"},
		GeneratedNoticeFiles: []string{"NOTICE.fuchsia"},
	}

	errs := Validate(tmpDir, []*Readme{&readme})
	if len(errs) != 0 {
		t.Fatalf("expected validation success for existing Generated Notice File, got: %v", errs)
	}

	readmeMissing := Readme{
		Name:                 "test",
		URL:                  "https://example.com",
		Revision:             "1234",
		SecurityCritical:     "no",
		Licenses:             []string{"MIT"},
		LicenseFiles:         []string{"LICENSE"},
		GeneratedNoticeFiles: []string{"NONEXISTENT_NOTICE"},
	}

	errsMissing := Validate(tmpDir, []*Readme{&readmeMissing})
	if len(errsMissing) == 0 {
		t.Fatalf("expected validation error for missing Generated Notice File, got none")
	}
	found := false
	for _, err := range errsMissing {
		if strings.Contains(err.Error(), "Generated Notice File does not exist") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("expected error containing 'Generated Notice File does not exist', got: %v", errsMissing)
	}
}

func TestValidate_GeneratedNoticeFile_CoLocatedWithReadme(t *testing.T) {
	tmpDir := t.TempDir()
	virtualDir := filepath.Join(tmpDir, "assets", "readmes", "prebuilt", "sample")
	if err := os.MkdirAll(virtualDir, 0755); err != nil {
		t.Fatalf("failed to create virtual dir: %v", err)
	}
	projectRoot := filepath.Join(tmpDir, "prebuilt", "sample")
	if err := os.MkdirAll(projectRoot, 0755); err != nil {
		t.Fatalf("failed to create project root: %v", err)
	}

	if err := os.WriteFile(filepath.Join(projectRoot, "LICENSE"), []byte("MIT License"), 0644); err != nil {
		t.Fatalf("failed to write license file: %v", err)
	}
	readme := Readme{
		FilePath:             filepath.Join(virtualDir, "README.fuchsia"),
		Name:                 "sample",
		URL:                  "https://example.com",
		Revision:             "1234",
		SecurityCritical:     "no",
		Licenses:             []string{"MIT"},
		LicenseFiles:         []string{"LICENSE"},
		GeneratedNoticeFiles: []string{"NOTICE.fuchsia"},
	}

	// Missing NOTICE.fuchsia in virtualDir should fail validation.
	errs := Validate(projectRoot, []*Readme{&readme})
	if len(errs) == 0 {
		t.Fatalf("expected validation failure when NOTICE.fuchsia is missing from virtualDir")
	}

	// Writing NOTICE.fuchsia into virtualDir should now succeed.
	if err := os.WriteFile(filepath.Join(virtualDir, "NOTICE.fuchsia"), []byte("Notice content"), 0644); err != nil {
		t.Fatalf("failed to write notice file: %v", err)
	}

	errs = Validate(projectRoot, []*Readme{&readme})
	if len(errs) != 0 {
		t.Fatalf("expected validation success for notice co-located with virtual README, got: %v", errs)
	}
}

func TestValidate_SourceFileIgnored(t *testing.T) {
	tmpDir := t.TempDir()
	readmeContent := `Name: sample_lib
URL: https://example.com
Revision: 1234
Security Critical: yes
License: MIT
License File: LICENSE
Source File: foo/bar.cc
Source File: baz/qux.cc
`
	if err := os.WriteFile(filepath.Join(tmpDir, "LICENSE"), []byte("MIT License"), 0644); err != nil {
		t.Fatalf("failed to write license file: %v", err)
	}
	readmePath := filepath.Join(tmpDir, "README.fuchsia")
	if err := os.WriteFile(readmePath, []byte(readmeContent), 0644); err != nil {
		t.Fatalf("failed to write test file: %v", err)
	}

	readmes, err := ParseFile(readmePath)
	if err != nil {
		t.Fatalf("failed to parse file: %v", err)
	}

	if len(readmes[0].UnknownFields) != 0 {
		t.Fatalf("expected 0 unknown fields, got: %v", readmes[0].UnknownFields)
	}

	errs := Validate(tmpDir, readmes)
	if len(errs) != 0 {
		t.Fatalf("expected 0 validation errors, got: %v", errs)
	}
}
