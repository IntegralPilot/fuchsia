// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package pipeline

import (
	"context"
	"sort"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/readme_fuchsia"
)

type Readme = readme_fuchsia.Readme
type UnknownField = readme_fuchsia.UnknownField

// ReadmeSegment represents one package section in a README.fuchsia file.
type ReadmeSegment struct {
	Original *Readme
	Updated  *Readme
}

// ReadmeFile represents a README.fuchsia document containing one or more package segments.
type ReadmeFile struct {
	Path     string
	Segments []*ReadmeSegment
}

// OriginalSegments returns the original parsed Readme metadata structs.
func (rf *ReadmeFile) OriginalSegments() []*Readme {
	var result []*Readme
	for _, s := range rf.Segments {
		if s.Original != nil {
			result = append(result, s.Original)
		}
	}
	return result
}

// UpdatedSegments returns the updated Readme metadata structs.
func (rf *ReadmeFile) UpdatedSegments() []*Readme {
	var result []*Readme
	for _, s := range rf.Segments {
		if s.Updated != nil {
			result = append(result, s.Updated)
		}
	}
	return result
}

// RawPath represents the output of the Discovery Stage (Crawler).
type RawPath struct {
	Path  string
	IsDir bool
}

// FileInfo holds metadata about a specific file within a project.
type FileInfo struct {
	Path          string
	LicenseParser string // e.g., "Android", "Chromium", or "" (default)
	IsNonLicense  bool
	IsLicenseFile bool // Explicitly mark as primary license file
}

// Project represents the output of the Project Boundary Stage (Grouper).
type Project struct {
	RootPath        string
	Files           []FileInfo
	ManifestName    string // Package name in repository manifest, if known
	IsPrivate       bool   // True if project originates from a proprietary/private repository
	Readme          *ReadmeFile
	ClassifiedFiles []ClassifiedFile
}

// FoundLicenses returns all classified files that contain confirmed license matches, sorted by path.
func (p *Project) FoundLicenses() []ClassifiedFile {
	var found []ClassifiedFile
	for _, cf := range p.ClassifiedFiles {
		if cf.HasLicenses() {
			found = append(found, cf)
		}
	}
	sort.Slice(found, func(i, j int) bool {
		return found[i].Path < found[j].Path
	})
	return found
}

// IsFirstParty returns true if the project explicitly declares FirstParty == "yes" in its README.
func (p *Project) IsFirstParty() bool {
	if p.Readme != nil {
		for _, s := range p.Readme.OriginalSegments() {
			if s.FirstParty == "yes" {
				return true
			}
		}
	}
	return false
}

// FilteredProject represents the output of the Build Graph Filtering Stage (Pruner).
type FilteredProject struct {
	Project
	// Build graph specifics...
}

// LicenseMatch represents a specific license pattern detected within a file.
type LicenseMatch struct {
	SPDXID      string // e.g., "MIT", "Apache-2.0", "FuchsiaCopyright"
	MatchType   string // e.g., "Notice", "Restricted", "Forbidden"
	PatternName string // e.g., "Apache-with-LLVM-Exception.txt" (actual matching pattern filename)
	StartLine   int    // 1-based line number where the match begins
	EndLine     int    // 1-based line number where the match ends
	Text        []byte // The exact matched text block
}

// IsLicense returns true if the match is a valid license match (not a copyright header or internal metadata).
func (m LicenseMatch) IsLicense() bool {
	return m.MatchType != "Copyright" && !strings.HasPrefix(m.MatchType, "_")
}

// IsCopyright returns true if the match is a copyright header.
func (m LicenseMatch) IsCopyright() bool {
	return m.MatchType == "Copyright"
}

// ClassifiedFile represents the output of the Ingestion Stage (Classifier).
type ClassifiedFile struct {
	Path          string
	ProjectRoot   string
	IsLicenseFile bool
	HasReadme     bool
	IsFirstParty  bool
	AnalyzedText  []byte

	// Matches contains every discrete license or copyright block found in the file.
	Matches []LicenseMatch
}

// HasLicenses returns true if the file is explicitly marked as a primary license file
// or contains at least one non-copyright, non-internal license pattern match.
func (cf ClassifiedFile) HasLicenses() bool {
	if cf.IsLicenseFile {
		return true
	}
	for _, m := range cf.Matches {
		if m.IsLicense() {
			return true
		}
	}
	return false
}

// ComplianceError represents a violation found during the Validation Stage (Policy Engine).
type ComplianceError struct {
	CheckName    string
	LicenseID    string
	Project      string
	FilePath     string
	StartLine    int
	EndLine      int
	Issue        string
	Replacements []string
}

// Discoverer defines the contract for Stage 1: Filesystem Crawler.
type Discoverer interface {
	// Run emits discovered paths into the returned channel.
	Run(ctx context.Context, rootDirs []string) (<-chan RawPath, error)
}

// Grouper defines the contract for Stage 2: Project Boundary.
type Grouper interface {
	// Run consumes RawPaths and emits grouped Projects.
	Run(ctx context.Context, in <-chan RawPath) (<-chan Project, error)
}

// Pruner defines the contract for Stage 3: GN/Bazel build graph filtering.
type Pruner interface {
	// Run cross-references files in Projects against the build graph and emits FilteredProjects.
	Run(ctx context.Context, in <-chan Project) (<-chan FilteredProject, error)
}

// Classifier defines the contract for Stage 4: Worker pool & disk caching for License Classifier.
type Classifier interface {
	// Run reads, normalizes, and classifies the files within FilteredProjects.
	Run(ctx context.Context, in <-chan FilteredProject) (<-chan ClassifiedFile, error)
}

// Validator defines the contract for Stage 5: Policy Engine.
type Validator interface {
	// Run cross-references ClassifiedFiles against allowed policies and emits any errors.
	Run(ctx context.Context, in <-chan ClassifiedFile) (<-chan ComplianceError, error)
}

// Renderer defines the contract for Stage 6: Consuming analyzed projects and compliance errors.
type Renderer interface {
	Run(ctx context.Context, projects []*Project, errors []ComplianceError) error
}

// MultiRenderer executes multiple independent renderers in sequence.
type MultiRenderer []Renderer

func (mr MultiRenderer) Run(ctx context.Context, projects []*Project, errors []ComplianceError) error {
	var firstErr error
	for _, r := range mr {
		if err := r.Run(ctx, projects, errors); err != nil && firstErr == nil {
			firstErr = err
		}
	}
	return firstErr
}

// RenderFunc allows standard functions to satisfy the Renderer interface.
type RenderFunc func(ctx context.Context, projects []*Project, errors []ComplianceError) error

func (f RenderFunc) Run(ctx context.Context, projects []*Project, errors []ComplianceError) error {
	return f(ctx, projects, errors)
}
