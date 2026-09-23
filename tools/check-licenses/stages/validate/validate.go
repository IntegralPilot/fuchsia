// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package validate

import (
	"context"
	"fmt"
	"path/filepath"
	"sort"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/metrics"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

// Validator implements pipeline.Validator. It acts as the Policy Engine,
// consuming ClassifiedFiles and checking them against allowed policies.
type Validator struct {
	FuchsiaDir string
	Config
}

// NewValidator creates a new stateless policy engine.
func NewValidator(fuchsiaDir string, config Config) *Validator {
	if config.PolicyExceptions == nil {
		config.PolicyExceptions = make(map[string]map[string]RuleMetadata)
	}
	if config.AllowedLicenses == nil {
		config.AllowedLicenses = make(map[string]map[string]RuleMetadata)
	}
	if config.CopyrightExtensions == nil {
		config.CopyrightExtensions = make(map[string]bool)
	}

	// Ensure FuchsiaDir is absolute for consistent comparison
	absFuchsiaDir, err := filepath.Abs(fuchsiaDir)
	if err == nil {
		fuchsiaDir = absFuchsiaDir
	}

	return &Validator{
		FuchsiaDir: fuchsiaDir,
		Config:     config,
	}
}

// Run cross-references ClassifiedFiles against allowed policies and emits ComplianceErrors.
func (v *Validator) Run(ctx context.Context, in <-chan pipeline.ClassifiedFile) (<-chan pipeline.ComplianceError, error) {
	out := make(chan pipeline.ComplianceError, 100)

	go func() {
		defer close(out)
		defer metrics.ChecksDuration.Track()()

		projectHasLicense := make(map[string]bool)
		projectHasReadme := make(map[string]bool)
		projectIsFirstParty := make(map[string]bool)

		for cf := range in {
			if ctx.Err() != nil {
				return
			}

			if _, exists := projectHasLicense[cf.ProjectRoot]; !exists {
				projectHasLicense[cf.ProjectRoot] = false
			}
			if cf.HasReadme {
				projectHasReadme[cf.ProjectRoot] = true
			}
			if cf.IsFirstParty {
				projectIsFirstParty[cf.ProjectRoot] = true
			}

			// We need a consistent relative path for allowlist lookups
			relPath, err := filepath.Rel(v.FuchsiaDir, cf.Path)
			if err != nil {
				// If we can't make it relative, just use the original path
				relPath = cf.Path
			}

			// Some paths might be "."
			if relPath == "." {
				relPath = ""
			}
			relPath = filepath.ToSlash(relPath)

			isRecognizedOrAllowed := len(cf.Matches) > 0 || v.isPolicyExceptionAllowed(PolicyUnrecognizedLicense, relPath)
			if cf.IsLicenseFile && isRecognizedOrAllowed {
				projectHasLicense[cf.ProjectRoot] = true
			}

			// 1. Check: AllLicenseTextsMustBeRecognized
			// Explicit license files (like LICENSE or NOTICE) MUST have at least one recognized license,
			// UNLESS their file path is explicitly allowlisted.
			if cf.IsLicenseFile {
				if len(cf.Matches) == 0 {
					if !v.isPolicyExceptionAllowed(PolicyUnrecognizedLicense, relPath) {
						relProjRoot := v.formatRelProject(cf.ProjectRoot)
						metrics.ValidationErrors.Inc(PolicyUnrecognizedLicense)
						err := pipeline.ComplianceError{
							CheckName: PolicyUnrecognizedLicense,
							Project:   cf.ProjectRoot,
							FilePath:  cf.Path,
							Issue: fmt.Sprintf(
								"Unrecognized license text: no SPDX ID could be matched.\n\n"+
									"Details:\n"+
									"  - Project: %s\n"+
									"  - File: %s\n\n"+
									"Remediation:\n"+
									"  If this file is an exception, allow it by running:\n"+
									"    fx check-licenses policy add -bug BUG_ID AllLicenseTextsMustBeRecognized %s\n\n"+
									"Documentation:\n"+
									"  https://fuchsia.dev/fuchsia-src/contribute/governance/policy/open-source-licensing-policies",
								relProjRoot, relPath, relPath),
						}
						select {
						case <-ctx.Done():
							return
						case out <- err:
						}
					}
				}
			}

			// 2. Check: AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders
			// Source code owned by Fuchsia (ProjectRoot == FuchsiaDir or IsFirstParty) MUST have a FuchsiaCopyright,
			// UNLESS their file path is explicitly allowlisted.
			isFuchsiaProject := cf.ProjectRoot == v.FuchsiaDir || cf.ProjectRoot == "." || cf.ProjectRoot == "" || cf.IsFirstParty

			if !cf.IsLicenseFile && isFuchsiaProject {
				hasFuchsiaCopyright := CheckCopyrightText(cf.AnalyzedText)

				if !hasFuchsiaCopyright {
					if !v.isPolicyExceptionAllowed(PolicyFuchsiaCopyright, relPath) {
						// TODO(https://fxbug.dev/505430724): Skip empty __init__.py files
						if filepath.Base(cf.Path) == "__init__.py" && len(cf.AnalyzedText) == 0 {
							continue
						}

						// To avoid flagging every single JSON/XML/config file, we only enforce this on
						// common source code files that support comments.
						// The Crawler's TargetExtensions naturally handles this, but we do a sanity check.
						ext := strings.ToLower(filepath.Ext(cf.Path))
						if v.CopyrightExtensions[ext] {
							metrics.ValidationErrors.Inc(PolicyFuchsiaCopyright)
							var replacements []string
							if newBytes, err := AddCopyright(cf.Path); err == nil {
								replacements = []string{string(newBytes)}
							} else if len(cf.AnalyzedText) > 0 {
								if newBytes, err := AddCopyrightToBytes(cf.Path, cf.AnalyzedText); err == nil {
									replacements = []string{string(newBytes)}
								}
							}
							relProjRoot := v.formatRelProject(cf.ProjectRoot)
							err := pipeline.ComplianceError{
								CheckName: PolicyFuchsiaCopyright,
								Project:   cf.ProjectRoot,
								FilePath:  cf.Path,
								Issue: fmt.Sprintf(
									"Missing Fuchsia copyright header in first-party source file.\n\n"+
										"Details:\n"+
										"  - Project: %s\n"+
										"  - File: %s\n\n"+
										"Remediation:\n"+
										"  Fix this automatically by running:\n"+
										"    fx check-licenses copyright add %s\n"+
										"  Or allow an exception by running:\n"+
										"    fx check-licenses policy add -bug BUG_ID AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders %s\n\n"+
										"Documentation:\n"+
										"  https://fuchsia.dev/fuchsia-src/contribute/governance/policy/open-source-licensing-policies",
									relProjRoot, relPath, relPath, relPath),
								Replacements: replacements,
							}
							select {
							case <-ctx.Done():
								return
							case out <- err:
							}
						}
					}
				}
			}

			// 3. Check: AllLicensePatternUsagesMustBeApproved
			// Certain license patterns (like GPL) are restricted and must be explicitly approved for usage.
			if cf.IsLicenseFile {
				for _, match := range cf.Matches {
					needsApproval := true
					switch {
					case match.MatchType == "Copyright" || match.MatchType == "Approved" || match.MatchType == "Permissive" || match.MatchType == "Notice" || match.MatchType == "Unencumbered" || match.MatchType == "Unclassified" || match.SPDXID == "FuchsiaCopyright":
						needsApproval = false
					case strings.HasPrefix(match.MatchType, "_"):
						if _, isRestricted := v.AllowedLicenses[match.SPDXID]; !isRestricted {
							needsApproval = false
						}
					}

					if needsApproval {
						relProjRoot := v.formatRelProject(cf.ProjectRoot)
						if !v.isAllowedLicense(match.SPDXID, relPath, relProjRoot, cf.ProjectRoot) {
							metrics.ValidationErrors.Inc("UnapprovedLicenseUsage")
							startLine := match.StartLine
							endLine := match.EndLine
							if startLine > 0 && endLine < startLine {
								endLine = startLine
							}
							err := pipeline.ComplianceError{
								CheckName: CheckPatternApproval,
								LicenseID: match.SPDXID,
								Project:   cf.ProjectRoot,
								FilePath:  cf.Path,
								StartLine: startLine,
								EndLine:   endLine,
								Issue: fmt.Sprintf(
									"File was not approved to use license pattern %s.\n\n"+
										"Details:\n"+
										"  - Project: %s\n"+
										"  - File: %s\n"+
										"  - License Pattern: %s (%s)\n\n"+
										"Remediation:\n"+
										"  To allow this project to use this license, run:\n"+
										"    fx check-licenses allowlist add -bug BUG_ID %s %s\n\n"+
										"Documentation:\n"+
										"  https://fuchsia.dev/fuchsia-src/contribute/governance/policy/open-source-licensing-policies",
									match.SPDXID, relProjRoot, relPath, match.SPDXID, match.MatchType, match.SPDXID, relProjRoot),
							}
							select {
							case <-ctx.Done():
								return
							case out <- err:
							}
						}
					}
				}
			}
		}

		var projs []string
		for proj := range projectHasLicense {
			projs = append(projs, proj)
		}
		sort.Strings(projs)
		for _, proj := range projs {
			hasLicense := projectHasLicense[proj]
			isFirstParty := projectIsFirstParty[proj]
			if proj == v.FuchsiaDir || proj == "." || proj == "" || isFirstParty {
				continue
			}
			if !hasLicense {
				relProjRoot, _ := filepath.Rel(v.FuchsiaDir, proj)
				relProjRoot = filepath.ToSlash(relProjRoot)
				if !v.isPolicyExceptionAllowed(PolicyNoLicense, relProjRoot) {
					metrics.ValidationErrors.Inc(PolicyNoLicense)
					targetFile := ""
					if projectHasReadme[proj] {
						targetFile = filepath.Join(proj, "README.fuchsia")
					}
					err := pipeline.ComplianceError{
						CheckName: PolicyNoLicense,
						Project:   proj,
						FilePath:  targetFile,
						Issue: fmt.Sprintf(
							"Project has no recognized license files.\n\n"+
								"Details:\n"+
								"  - Project: %s\n"+
								"  - Requirement: Every third-party project must contain a license file.\n\n"+
								"Remediation:\n"+
								"  Add a LICENSE file to the project, or if this project is an exception, allow it by running:\n"+
								"    fx check-licenses policy add -bug BUG_ID AllProjectsMustHaveALicense %s\n\n"+
								"Documentation:\n"+
								"  https://fuchsia.dev/fuchsia-src/contribute/governance/policy/open-source-licensing-policies",
							relProjRoot, relProjRoot),
					}
					select {
					case <-ctx.Done():
						return
					case out <- err:
					}
				} else {
					metrics.AllowlistHits.Inc(PolicyNoLicense)
				}
			}

			// 5. Check: AllProjectsMustHaveAReadme
			// Every third-party project must contain a README.fuchsia or package manifest.
			hasReadme := projectHasReadme[proj]
			if !hasReadme {
				relProjRoot, _ := filepath.Rel(v.FuchsiaDir, proj)
				relProjRoot = filepath.ToSlash(relProjRoot)
				if !v.isPolicyExceptionAllowed(PolicyNoReadme, relProjRoot) {
					metrics.ValidationErrors.Inc(PolicyNoReadme)
					virtualReadmeDir := v.virtualReadmeDir(relProjRoot)
					err := pipeline.ComplianceError{
						CheckName: PolicyNoReadme,
						Project:   proj,
						FilePath:  "",
						Issue: fmt.Sprintf(
							"Third-party project is missing a README.fuchsia file.\n\n"+
								"Details:\n"+
								"  - Project: %s\n"+
								"  - Requirement: Every third-party project must have a README.fuchsia or package manifest.\n\n"+
								"Remediation:\n"+
								"  To fix this:\n"+
								"    - Add a README.fuchsia to %s/README.fuchsia\n"+
								"    - Or add a virtual README to %s/%s/README.fuchsia\n"+
								"    - Or allow an exception by running:\n"+
								"        fx check-licenses policy add -bug BUG_ID AllProjectsMustHaveAReadme %s\n\n"+
								"Documentation:\n"+
								"  https://fuchsia.dev/fuchsia-src/development/source_code/third-party-metadata",
							relProjRoot, relProjRoot, virtualReadmeDir, relProjRoot, relProjRoot),
					}
					select {
					case <-ctx.Done():
						return
					case out <- err:
					}
				} else {
					metrics.AllowlistHits.Inc(PolicyNoReadme)
				}
			}
		}
	}()

	return out, nil
}

func (v *Validator) formatRelProject(projectRoot string) string {
	relProjRoot, _ := filepath.Rel(v.FuchsiaDir, projectRoot)
	relProjRoot = filepath.ToSlash(relProjRoot)
	if relProjRoot == "" || relProjRoot == "." {
		return "root"
	}
	return relProjRoot
}

func (v *Validator) isPolicyExceptionAllowed(policyName, relPath string) bool {
	return isAllowed(v.PolicyExceptions, policyName, relPath)
}

func (v *Validator) isAllowedLicense(spdxID, relPath, relProjRoot, projectRoot string) bool {
	if list, ok := v.AllowedLicenses[spdxID]; ok {
		for _, path := range []string{relPath, relProjRoot, projectRoot} {
			if path != "" {
				if _, ok := list[path]; ok {
					metrics.AllowlistHits.Inc("AllowedLicenses_" + spdxID)
					return true
				}
			}
		}
	}
	return false
}

func isAllowed(targetMap map[string]map[string]RuleMetadata, key, targetPath string) bool {
	if list, ok := targetMap[key]; ok {
		slashPath := strings.TrimPrefix(filepath.ToSlash(filepath.Clean(targetPath)), "/")
		if _, ok := list[slashPath]; ok {
			metrics.AllowlistHits.Inc(key)
			return true
		}
	}
	return false
}

func (v *Validator) virtualReadmeDir(projectPath string) string {
	if v.Config.VirtualReadmeDir != nil {
		return v.Config.VirtualReadmeDir(projectPath)
	}
	cleanPath := strings.TrimPrefix(projectPath, "//")
	if strings.HasPrefix(cleanPath, "vendor/") || cleanPath == "vendor" {
		return "vendor/google/tools/check-licenses/assets/readmes"
	}
	return "tools/check-licenses/assets/readmes"
}
