// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package prune

import (
	"context"
	"os"
	"path/filepath"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/metrics"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

// Pruner implements pipeline.Pruner. It filters out projects and files based on
// build graph membership or targeted FastMode criteria.
type Pruner struct {
	// ValidFiles is a set of absolute file paths that are known to be part of the build.
	ValidFiles map[string]bool

	// FilesInReadmeOnly restricts files within each project to only those explicitly declared in
	// README.fuchsia (or explicitly targeted), avoiding classifying untargeted source files.
	FilesInReadmeOnly bool

	// TargetFiles specifies the explicit files or directories being targeted.
	TargetFiles []string

	// FuchsiaDir is the root directory of the Fuchsia repository.
	FuchsiaDir string
}

// NewPruner creates a new stateless project pruner.
func NewPruner(validFiles map[string]bool) *Pruner {
	return &Pruner{
		ValidFiles: validFiles,
	}
}

// Run consumes a stream of Projects, filters projects and files based on target criteria
// and the ValidFiles map, and emits FilteredProjects.
func (p *Pruner) Run(ctx context.Context, in <-chan pipeline.Project) (<-chan pipeline.FilteredProject, error) {
	out := make(chan pipeline.FilteredProject, 100)

	var targetDirs []string
	targetFilesSet := make(map[string]bool)

	// Partition TargetFiles into explicit target directories and individual target files.
	// If a target cannot be stated on disk (e.g. in synthetic test cases or dry-run environments),
	// heuristically infer whether it represents a directory based on whether it has a file extension.
	if len(p.TargetFiles) > 0 {
		for _, tf := range p.TargetFiles {
			cleanTarget := filepath.Clean(tf)
			if !filepath.IsAbs(cleanTarget) && p.FuchsiaDir != "" {
				cleanTarget = filepath.Join(p.FuchsiaDir, cleanTarget)
			}

			if info, err := os.Stat(cleanTarget); err == nil {
				if info.IsDir() {
					targetDirs = append(targetDirs, cleanTarget)
				} else {
					targetFilesSet[cleanTarget] = true
				}
			} else {
				if filepath.Ext(cleanTarget) == "" {
					targetDirs = append(targetDirs, cleanTarget)
				} else {
					targetFilesSet[cleanTarget] = true
				}
			}
		}
	}

	// isTargetFile checks if a file path is explicitly targeted or resides within a targeted directory.
	isTargetFile := func(path string) bool {
		if targetFilesSet[path] {
			return true
		}
		for _, td := range targetDirs {
			if path == td || strings.HasPrefix(path, td+string(filepath.Separator)) {
				return true
			}
		}
		return false
	}

	// isTargetProject checks if a project is relevant to the target files.
	// A project matches if its root path is or contains a target directory/file,
	// or if any of its discovered files match a target.
	isTargetProject := func(proj pipeline.Project) bool {
		if len(p.TargetFiles) == 0 {
			return true
		}
		for _, td := range targetDirs {
			if proj.RootPath == td ||
				strings.HasPrefix(proj.RootPath, td+string(filepath.Separator)) ||
				strings.HasPrefix(td, proj.RootPath+string(filepath.Separator)) {
				return true
			}
		}
		for tf := range targetFilesSet {
			if tf == proj.RootPath || strings.HasPrefix(tf, proj.RootPath+string(filepath.Separator)) {
				return true
			}
		}
		for _, f := range proj.Files {
			if isTargetFile(f.Path) {
				return true
			}
		}
		return false
	}

	go func() {
		defer close(out)
		defer metrics.FilterDuration.Track()()

		for proj := range in {
			if ctx.Err() != nil {
				return
			}

			// Filter out projects that do not intersect with the target files or directories.
			if len(p.TargetFiles) > 0 && !isTargetProject(proj) {
				metrics.ProjectsProcessed.Inc("pruned_by_target")
				continue
			}

			// When FilesInReadmeOnly (FastMode) is enabled, prune untargeted source files from the project.
			// We retain only explicitly targeted files, license/non-license files, and build/package manifests
			// (e.g., README.fuchsia, Cargo.toml, go.mod) needed for verification, bypassing expensive classification
			// of untouched source files.
			if p.FilesInReadmeOnly {
				var keptFiles []pipeline.FileInfo
				for _, f := range proj.Files {
					base := filepath.Base(f.Path)
					isMeta := base == "README.fuchsia" || base == "Cargo.toml" || base == "go.mod" || base == "pubspec.yaml"
					if isTargetFile(f.Path) || f.IsLicenseFile || f.IsNonLicense || isMeta {
						keptFiles = append(keptFiles, f)
					}
				}
				proj.Files = keptFiles
			}

			// If no valid files are provided, we assume we're not pruning anything (e.g., running without a build graph).
			if len(p.ValidFiles) == 0 {
				metrics.ProjectsProcessed.Inc("kept_by_gn")
				select {
				case <-ctx.Done():
					return
				case out <- pipeline.FilteredProject{Project: proj}:
				}
				continue
			}

			// Check if any file in the project is in the build graph
			keep := false
			for _, file := range proj.Files {
				if p.ValidFiles[file.Path] {
					keep = true
					break
				}
			}

			if keep {
				metrics.ProjectsProcessed.Inc("kept_by_gn")
				select {
				case <-ctx.Done():
					return
				case out <- pipeline.FilteredProject{Project: proj}:
				}
			} else {
				metrics.ProjectsProcessed.Inc("pruned_by_gn")
			}
		}
	}()

	return out, nil
}
