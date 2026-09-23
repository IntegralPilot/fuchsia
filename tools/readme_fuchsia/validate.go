// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme_fuchsia

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const docURL = "https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata"

// Validate checks if the README.fuchsia file structures contain all required fields
// and no unknown fields. It also verifies that referenced paths exist on disk.
func Validate(projectRoot string, readmes []*Readme) []error {
	var errs []error
	var baseDir = projectRoot

	for i, r := range readmes {
		filePath := r.FilePath
		defaultSpan := r.BlockSpan
		if defaultSpan.StartLine == 0 {
			defaultSpan = LineSpan{StartLine: 1, EndLine: 1}
		}

		getSpan := func(key string) LineSpan {
			if r.Spans != nil {
				if s, ok := r.Spans[key]; ok && s.StartLine > 0 {
					return s
				}
			}
			return defaultSpan
		}

		addFinding := func(msg string, span LineSpan, replacements ...string) {
			if span.StartLine == 0 {
				span = defaultSpan
			}
			errs = append(errs, Finding{
				FilePath:     filePath,
				Line:         span.StartLine,
				EndLine:      span.EndLine,
				Level:        "error",
				Message:      msg,
				Replacements: replacements,
			})
		}

		var currentDir string
		if i == 0 {
			currentDir = baseDir
		} else {
			if r.Location != "" {
				currentDir = filepath.Join(baseDir, r.Location)
				if _, err := os.Stat(currentDir); os.IsNotExist(err) {
					addFinding(fmt.Sprintf("[%d]: 'Location' directory does not exist: %s (%s)", i+1, r.Location, docURL), getSpan("Location"))
				}
			} else {
				currentDir = baseDir // Fallback
			}
		}

		// Check 1: Unknown fields
		for _, uf := range r.UnknownFields {
			addFinding(fmt.Sprintf("[%d]: Found unknown/invalid fields: [{Key:%s Value:%s}] (%s#syntax)", i+1, uf.Key, uf.Value, docURL), uf.Span)
		}

		// Check 2: Required Fields
		if r.Name == "" {
			addFinding(fmt.Sprintf("[%d]: Missing required field 'Name' (%s#name)", i+1, docURL), defaultSpan)
		}

		if r.FirstParty != "" && r.FirstParty != "yes" && r.FirstParty != "no" {
			var repl []string
			if strings.EqualFold(r.FirstParty, "true") {
				repl = []string{"First Party: yes"}
			} else if strings.EqualFold(r.FirstParty, "false") {
				repl = []string{"First Party: no"}
			}
			addFinding(fmt.Sprintf("[%d]: Field 'First Party' has an unknown value. Required 'yes' or 'no', got %q (%s)", i+1, r.FirstParty, docURL), getSpan("First Party"), repl...)
		}

		if r.FirstParty != "yes" {
			hasUrlAndRev := r.URL != "" && r.Revision != ""
			hasCpeAndVer := r.CPEPrefix != "" && r.Version != ""
			if !hasUrlAndRev && !hasCpeAndVer {
				span := defaultSpan
				if r.URL != "" {
					span = getSpan("URL")
				}
				addFinding(fmt.Sprintf("[%d]: Missing required fields. Must specify either ('URL' AND 'Revision') OR ('CPEPrefix' AND 'Version') (%s#url)", i+1, docURL), span)
			}

			if r.SecurityCritical == "" {
				addFinding(fmt.Sprintf("[%d]: Missing required field 'Security Critical' (%s#security-critical)", i+1, docURL), defaultSpan)
			} else if r.SecurityCritical != "yes" && r.SecurityCritical != "no" {
				var repl []string
				if strings.EqualFold(r.SecurityCritical, "true") {
					repl = []string{"Security Critical: yes"}
				} else if strings.EqualFold(r.SecurityCritical, "false") {
					repl = []string{"Security Critical: no"}
				}
				addFinding(fmt.Sprintf("[%d]: Field 'Security Critical' has an unknown value. Required 'yes' or 'no', got %q (%s#security-critical)", i+1, r.SecurityCritical, docURL), getSpan("Security Critical"), repl...)
			}
			if len(r.Licenses) == 0 {
				addFinding(fmt.Sprintf("[%d]: Missing required field 'License' (%s#license)", i+1, docURL), defaultSpan)
			}
			if len(r.LicenseFiles) == 0 {
				addFinding(fmt.Sprintf("[%d]: Missing required field 'License File'. At least one must be specified. (%s#license-file)", i+1, docURL), defaultSpan)
			}
		}

		if i > 0 && r.Location == "" {
			addFinding(fmt.Sprintf("[%d]: Missing required field 'Location' for sub-project defined after a DEPENDENCY DIVIDER (%s)", i+1, docURL), defaultSpan)
		}

		for _, lf := range r.LicenseFiles {
			filePathOnDisk := filepath.Join(currentDir, lf)
			if _, err := os.Stat(filePathOnDisk); os.IsNotExist(err) {
				addFinding(fmt.Sprintf("[%d]: License File does not exist: %s (%s#license-file)", i+1, filePathOnDisk, docURL), getSpan(lf))
			}
		}

		for _, nlf := range r.NonLicenseFiles {
			filePathOnDisk := filepath.Join(currentDir, nlf)
			if _, err := os.Stat(filePathOnDisk); os.IsNotExist(err) {
				addFinding(fmt.Sprintf("[%d]: Non-License File does not exist: %s (%s)", i+1, filePathOnDisk, docURL), getSpan(nlf))
			}
		}

		for _, gnf := range r.GeneratedNoticeFiles {
			noticeDir := currentDir
			if r.FilePath != "" {
				noticeDir = filepath.Dir(r.FilePath)
				if loc := filepath.Clean(r.Location); loc != "" && loc != "." {
					subDir := filepath.Join(noticeDir, loc)
					if stat, err := os.Stat(subDir); err == nil && stat.IsDir() {
						noticeDir = subDir
					}
				}
			}
			filePathOnDisk := filepath.Join(noticeDir, gnf)
			if _, err := os.Stat(filePathOnDisk); os.IsNotExist(err) {
				addFinding(fmt.Sprintf("[%d]: Generated Notice File does not exist: %s (%s#license-file)", i+1, filePathOnDisk, docURL), getSpan(gnf))
			}
		}
	}

	return errs
}
