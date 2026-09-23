// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"path/filepath"
	"sort"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/readme_fuchsia"
)

// Validate checks if the README.fuchsia file structures contain all required fields
// and no unknown fields. It also verifies that referenced paths exist on disk.
// Returns a slice of all encountered errors.
func Validate(fuchsiaDir, readmeFilePath string, readmes []*Readme, config Config) []error {
	var outOfTree map[string]string
	if config != nil {
		outOfTree = config.OutOfTreeReadmes()
	}
	var firstReadme *Readme
	if len(readmes) > 0 {
		firstReadme = readmes[0]
	}
	readmeDir := ResolveProjectRoot(firstReadme, readmeFilePath, fuchsiaDir, outOfTree)

	relBaseDir, err := filepath.Rel(fuchsiaDir, readmeDir)
	if err != nil {
		relBaseDir = readmeDir
	}
	if relBaseDir == "." {
		relBaseDir = ""
	}

	allowMissingLicense := false
	if config != nil {
		allowMissingLicense = config.HasPolicyException("AllProjectsMustHaveALicense", relBaseDir)
	}

	allowReadmeNeedsUpdate := false
	if config != nil {
		allowReadmeNeedsUpdate = config.HasPolicyException("ReadmeFuchsiaNeedsUpdate", relBaseDir)
	}

	if allowReadmeNeedsUpdate {
		return nil
	}

	errs := readme_fuchsia.Validate(readmeDir, readmes)

	if allowMissingLicense && len(errs) > 0 {
		var filteredErrs []error
		for _, err := range errs {
			msg := err.Error()
			isMissingLicenseErr := strings.Contains(msg, "Missing required field 'License'") ||
				strings.Contains(msg, "Missing required field 'License File'") ||
				strings.Contains(msg, "Missing required field 'Generated Notice File'")
			if !isMissingLicenseErr {
				filteredErrs = append(filteredErrs, err)
			}
		}
		errs = filteredErrs
	}

	return errs
}

// DeclarationsMatch checks if the LicenseFiles and GeneratedNoticeFiles slices match between two Readme structs.
func DeclarationsMatch(a, b *Readme) bool {
	if a == nil || b == nil {
		return a == b
	}
	return compareSlices(a.LicenseFiles, b.LicenseFiles) && compareSlices(a.GeneratedNoticeFiles, b.GeneratedNoticeFiles)
}

// DeclarationsMatchAll checks if all corresponding Readme segments in two slices have matching LicenseFiles and GeneratedNoticeFiles.
func DeclarationsMatchAll(a, b []*Readme) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if !DeclarationsMatch(a[i], b[i]) {
			return false
		}
	}
	return true
}

func compareSlices(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	aCopy := append([]string(nil), a...)
	bCopy := append([]string(nil), b...)
	sort.Strings(aCopy)
	sort.Strings(bCopy)
	for i := range aCopy {
		if aCopy[i] != bCopy[i] {
			return false
		}
	}
	return true
}
