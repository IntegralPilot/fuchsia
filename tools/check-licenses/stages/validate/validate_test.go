// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package validate

import (
	"context"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

func TestValidator_Run(t *testing.T) {
	fuchsiaDir := t.TempDir()

	policyExceptions := map[string]map[string]RuleMetadata{
		"AllLicenseTextsMustBeRecognized": {
			"third_party/foo/LICENSE": RuleMetadata{Bug: "test", Description: "test"},
		},
		"AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders": {
			"src/legacy/old.cc": RuleMetadata{Bug: "test", Description: "test"},
		},
		"AllProjectsMustHaveALicense": {
			"third_party/foo": RuleMetadata{Bug: "test", Description: "test"},
			"third_party/bar": RuleMetadata{Bug: "test", Description: "test"},
		},
	}

	allowedLicenses := map[string]map[string]RuleMetadata{
		"GPL-2.0": {
			"third_party/legacy_gpl/LICENSE": RuleMetadata{Bug: "test", Description: "test"},
		},
	}

	copyrightExtensions := map[string]bool{
		".cc": true,
		".py": true,
	}

	validator := NewValidator(fuchsiaDir, Config{
		PolicyExceptions:    policyExceptions,
		AllowedLicenses:     allowedLicenses,
		CopyrightExtensions: copyrightExtensions,
	})

	inChan := make(chan pipeline.ClassifiedFile, 15)

	// 1. Valid License File
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "LICENSE"),
		IsLicenseFile: true,
		Matches:       []pipeline.LicenseMatch{{SPDXID: "MIT", MatchType: "Permissive"}},
	}

	// 2. Invalid License File (No matches, not allowlisted)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "third_party", "bar", "LICENSE"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "third_party", "bar"),
		IsLicenseFile: true,
		HasReadme:     true,
		Matches:       []pipeline.LicenseMatch{},
	}

	// 3. Invalid License File (No matches, BUT allowlisted)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "third_party", "foo", "LICENSE"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "third_party", "foo"),
		IsLicenseFile: true,
		HasReadme:     true,
		Matches:       []pipeline.LicenseMatch{},
	}

	// 4. Valid Source File
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "src", "main.cc"),
		ProjectRoot:   fuchsiaDir,
		IsLicenseFile: false,
		Matches:       []pipeline.LicenseMatch{{SPDXID: "FuchsiaCopyright", MatchType: "Copyright"}},
		AnalyzedText:  []byte("// Copyright 2026 The Fuchsia Authors. All rights reserved.\n// Use of this source code is governed by a BSD-style license that can be\n// found in the LICENSE file.\n"),
	}

	// 5. Invalid Source File (No copyright, not allowlisted)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "src", "bad.cc"),
		ProjectRoot:   fuchsiaDir,
		IsLicenseFile: false,
		Matches:       []pipeline.LicenseMatch{{SPDXID: "MIT", MatchType: "Permissive"}}, // MIT is not FuchsiaCopyright
	}

	// 6. Invalid Source File (No copyright, BUT allowlisted)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "src", "legacy", "old.cc"),
		ProjectRoot:   fuchsiaDir,
		IsLicenseFile: false,
		Matches:       []pipeline.LicenseMatch{},
	}

	// 7. Non-Fuchsia Source File (No copyright, but it's third-party)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "third_party", "foo", "main.cc"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "third_party", "foo"),
		IsLicenseFile: false,
		HasReadme:     true,
		Matches:       []pipeline.LicenseMatch{},
	}

	// 8. Non-source extension (No copyright, not allowlisted, but extension skips check)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "src", "image.jpg"),
		ProjectRoot:   fuchsiaDir,
		IsLicenseFile: false,
		Matches:       []pipeline.LicenseMatch{},
	}

	// 9. Restricted License File (Not allowlisted)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "third_party", "bad_gpl", "LICENSE"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "third_party", "bad_gpl"),
		IsLicenseFile: true,
		HasReadme:     true,
		Matches:       []pipeline.LicenseMatch{{SPDXID: "GPL-2.0", MatchType: "Restricted"}},
	}

	// 10. Restricted License File (Allowlisted)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "third_party", "legacy_gpl", "LICENSE"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "third_party", "legacy_gpl"),
		IsLicenseFile: true,
		HasReadme:     true,
		Matches:       []pipeline.LicenseMatch{{SPDXID: "GPL-2.0", MatchType: "Restricted"}},
	}

	// 11. Empty __init__.py (Exempted)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "src", "__init__.py"),
		ProjectRoot:   fuchsiaDir,
		IsLicenseFile: false,
		Matches:       []pipeline.LicenseMatch{},
		AnalyzedText:  []byte{},
	}

	// 12. Non-empty __init__.py (Not exempted)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "src", "sub", "__init__.py"),
		ProjectRoot:   fuchsiaDir,
		IsLicenseFile: false,
		Matches:       []pipeline.LicenseMatch{},
		AnalyzedText:  []byte("print('hello')"),
	}

	close(inChan)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	outChan, err := validator.Run(ctx, inChan)
	if err != nil {
		t.Fatalf("Failed to run validator: %v", err)
	}

	var errors []pipeline.ComplianceError
	for err := range outChan {
		errors = append(errors, err)
	}

	if len(errors) != 4 {
		t.Fatalf("Expected exactly 4 compliance errors, got %d: %v", len(errors), errors)
	}

	hasUnrecognizedLicenseErr := false
	hasMissingCopyrightErr := false
	hasMissingInitCopyrightErr := false
	hasUnapprovedPatternErr := false

	for _, e := range errors {
		if strings.Contains(e.Issue, "Unrecognized license text") {
			if e.FilePath != filepath.Join(fuchsiaDir, "third_party", "bar", "LICENSE") {
				t.Errorf("Unexpected unrecognized license error for file: %s", e.FilePath)
			}
			hasUnrecognizedLicenseErr = true
		}
		if strings.Contains(e.Issue, "Missing Fuchsia copyright header") {
			if e.FilePath == filepath.Join(fuchsiaDir, "src", "bad.cc") {
				hasMissingCopyrightErr = true
			} else if e.FilePath == filepath.Join(fuchsiaDir, "src", "sub", "__init__.py") {
				hasMissingInitCopyrightErr = true
				if len(e.Replacements) != 1 {
					t.Errorf("Expected 1 replacement for missing copyright, got %d", len(e.Replacements))
				} else if !strings.Contains(e.Replacements[0], "The Fuchsia Authors") || !strings.Contains(e.Replacements[0], "print('hello')") {
					t.Errorf("Replacement content unexpected: %q", e.Replacements[0])
				}
			} else {
				t.Errorf("Unexpected missing copyright error for file: %s", e.FilePath)
			}
		}
		if strings.Contains(e.Issue, "was not approved to use license pattern") {
			if e.FilePath != filepath.Join(fuchsiaDir, "third_party", "bad_gpl", "LICENSE") {
				t.Errorf("Unexpected unapproved pattern error for file: %s", e.FilePath)
			}
			hasUnapprovedPatternErr = true
		}
	}

	if !hasUnrecognizedLicenseErr {
		t.Error("Expected unrecognized license error, but it was not emitted")
	}
	if !hasMissingCopyrightErr {
		t.Error("Expected missing copyright error for bad.cc, but it was not emitted")
	}
	if !hasMissingInitCopyrightErr {
		t.Error("Expected missing copyright error for non-empty __init__.py, but it was not emitted")
	}
	if !hasUnapprovedPatternErr {
		t.Error("Expected unapproved pattern error, but it was not emitted")
	}

	for _, e := range errors {
		assertFindingStructure(t, e.Issue, true)
	}
}

func assertFindingStructure(t *testing.T, issue string, checkBugID bool) {
	t.Helper()
	if strings.Contains(issue, "<BugID>") {
		t.Errorf("Error message must not contain <BugID>: %s", issue)
	}
	if checkBugID && !strings.Contains(issue, "BUG_ID") {
		t.Errorf("Error message must contain 'BUG_ID': %s", issue)
	}
	for _, section := range []string{"Details:", "Remediation:", "Documentation:"} {
		if !strings.Contains(issue, section) {
			t.Errorf("Error message must contain %q section: %s", section, issue)
		}
	}
}

func TestValidator_RunFailure_MissingLicense(t *testing.T) {
	fuchsiaDir := t.TempDir()
	validator := NewValidator(fuchsiaDir, Config{})

	inChan := make(chan pipeline.ClassifiedFile, 1)

	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "third_party/foo/main.cc"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "third_party/foo"),
		IsLicenseFile: false,
		HasReadme:     true,
		Matches:       []pipeline.LicenseMatch{},
	}
	close(inChan)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	outChan, err := validator.Run(ctx, inChan)
	if err != nil {
		t.Fatalf("Failed to run validator: %v", err)
	}

	var errors []pipeline.ComplianceError
	for err := range outChan {
		errors = append(errors, err)
	}

	if len(errors) != 1 {
		t.Fatalf("Expected 1 error due to missing license file, got %d: %v", len(errors), errors)
	}

	if errors[0].CheckName != PolicyNoLicense {
		t.Errorf("Expected check name %s, got: %s", PolicyNoLicense, errors[0].CheckName)
	}
	if !strings.Contains(errors[0].Issue, "Project has no recognized license files") {
		t.Errorf("Expected error to contain missing license issue description, got: %v", errors[0].Issue)
	}
	assertFindingStructure(t, errors[0].Issue, true)
}

func TestValidator_RunFailure_MissingReadme(t *testing.T) {
	fuchsiaDir := t.TempDir()
	validator := NewValidator(fuchsiaDir, Config{})

	inChan := make(chan pipeline.ClassifiedFile, 1)

	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "third_party/foo/LICENSE"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "third_party/foo"),
		IsLicenseFile: true,
		HasReadme:     false,
		Matches:       []pipeline.LicenseMatch{{SPDXID: "Apache-2.0", MatchType: "Approved"}},
	}
	close(inChan)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	outChan, err := validator.Run(ctx, inChan)
	if err != nil {
		t.Fatalf("Failed to run validator: %v", err)
	}

	var errors []pipeline.ComplianceError
	for err := range outChan {
		errors = append(errors, err)
	}

	if len(errors) != 1 {
		t.Fatalf("Expected 1 error due to missing readme, got %d: %v", len(errors), errors)
	}

	if errors[0].CheckName != PolicyNoReadme {
		t.Errorf("Expected check name %s, got: %s", PolicyNoReadme, errors[0].CheckName)
	}
	if !strings.Contains(errors[0].Issue, "Third-party project is missing a README.fuchsia file") {
		t.Errorf("Expected error to contain missing readme issue description, got: %v", errors[0].Issue)
	}
	if !strings.Contains(errors[0].Issue, "Or add a virtual README to tools/check-licenses/assets/readmes/third_party/foo/README.fuchsia") {
		t.Errorf("Expected error to contain public virtual README path, got: %s", errors[0].Issue)
	}
	assertFindingStructure(t, errors[0].Issue, true)
}

func TestValidator_RunFailure_MissingReadme_PrivateAndVendor(t *testing.T) {
	fuchsiaDir := t.TempDir()

	// 1. Private vendor project without custom resolver -> should suggest vendor/google/tools/check-licenses/assets/readmes
	validatorDefault := NewValidator(fuchsiaDir, Config{})
	inChan := make(chan pipeline.ClassifiedFile, 1)
	inChan <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "vendor/google/secret/LICENSE"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "vendor/google/secret"),
		IsLicenseFile: true,
		HasReadme:     false,
		Matches:       []pipeline.LicenseMatch{{SPDXID: "Apache-2.0", MatchType: "Approved"}},
	}
	close(inChan)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	outChan, err := validatorDefault.Run(ctx, inChan)
	if err != nil {
		t.Fatalf("Failed to run validator: %v", err)
	}

	var errors []pipeline.ComplianceError
	for err := range outChan {
		errors = append(errors, err)
	}

	if len(errors) != 1 {
		t.Fatalf("Expected 1 error, got %d", len(errors))
	}
	expectedPrivateMsg := "Or add a virtual README to vendor/google/tools/check-licenses/assets/readmes/vendor/google/secret/README.fuchsia"
	if !strings.Contains(errors[0].Issue, expectedPrivateMsg) {
		t.Errorf("Expected error to contain %q, got: %s", expectedPrivateMsg, errors[0].Issue)
	}
	if got := validatorDefault.virtualReadmeDir("//vendor/google/secret"); got != "vendor/google/tools/check-licenses/assets/readmes" {
		t.Errorf("virtualReadmeDir('//vendor/google/secret') = %q, want vendor/google assets", got)
	}
	if got := validatorDefault.virtualReadmeDir("//third_party/foo"); got != "tools/check-licenses/assets/readmes" {
		t.Errorf("virtualReadmeDir('//third_party/foo') = %q, want public assets", got)
	}

	// 2. Custom VirtualReadmeDir resolver (e.g. for Jiri private projects in prebuilt/ or partner vendor repos)
	validatorCustom := NewValidator(fuchsiaDir, Config{
		VirtualReadmeDir: func(projectPath string) string {
			if strings.HasPrefix(projectPath, "prebuilt/internal/") {
				return "vendor/google/tools/check-licenses/assets/readmes"
			}
			if strings.HasPrefix(projectPath, "vendor/partner/") {
				return "vendor/partner/tools/check-licenses/assets/readmes"
			}
			return "tools/check-licenses/assets/readmes"
		},
	})

	inChan2 := make(chan pipeline.ClassifiedFile, 2)
	inChan2 <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "prebuilt/internal/firmware/LICENSE"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "prebuilt/internal/firmware"),
		IsLicenseFile: true,
		HasReadme:     false,
		Matches:       []pipeline.LicenseMatch{{SPDXID: "Apache-2.0", MatchType: "Approved"}},
	}
	inChan2 <- pipeline.ClassifiedFile{
		Path:          filepath.Join(fuchsiaDir, "vendor/partner/pkg/LICENSE"),
		ProjectRoot:   filepath.Join(fuchsiaDir, "vendor/partner/pkg"),
		IsLicenseFile: true,
		HasReadme:     false,
		Matches:       []pipeline.LicenseMatch{{SPDXID: "Apache-2.0", MatchType: "Approved"}},
	}
	close(inChan2)

	ctx2, cancel2 := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel2()

	outChan2, err := validatorCustom.Run(ctx2, inChan2)
	if err != nil {
		t.Fatalf("Failed to run validator: %v", err)
	}

	var errors2 []pipeline.ComplianceError
	for err := range outChan2 {
		errors2 = append(errors2, err)
	}

	if len(errors2) != 2 {
		t.Fatalf("Expected 2 errors, got %d", len(errors2))
	}

	expectedPrebuiltMsg := "Or add a virtual README to vendor/google/tools/check-licenses/assets/readmes/prebuilt/internal/firmware/README.fuchsia"
	expectedPartnerMsg := "Or add a virtual README to vendor/partner/tools/check-licenses/assets/readmes/vendor/partner/pkg/README.fuchsia"

	foundPrebuilt := false
	foundPartner := false
	for _, e := range errors2 {
		if strings.Contains(e.Issue, expectedPrebuiltMsg) {
			foundPrebuilt = true
		}
		if strings.Contains(e.Issue, expectedPartnerMsg) {
			foundPartner = true
		}
	}
	if !foundPrebuilt {
		t.Errorf("Expected to find error with %q", expectedPrebuiltMsg)
	}
	if !foundPartner {
		t.Errorf("Expected to find error with %q", expectedPartnerMsg)
	}
}

func TestAddCopyrightToBytes_ShebangAndLineEndings(t *testing.T) {
	// 1. Unix line endings with shebang
	unixContent := []byte("#!/usr/bin/env python\nprint('hello')\n")
	resUnix, err := AddCopyrightToBytes("foo.py", unixContent)
	if err != nil {
		t.Fatalf("AddCopyrightToBytes failed: %v", err)
	}
	expectedShebangUnix := "#!/usr/bin/env python\n# Copyright"
	if !strings.HasPrefix(string(resUnix), expectedShebangUnix) {
		t.Errorf("Expected Unix shebang prefix, got: %s", string(resUnix)[:50])
	}
	if strings.Contains(string(resUnix), "\r") {
		t.Errorf("Expected no carriage return in Unix output")
	}

	// 2. Windows CRLF line endings with shebang
	crlfContent := []byte("#!/usr/bin/env bash\r\necho hi\r\n")
	resCrlf, err := AddCopyrightToBytes("script.sh", crlfContent)
	if err != nil {
		t.Fatalf("AddCopyrightToBytes failed: %v", err)
	}
	expectedShebangCrlf := "#!/usr/bin/env bash\r\n# Copyright"
	if !strings.HasPrefix(string(resCrlf), expectedShebangCrlf) {
		t.Errorf("Expected CRLF shebang prefix, got: %q", string(resCrlf)[:50])
	}
	// Check that there are no standalone \n (all newlines must be \r\n)
	s := string(resCrlf)
	sWithoutCrlf := strings.ReplaceAll(s, "\r\n", "")
	if strings.Contains(sWithoutCrlf, "\n") || strings.Contains(sWithoutCrlf, "\r") {
		t.Errorf("Expected all line breaks in CRLF content to be \\r\\n")
	}
}

func TestIsProjectScopePolicy(t *testing.T) {
	if !IsProjectScopePolicy(PolicyNoLicense) {
		t.Errorf("Expected PolicyNoLicense to be project-scoped")
	}
	if !IsProjectScopePolicy(PolicyNoReadme) {
		t.Errorf("Expected PolicyNoReadme to be project-scoped")
	}
	if IsProjectScopePolicy(PolicyUnrecognizedLicense) {
		t.Errorf("Expected PolicyUnrecognizedLicense NOT to be project-scoped")
	}
	if IsProjectScopePolicy(PolicyFuchsiaCopyright) {
		t.Errorf("Expected PolicyFuchsiaCopyright NOT to be project-scoped")
	}
	if IsProjectScopePolicy("UnknownPolicy") {
		t.Errorf("Expected UnknownPolicy NOT to be project-scoped")
	}
}

func TestIsValidPolicy(t *testing.T) {
	for _, p := range ValidPolicies() {
		if !IsValidPolicy(p) {
			t.Errorf("Expected IsValidPolicy(%q) to be true", p)
		}
	}
	if IsValidPolicy("NonExistentPolicy") {
		t.Errorf("Expected IsValidPolicy('NonExistentPolicy') to be false")
	}
}

func TestValidPolicies_Sorted(t *testing.T) {
	policies := ValidPolicies()
	if !sort.StringsAreSorted(policies) {
		t.Errorf("Expected ValidPolicies() to be sorted, got: %v", policies)
	}
}
