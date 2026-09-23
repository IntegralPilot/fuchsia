// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"io/fs"
	"path/filepath"
	"sort"
)

// ProjectInfo contains basic information about a discovered project.
type ProjectInfo struct {
	Path string
	Name string
}

// DiscoverProjects walks down the directory tree from rootDir and returns
// all projects found, respecting skip configs and all boundary file types.
func DiscoverProjects(rootDir, fuchsiaDir string, config Config) ([]ProjectInfo, error) {
	var projects []ProjectInfo

	absRoot, err := filepath.Abs(rootDir)
	if err != nil {
		return nil, err
	}

	err = filepath.WalkDir(absRoot, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil // Continue on error
		}

		if config != nil && config.IsSkipped(path) {
			if d.IsDir() {
				return filepath.SkipDir
			}
			return nil
		}

		if d.IsDir() {
			var outOfTree map[string]string
			if config != nil {
				outOfTree = config.OutOfTreeReadmes()
			}
			// Check if this directory is a project boundary!
			isBoundary, bestPath, allReadmes, err := IsProjectBoundary(path, fuchsiaDir, outOfTree)
			if err != nil {
				// Log or ignore? Let's ignore for now to match other commands.
				return nil
			}

			if isBoundary && len(allReadmes) > 0 {
				// We found a project!
				// Compute logical path relative to fuchsiaDir
				logicalDir := filepath.Dir(bestPath)
				for logPath, physPath := range outOfTree {
					if physPath == bestPath {
						logicalDir = filepath.Join(fuchsiaDir, logPath)
						break
					}
				}

				for _, r := range allReadmes {
					rel := logicalDir
					if r.Location != "" && r.Location != "." {
						rel = filepath.Join(rel, r.Location)
					}
					relPath, _ := filepath.Rel(fuchsiaDir, rel)

					name := r.Name
					if name == "" {
						name = "Unknown Project"
					}

					projects = append(projects, ProjectInfo{
						Path: relPath,
						Name: name,
					})
				}
			}
		}

		return nil
	})

	if err != nil {
		return nil, err
	}

	// Sort projects by path for consistent output!
	sort.Slice(projects, func(i, j int) bool {
		return projects[i].Path < projects[j].Path
	})

	return projects, nil
}
