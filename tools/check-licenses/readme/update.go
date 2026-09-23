// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

// UpdateWithClassifiedFiles updates a slice of Readmes in-place with the given classified files
// and writes generated NOTICE.fuchsia files to disk.
// It maps each classified file to the correct sub-project (based on Location) and populates the LicenseFiles arrays.
// Files that match any NonLicenseFile entries are ignored.
// If preserveExisting is true, existing license file entries and NOTICE.fuchsia files are preserved
// rather than being cleared or deleted (e.g. for targeted or incremental updates).
// Returns a map of absolute notice file paths to their generated notice content.
func UpdateWithClassifiedFiles(fuchsiaDir, absDir string, readmes []*Readme, foundLicenses []pipeline.ClassifiedFile, preserveExisting bool) map[string]string {
	return updateWithClassifiedFilesInternal(fuchsiaDir, absDir, readmes, foundLicenses, preserveExisting, true)
}

// UpdateWithClassifiedFilesDryRun updates a slice of Readmes in memory without modifying NOTICE.fuchsia files on disk.
// Returns a map of absolute notice file paths to their generated notice content.
func UpdateWithClassifiedFilesDryRun(fuchsiaDir, absDir string, readmes []*Readme, foundLicenses []pipeline.ClassifiedFile, preserveExisting bool) map[string]string {
	return updateWithClassifiedFilesInternal(fuchsiaDir, absDir, readmes, foundLicenses, preserveExisting, false)
}

func updateWithClassifiedFilesInternal(fuchsiaDir, absDir string, readmes []*Readme, foundLicenses []pipeline.ClassifiedFile, preserveExisting, writeFiles bool) map[string]string {
	fileToReadme := make(map[string]*Readme)

	for _, cf := range foundLicenses {
		relToFile, _ := filepath.Rel(absDir, cf.Path)
		var bestMatch *Readme
		bestPrefixLength := -1

		for _, r := range readmes {
			loc := filepath.Clean(r.Location)
			if loc == "" || loc == "." {
				if bestPrefixLength < 0 {
					bestMatch = r
					bestPrefixLength = 0
				}
			} else {
				if strings.HasPrefix(relToFile, loc+"/") || relToFile == loc {
					if len(loc) > bestPrefixLength {
						bestMatch = r
						bestPrefixLength = len(loc)
					}
				}
			}
		}
		if bestMatch != nil {
			fileToReadme[cf.Path] = bestMatch
		}
	}

	isPrimaryLicenseFile := make(map[string]bool)
	primaryLicensesByReadme := make(map[*Readme]map[string]bool)
	for _, r := range readmes {
		primaryLicensesByReadme[r] = make(map[string]bool)
		if preserveExisting {
			for _, l := range r.Licenses {
				primaryLicensesByReadme[r][l] = true
			}
		}
	}

	for _, cf := range foundLicenses {
		r := fileToReadme[cf.Path]
		if r == nil {
			continue
		}
		relToReadme, _ := filepath.Rel(absDir, cf.Path)
		if isGeneratedNoticeFile(cf.Path, r, relToReadme) {
			continue
		}

		isPrimary := cf.IsLicenseFile
		if !isPrimary {
			for _, lf := range r.LicenseFiles {
				if lf == relToReadme {
					isPrimary = true
					break
				}
			}
		}

		if isPrimary {
			isPrimaryLicenseFile[cf.Path] = true
			for _, m := range cf.Matches {
				if m.MatchType != "Copyright" && !strings.HasPrefix(m.MatchType, "_") {
					primaryLicensesByReadme[r][m.SPDXID] = true
				}
			}
		}
	}

	readmeSourceFiles := make(map[*Readme][]sourceMatchInfo)

	if !preserveExisting {
		for _, r := range readmes {
			r.LicenseFiles = nil
			r.GeneratedNoticeFiles = nil
			r.Licenses = nil
		}
	}

	for _, cf := range foundLicenses {
		r := fileToReadme[cf.Path]
		if r == nil {
			continue
		}

		relToReadme, _ := filepath.Rel(absDir, cf.Path)
		if isGeneratedNoticeFile(cf.Path, r, relToReadme) {
			continue
		}

		relToFuchsia, _ := filepath.Rel(fuchsiaDir, cf.Path)
		isNonLicense := false
		for _, nlf := range r.NonLicenseFiles {
			if filepath.Clean(nlf) == relToReadme || filepath.Clean(nlf) == relToFuchsia {
				isNonLicense = true
				break
			}
		}
		if isNonLicense {
			continue
		}

		lics := make(map[string]bool)
		for _, m := range cf.Matches {
			if m.IsLicense() {
				lics[m.SPDXID] = true
			}
		}

		if isPrimaryLicenseFile[cf.Path] && len(lics) == 0 {
			lics["Unclassified"] = true
		}

		if !isPrimaryLicenseFile[cf.Path] {
			if len(lics) == 0 {
				continue
			}
			isSubset := true
			for l := range lics {
				if !primaryLicensesByReadme[r][l] {
					isSubset = false
					break
				}
			}
			if isSubset {
				continue
			}
		}

		if isPrimaryLicenseFile[cf.Path] {
			r.LicenseFiles = append(r.LicenseFiles, relToReadme)
			for l := range lics {
				r.Licenses = append(r.Licenses, l)
			}
		} else {
			for l := range lics {
				r.Licenses = append(r.Licenses, l)
			}
			readmeSourceFiles[r] = append(readmeSourceFiles[r], sourceMatchInfo{
				relPath: relToReadme,
				cf:      cf,
			})
		}
	}

	notices := make(map[string]string)
	for _, r := range readmes {
		noticeDir := absDir
		if r.FilePath != "" {
			noticeDir = filepath.Dir(r.FilePath)
		}
		if loc := filepath.Clean(r.Location); loc != "" && loc != "." {
			subDir := filepath.Join(noticeDir, loc)
			if stat, err := os.Stat(subDir); err == nil && stat.IsDir() {
				noticeDir = subDir
			}
		}
		noticePath := filepath.Join(noticeDir, "NOTICE.fuchsia")

		if len(readmeSourceFiles[r]) > 0 {
			content := generateNoticeContent(readmeSourceFiles[r])
			notices[noticePath] = content
			r.GeneratedNoticeFiles = []string{"NOTICE.fuchsia"}
			if writeFiles {
				_ = os.MkdirAll(noticeDir, 0755)
				_ = os.WriteFile(noticePath, []byte(content), 0644)
			}
		} else if !preserveExisting {
			if writeFiles {
				_ = os.Remove(noticePath)
			}
			r.GeneratedNoticeFiles = nil
		}

		r.Licenses = deduplicateAndSort(r.Licenses)
		r.LicenseFiles = deduplicateAndSort(r.LicenseFiles)
		r.GeneratedNoticeFiles = deduplicateAndSort(r.GeneratedNoticeFiles)
	}
	return notices
}

type sourceMatchInfo struct {
	relPath string
	cf      pipeline.ClassifiedFile
}

func generateNoticeContent(matches []sourceMatchInfo) string {
	type noticeBlock struct {
		text  string
		files []string
	}
	blockMap := make(map[string]*noticeBlock)

	for _, m := range matches {
		for _, lm := range m.cf.Matches {
			if lm.MatchType == "Copyright" || strings.HasPrefix(lm.MatchType, "_") {
				continue
			}
			txt := strings.TrimSpace(string(lm.Text))
			if txt == "" {
				continue
			}
			nb, ok := blockMap[txt]
			if !ok {
				nb = &noticeBlock{text: txt}
				blockMap[txt] = nb
			}
			nb.files = append(nb.files, m.relPath)
		}
	}

	var blocks []*noticeBlock
	for _, nb := range blockMap {
		nb.files = deduplicateAndSort(nb.files)
		blocks = append(blocks, nb)
	}

	sort.Slice(blocks, func(i, j int) bool {
		if len(blocks[i].files) > 0 && len(blocks[j].files) > 0 {
			if blocks[i].files[0] != blocks[j].files[0] {
				return blocks[i].files[0] < blocks[j].files[0]
			}
		}
		return blocks[i].text < blocks[j].text
	})

	var b strings.Builder
	for _, nb := range blocks {
		b.WriteString("================================================================================\n")
		b.WriteString("The following files are covered by this license:\n")
		for _, f := range nb.files {
			b.WriteString(fmt.Sprintf("  - %s\n", f))
		}
		b.WriteString("--------------------------------------------------------------------------------\n")
		b.WriteString(nb.text)
		b.WriteString("\n================================================================================\n\n")
	}

	return strings.TrimSpace(b.String()) + "\n"
}

func deduplicateAndSort(items []string) []string {
	seen := make(map[string]bool)
	var result []string
	for _, item := range items {
		trimmed := strings.TrimSpace(item)
		if trimmed != "" && !seen[trimmed] {
			seen[trimmed] = true
			result = append(result, trimmed)
		}
	}
	sort.Strings(result)
	return result
}

// FilterClassifiedFiles returns only the classified files that contain valid license matches
// (excluding copyright-only matches and internal metadata matches starting with '_').
func FilterClassifiedFiles(files []pipeline.ClassifiedFile) []pipeline.ClassifiedFile {
	var found []pipeline.ClassifiedFile
	for _, cf := range files {
		if cf.HasLicenses() {
			found = append(found, cf)
		}
	}
	return found
}

func isGeneratedNoticeFile(path string, r *Readme, relToReadme string) bool {
	if filepath.Base(path) == "NOTICE.fuchsia" {
		return true
	}
	for _, gnf := range r.GeneratedNoticeFiles {
		if filepath.Clean(gnf) == relToReadme {
			return true
		}
	}
	return false
}
