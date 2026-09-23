// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/readme"
)

// Check names emitted by TargetComplianceVerifier.
const (
	CheckUndeclaredLicense          = "UndeclaredLicense"
	CheckReadmeDeclarationOutOfDate = "ReadmeDeclarationOutOfDate"
	CheckNoticeFileOutOfDate        = "NoticeFileOutOfDate"
)

// TargetComplianceVerifier checks that individual target files or sub-directories
// have their detected licenses declared in their parent README.fuchsia.
type TargetComplianceVerifier struct {
	FuchsiaDir       string
	TargetPaths      []string
	Config           readme.Config
	PreserveExisting bool
	Findings         []Finding
}

// NewTargetComplianceVerifier creates a new TargetComplianceVerifier.
func NewTargetComplianceVerifier(fuchsiaDir string, config readme.Config, targetPaths ...string) *TargetComplianceVerifier {
	var canonicalTargets []string
	for _, tp := range targetPaths {
		if tp == "" {
			continue
		}
		clean := filepath.Clean(tp)
		if !filepath.IsAbs(clean) && fuchsiaDir != "" {
			clean = filepath.Join(fuchsiaDir, clean)
		}
		canonicalTargets = append(canonicalTargets, clean)
	}
	return &TargetComplianceVerifier{
		FuchsiaDir:  fuchsiaDir,
		TargetPaths: canonicalTargets,
		Config:      config,
	}
}

func (v *TargetComplianceVerifier) relPath(path string) string {
	if path == "" {
		return ""
	}
	if r, err := filepath.Rel(v.FuchsiaDir, path); err == nil && r != "." && !strings.HasPrefix(r, "..") {
		return filepath.ToSlash(r)
	}
	return filepath.ToSlash(path)
}

func (v *TargetComplianceVerifier) Run(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
	v.Findings = nil
	if len(v.TargetPaths) == 0 {
		return nil
	}

	targetIsDir := make(map[string]bool, len(v.TargetPaths))
	for _, tp := range v.TargetPaths {
		if info, err := os.Stat(tp); err == nil && info.IsDir() {
			targetIsDir[tp] = true
		}
	}

	// 1. Check compliance and policy errors for target paths.
	// First-party code is governed by repository compliance policies (e.g. required Fuchsia
	// copyright headers) rather than in-tree README manifests. The validate stage produces
	// ComplianceErrors for policy violations; match them against the targeted files/directories.
	if len(errors) > 0 {
		for _, e := range errors {
			filePath := e.FilePath
			if !filepath.IsAbs(filePath) && filePath != "" && v.FuchsiaDir != "" {
				filePath = filepath.Join(v.FuchsiaDir, filePath)
			}
			projPath := e.Project
			if !filepath.IsAbs(projPath) && projPath != "" && v.FuchsiaDir != "" {
				projPath = filepath.Join(v.FuchsiaDir, projPath)
			}

			for _, targetPath := range v.TargetPaths {
				if targetPath == "" {
					continue
				}
				isDir := targetIsDir[targetPath]

				matches := false
				if isDir {
					if filePath != "" {
						rel, err := filepath.Rel(targetPath, filePath)
						if err == nil && !strings.HasPrefix(rel, "..") {
							matches = true
						}
					} else if projPath != "" {
						rel, err := filepath.Rel(projPath, targetPath)
						if err == nil && !strings.HasPrefix(rel, "..") {
							matches = true
						}
					}
				} else {
					if filePath == targetPath {
						matches = true
					} else if filePath == "" && projPath != "" {
						rel, err := filepath.Rel(projPath, targetPath)
						if err == nil && !strings.HasPrefix(rel, "..") {
							matches = true
						}
					}
				}

				if matches {
					relPath := v.relPath(e.FilePath)
					if relPath == "" {
						relPath = v.relPath(targetPath)
					}
					v.Findings = append(v.Findings, Finding{
						FilePath:     relPath,
						Line:         e.StartLine,
						EndLine:      e.EndLine,
						Level:        "error",
						Message:      e.Issue,
						CheckName:    e.CheckName,
						Replacements: e.Replacements,
					})
					break
				}
			}
		}
	}

	projectNotices := make(map[string]map[string]string)
	getNotices := func(proj *pipeline.Project) map[string]string {
		if n, ok := projectNotices[proj.RootPath]; ok {
			return n
		}
		n := readme.UpdateWithClassifiedFilesDryRun(v.FuchsiaDir, proj.RootPath, proj.Readme.UpdatedSegments(), proj.FoundLicenses(), v.PreserveExisting)
		projectNotices[proj.RootPath] = n
		return n
	}

	for _, targetPath := range v.TargetPaths {
		if targetPath == "" {
			continue
		}

		absTarget := targetPath
		isDir := targetIsDir[absTarget]

		for _, proj := range projects {
			if proj.Readme == nil || len(proj.Readme.Segments) == 0 {
				continue
			}

			relTargetClean, err := filepath.Rel(proj.RootPath, absTarget)
			targetInProj := err == nil && !strings.HasPrefix(relTargetClean, "..")
			projInTarget := false
			if isDir {
				relProjFromTarget, err := filepath.Rel(absTarget, proj.RootPath)
				projInTarget = err == nil && !strings.HasPrefix(relProjFromTarget, "..")
			}
			if !targetInProj && !projInTarget {
				continue
			}

			relProj := v.relPath(proj.RootPath)

			declaredPrimary := make(map[string]bool)
			for _, r := range proj.Readme.OriginalSegments() {
				for _, l := range r.Licenses {
					declaredPrimary[l] = true
				}
			}

			for _, cf := range proj.FoundLicenses() {
				relCf, _ := filepath.Rel(proj.RootPath, cf.Path)
				// For directory targets, only evaluate files that reside within the targeted sub-directory.
				if isDir {
					relToTarget, err := filepath.Rel(absTarget, cf.Path)
					if err != nil || strings.HasPrefix(relToTarget, "..") {
						continue
					}
				} else if filepath.Clean(relCf) != filepath.Clean(relTargetClean) {
					continue
				}
				for _, match := range cf.Matches {
					if match.MatchType != "Copyright" && !strings.HasPrefix(match.MatchType, "_") {
						if !declaredPrimary[match.SPDXID] {
							relPath := v.relPath(cf.Path)
							v.Findings = append(v.Findings, Finding{
								FilePath:  relPath,
								Line:      match.StartLine,
								EndLine:   match.EndLine,
								Level:     "error",
								CheckName: CheckUndeclaredLicense,
								Message: fmt.Sprintf(
									"Undeclared license header (%s) found in source file.\n\n"+
										"Details:\n"+
										"  - Project: %s\n"+
										"  - File: %s\n"+
										"  - License: %s\n\n"+
										"Remediation:\n"+
										"  Run `fx check-licenses project update %s`\n\n"+
										"Documentation:\n"+
										"  https://fuchsia.dev/fuchsia-src/contribute/governance/policy/open-source-licensing-policies",
									match.SPDXID, relProj, relPath, match.SPDXID, relProj),
							})
						}
					}
				}
			}

			// 1st-party projects are governed by virtual READMEs and do not maintain in-tree README.fuchsia manifests.
			if proj.IsFirstParty() {
				continue
			}

			notices := getNotices(proj)

			origs := proj.Readme.OriginalSegments()
			updated := proj.Readme.UpdatedSegments()
			formattedReadme := readme.Format(updated)
			readmeMatchesAll := readme.DeclarationsMatchAll(origs, updated)

			relReadme := v.relPath(proj.Readme.Path)

			isReadmeTarget := filepath.Clean(absTarget) == filepath.Clean(proj.Readme.Path)

			var expectedEntry string
			var actualEntry string
			targetNeedsNotice := false
			if !isDir && !isReadmeTarget {
				relTarget, err := filepath.Rel(proj.RootPath, absTarget)
				if err == nil {
					relTarget = filepath.ToSlash(filepath.Clean(relTarget))
					for _, r := range updated {
						for _, lf := range r.LicenseFiles {
							if filepath.Clean(lf) == relTarget {
								expectedEntry = lf
								break
							}
						}
						if expectedEntry != "" {
							break
						}
					}

					for _, r := range origs {
						for _, lf := range r.LicenseFiles {
							if filepath.Clean(lf) == relTarget {
								actualEntry = lf
								break
							}
						}
						if actualEntry != "" {
							break
						}
					}

					// Check if this target is covered by a generated notice file
					pattern := fmt.Sprintf("  - %s\n", relTarget)
					for _, noticeContent := range notices {
						if strings.Contains(noticeContent, pattern) {
							targetNeedsNotice = true
							break
						}
					}
				}
			}

			targetMissingFromReadme := (expectedEntry != "" && actualEntry == "") || (targetNeedsNotice && len(origs) > 0 && len(origs[0].GeneratedNoticeFiles) == 0)
			if (isDir || isReadmeTarget || targetMissingFromReadme) && !readmeMatchesAll {
				v.Findings = append(v.Findings, Finding{
					FilePath:  relReadme,
					Level:     "error",
					CheckName: CheckReadmeDeclarationOutOfDate,
					Message: fmt.Sprintf(
						"License declarations in README.fuchsia are out of date.\n\n"+
							"Details:\n"+
							"  - Project: %s\n"+
							"  - File: %s\n\n"+
							"Remediation:\n"+
							"  Run `fx check-licenses project update %s`\n"+
							"  or apply the suggested replacement.\n\n"+
							"Documentation:\n"+
							"  https://fuchsia.dev/fuchsia-src/contribute/governance/policy/open-source-licensing-policies",
						relProj, relReadme, relProj),
					Replacements: []string{formattedReadme},
				})
			}

			// Check NOTICE.fuchsia files if this target is a directory or specifically targeting NOTICE.fuchsia,
			// or if target file should be covered by NOTICE.fuchsia and is missing or out of date.
			for noticePath, noticeContent := range notices {
				isNoticeTarget := filepath.Clean(absTarget) == filepath.Clean(noticePath)
				if !isDir && !isNoticeTarget && !targetNeedsNotice {
					continue
				}
				existingNotice, err := os.ReadFile(noticePath)
				if err != nil || string(existingNotice) != noticeContent {
					relNotice := v.relPath(noticePath)
					v.Findings = append(v.Findings, Finding{
						FilePath:  relNotice,
						Level:     "error",
						CheckName: CheckNoticeFileOutOfDate,
						Message: fmt.Sprintf(
							"Generated notice file is out of date or missing.\n\n"+
								"Details:\n"+
								"  - Project: %s\n"+
								"  - File: %s\n\n"+
								"Remediation:\n"+
								"  Run `fx check-licenses project update %s`\n"+
								"  or apply the suggested replacement.\n\n"+
								"Documentation:\n"+
								"  https://fuchsia.dev/fuchsia-src/contribute/governance/policy/open-source-licensing-policies",
							relProj, relNotice, relProj),
						Replacements: []string{noticeContent},
					})
				}
			}
		}
	}

	v.Findings = DeduplicateFindings(v.Findings)
	if len(v.Findings) > 0 {
		return fmt.Errorf("%s", v.Findings[0].Message)
	}

	return nil
}
