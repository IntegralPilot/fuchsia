// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/readme_fuchsia"
)

func TestTargetComplianceVerifier_Declared(t *testing.T) {
	tempDir := t.TempDir()
	sourceFile := filepath.Join(tempDir, "foo.cc")
	os.WriteFile(sourceFile, []byte("int main() {}"), 0644)
	licenseFile := filepath.Join(tempDir, "LICENSE")
	os.WriteFile(licenseFile, []byte("LICENSE"), 0644)

	orig := &readme_fuchsia.Readme{
		Name:             "foo",
		URL:              "http://foo",
		Version:          "1.0",
		Revision:         "123",
		SecurityCritical: "no",
		Licenses:         []string{"MIT"},
		LicenseFiles:     []string{"LICENSE"},
	}
	clone := *orig

	proj := &pipeline.Project{
		RootPath: tempDir,
		Readme: &pipeline.ReadmeFile{
			Path: filepath.Join(tempDir, "README.fuchsia"),
			Segments: []*pipeline.ReadmeSegment{
				{Original: orig, Updated: &clone},
			},
		},
		ClassifiedFiles: []pipeline.ClassifiedFile{
			{
				Path:        sourceFile,
				ProjectRoot: tempDir,
				Matches: []pipeline.LicenseMatch{
					{SPDXID: "MIT", MatchType: "Notice"},
				},
			},
		},
	}

	v := NewTargetComplianceVerifier(tempDir, nil, "foo.cc")
	if err := v.Run(context.Background(), []*pipeline.Project{proj}, nil); err != nil {
		t.Errorf("Expected success for declared license, got: %v", err)
	}
}

func TestTargetComplianceVerifier_Undeclared(t *testing.T) {
	tempDir := t.TempDir()
	sourceFile := filepath.Join(tempDir, "bar.cc")
	os.WriteFile(sourceFile, []byte("int main() {}"), 0644)
	licenseFile := filepath.Join(tempDir, "LICENSE")
	os.WriteFile(licenseFile, []byte("LICENSE"), 0644)

	orig := &readme_fuchsia.Readme{
		Name:             "foo",
		URL:              "http://foo",
		Version:          "1.0",
		Revision:         "123",
		SecurityCritical: "no",
		Licenses:         []string{"MIT"},
		LicenseFiles:     []string{"LICENSE"},
	}
	clone := *orig

	proj := &pipeline.Project{
		RootPath: tempDir,
		Readme: &pipeline.ReadmeFile{
			Path: filepath.Join(tempDir, "README.fuchsia"),
			Segments: []*pipeline.ReadmeSegment{
				{Original: orig, Updated: &clone},
			},
		},
		ClassifiedFiles: []pipeline.ClassifiedFile{
			{
				Path:        sourceFile,
				ProjectRoot: tempDir,
				Matches: []pipeline.LicenseMatch{
					{SPDXID: "GPL-2.0", MatchType: "Restricted"},
				},
			},
		},
	}

	v := NewTargetComplianceVerifier(tempDir, nil, "bar.cc")
	if err := v.Run(context.Background(), []*pipeline.Project{proj}, nil); err == nil {
		t.Errorf("Expected error for undeclared license, got nil")
	}
}

func TestTargetComplianceVerifier_FirstParty(t *testing.T) {
	tempDir := t.TempDir()
	sourceFile := filepath.Join(tempDir, "src", "lib", "valid.cc")
	os.MkdirAll(filepath.Dir(sourceFile), 0755)
	os.WriteFile(sourceFile, []byte("int main() {}"), 0644)
	licenseFile := filepath.Join(tempDir, "LICENSE")
	os.WriteFile(licenseFile, []byte("LICENSE"), 0644)

	orig := &readme_fuchsia.Readme{
		Name:             "Fuchsia",
		SecurityCritical: "yes",
		FirstParty:       "yes",
		Licenses:         []string{"BSD-2-Clause"},
		LicenseFiles:     []string{"LICENSE"},
	}
	clone := *orig

	proj := &pipeline.Project{
		RootPath: tempDir,
		Readme: &pipeline.ReadmeFile{
			Path: filepath.Join(tempDir, "tools", "check-licenses", "assets", "readmes", "README.fuchsia"),
			Segments: []*pipeline.ReadmeSegment{
				{Original: orig, Updated: &clone},
			},
		},
		ClassifiedFiles: []pipeline.ClassifiedFile{
			{
				Path:         sourceFile,
				ProjectRoot:  tempDir,
				IsFirstParty: true,
			},
		},
	}

	v := NewTargetComplianceVerifier(tempDir, nil, "src/lib/valid.cc")
	if err := v.Run(context.Background(), []*pipeline.Project{proj}, nil); err != nil {
		t.Errorf("Expected success for 1st-party source file, got: %v", err)
	}
}

func TestTargetComplianceVerifier_ComplianceErrors(t *testing.T) {
	tempDir := t.TempDir()
	sourceFile := filepath.Join(tempDir, "bad.cc")
	os.WriteFile(sourceFile, []byte("int main() {}"), 0644)

	complianceErr := pipeline.ComplianceError{
		CheckName: "AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders",
		Project:   tempDir,
		FilePath:  sourceFile,
		Issue:     "Missing Fuchsia copyright header in first-party source file.",
	}

	v := NewTargetComplianceVerifier(tempDir, nil, "bad.cc")
	err := v.Run(context.Background(), nil, []pipeline.ComplianceError{complianceErr})
	if err == nil {
		t.Fatal("Expected error when compliance error is present for target, got nil")
	}
	if !strings.Contains(err.Error(), "Missing Fuchsia copyright header") {
		t.Errorf("Expected error to contain copyright header message, got: %v", err)
	}
}

func TestTargetComplianceVerifier_ReadmeOutOfDate(t *testing.T) {
	tempDir := t.TempDir()
	sourceFile := filepath.Join(tempDir, "foo.cc")
	os.WriteFile(sourceFile, []byte("int main() {}"), 0644)
	licenseFile := filepath.Join(tempDir, "LICENSE")
	os.WriteFile(licenseFile, []byte("LICENSE"), 0644)
	apacheFile := filepath.Join(tempDir, "apache.cc")
	os.WriteFile(apacheFile, []byte("/* Apache 2.0 */"), 0644)
	readmePath := filepath.Join(tempDir, "README.fuchsia")
	os.WriteFile(readmePath, []byte("Name: foo\n"), 0644)

	orig := &readme_fuchsia.Readme{
		Name:             "foo",
		URL:              "http://foo",
		Version:          "1.0",
		Revision:         "123",
		SecurityCritical: "no",
		Licenses:         []string{"MIT"},
		LicenseFiles:     []string{"LICENSE"},
	}
	clone := *orig

	proj := &pipeline.Project{
		RootPath: tempDir,
		Readme: &pipeline.ReadmeFile{
			Path: readmePath,
			Segments: []*pipeline.ReadmeSegment{
				{Original: orig, Updated: &clone},
			},
		},
		ClassifiedFiles: []pipeline.ClassifiedFile{
			{
				Path:          licenseFile,
				ProjectRoot:   tempDir,
				IsLicenseFile: true,
				Matches: []pipeline.LicenseMatch{
					{SPDXID: "MIT", MatchType: "Notice"},
				},
			},
			{
				Path:        apacheFile,
				ProjectRoot: tempDir,
				Matches: []pipeline.LicenseMatch{
					{SPDXID: "Apache-2.0", MatchType: "Notice"},
				},
				AnalyzedText: []byte("/* Apache 2.0 */"),
			},
		},
	}

	v := NewTargetComplianceVerifier(tempDir, nil, "README.fuchsia")
	err := v.Run(context.Background(), []*pipeline.Project{proj}, nil)
	if err == nil {
		t.Fatal("Expected error for out-of-date README declarations, got nil")
	}
	if len(v.Findings) != 1 {
		t.Fatalf("Expected 1 finding, got %d: %+v", len(v.Findings), v.Findings)
	}
	f := v.Findings[0]
	if f.CheckName != CheckReadmeDeclarationOutOfDate {
		t.Errorf("Expected %s, got %s", CheckReadmeDeclarationOutOfDate, f.CheckName)
	}
	if len(f.Replacements) == 0 || !strings.Contains(f.Replacements[0], "Apache-2.0") {
		t.Errorf("Expected replacement containing updated licenses, got %+v", f.Replacements)
	}
}

func TestTargetComplianceVerifier_NoticeOutOfDate(t *testing.T) {
	tempDir := t.TempDir()
	sourceFile := filepath.Join(tempDir, "source.cc")
	os.WriteFile(sourceFile, []byte("/* BSD Notice */\nint main() {}"), 0644)
	licenseFile := filepath.Join(tempDir, "LICENSE")
	licenseContent := []byte("MIT License\nPermission is hereby granted...")
	os.WriteFile(licenseFile, licenseContent, 0644)
	readmePath := filepath.Join(tempDir, "README.fuchsia")
	os.WriteFile(readmePath, []byte("Name: foo\n"), 0644)

	orig := &readme_fuchsia.Readme{
		Name:                 "foo",
		URL:                  "http://foo",
		Version:              "1.0",
		Revision:             "123",
		SecurityCritical:     "no",
		Licenses:             []string{"MIT"},
		LicenseFiles:         []string{"LICENSE"},
		GeneratedNoticeFiles: []string{"NOTICE.fuchsia"},
	}
	clone := *orig

	noticePath := filepath.Join(tempDir, "NOTICE.fuchsia")
	os.WriteFile(noticePath, []byte("Old Notice Content"), 0644)

	proj := &pipeline.Project{
		RootPath: tempDir,
		Readme: &pipeline.ReadmeFile{
			Path: readmePath,
			Segments: []*pipeline.ReadmeSegment{
				{Original: orig, Updated: &clone},
			},
		},
		ClassifiedFiles: []pipeline.ClassifiedFile{
			{
				Path:          licenseFile,
				ProjectRoot:   tempDir,
				IsLicenseFile: true,
				Matches: []pipeline.LicenseMatch{
					{SPDXID: "MIT", MatchType: "Notice"},
				},
				AnalyzedText: licenseContent,
			},
			{
				Path:        sourceFile,
				ProjectRoot: tempDir,
				Matches: []pipeline.LicenseMatch{
					{SPDXID: "BSD", MatchType: "Notice", Text: []byte("BSD Notice Text")},
				},
				AnalyzedText: []byte("/* BSD Notice */\nint main() {}"),
			},
		},
	}

	v := NewTargetComplianceVerifier(tempDir, nil, "NOTICE.fuchsia")
	err := v.Run(context.Background(), []*pipeline.Project{proj}, nil)
	if err == nil {
		t.Fatal("Expected error for out-of-date NOTICE.fuchsia, got nil")
	}
	var noticeFindings []Finding
	for _, f := range v.Findings {
		if f.CheckName == CheckNoticeFileOutOfDate {
			noticeFindings = append(noticeFindings, f)
		}
	}
	if len(noticeFindings) != 1 {
		t.Fatalf("Expected exactly 1 deduplicated NoticeFileOutOfDate finding, got %d: %+v", len(noticeFindings), noticeFindings)
	}
	if len(noticeFindings[0].Replacements) == 0 || !strings.Contains(noticeFindings[0].Replacements[0], "BSD Notice Text") {
		t.Errorf("Expected replacement to contain BSD Notice Text, got %+v", noticeFindings[0].Replacements)
	}
}

func TestTargetComplianceVerifier_DirectoryTargetIgnoresUnrelatedFileErrors(t *testing.T) {
	tempDir := t.TempDir()
	dirA := filepath.Join(tempDir, "dirA")
	dirB := filepath.Join(tempDir, "dirB")
	os.MkdirAll(dirA, 0755)
	os.MkdirAll(dirB, 0755)
	unrelatedFile := filepath.Join(dirA, "unrelated.cc")
	os.WriteFile(unrelatedFile, []byte("int main() {}"), 0644)
	targetFile := filepath.Join(dirB, "target.cc")
	os.WriteFile(targetFile, []byte("int main() {}"), 0644)

	complianceErr := pipeline.ComplianceError{
		CheckName: "AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders",
		Project:   tempDir,
		FilePath:  unrelatedFile,
		Issue:     "Missing Fuchsia copyright header in unrelated file.",
	}

	v := NewTargetComplianceVerifier(tempDir, nil, dirB)
	err := v.Run(context.Background(), nil, []pipeline.ComplianceError{complianceErr})
	if err != nil {
		t.Fatalf("Expected directory target dirB to have no errors, but got: %v", err)
	}
	if len(v.Findings) != 0 {
		t.Fatalf("Expected 0 findings for dirB, got %d: %+v", len(v.Findings), v.Findings)
	}
}
