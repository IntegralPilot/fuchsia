// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package boundary

import (
	"path/filepath"
	"strings"
)

// Config holds the configuration for the boundary resolution stage.
type Config struct {
	// BarrierPaths are repository paths where project boundaries must stop.
	BarrierPaths map[string]bool

	// BarrierExceptions are repository paths under a barrier that should not be treated as project boundaries.
	BarrierExceptions map[string]bool

	// OutOfTreeReadmes maps logical repository paths to out-of-tree physical README.fuchsia files.
	OutOfTreeReadmes map[string]string

	// FilesInReadmeOnly restricts grouped files to only those listed in a README.fuchsia.
	FilesInReadmeOnly bool

	// ManifestProjectNames maps a project's filesystem path to its name in the manifest.
	ManifestProjectNames map[string]string

	// ManifestPrivateProjects tracks if a project path was found in a private manifest.
	ManifestPrivateProjects map[string]bool
}

// NewConfig initializes an empty boundary configuration.
func NewConfig() Config {
	return Config{
		BarrierPaths:            make(map[string]bool),
		BarrierExceptions:       make(map[string]bool),
		OutOfTreeReadmes:        make(map[string]string),
		ManifestProjectNames:    make(map[string]string),
		ManifestPrivateProjects: make(map[string]bool),
	}
}

// IsPrivateProject returns true if the project path belongs to a proprietary/private repository.
func (c *Config) IsPrivateProject(projectPath string) bool {
	if c == nil {
		return false
	}
	projectPath = filepath.Clean(projectPath)
	slashPath := filepath.ToSlash(projectPath)

	parts := strings.Split(strings.TrimPrefix(slashPath, "/"), "/")
	if len(parts) > 0 && parts[0] == "vendor" {
		return true
	}

	for i := len(parts); i > 0; i-- {
		p := strings.Join(parts[:i], "/")
		if c.ManifestPrivateProjects[p] {
			return true
		}
		if name, ok := c.ManifestProjectNames[p]; ok {
			if strings.HasPrefix(name, "fuchsia_internal/") || strings.HasPrefix(name, "vendor/") {
				return true
			}
		}
	}
	return false
}

// ManifestNameFor returns the manifest package name for a given project path.
func (c *Config) ManifestNameFor(projectPath string) string {
	if c == nil {
		return ""
	}
	projectPath = filepath.Clean(projectPath)
	slashPath := filepath.ToSlash(projectPath)
	parts := strings.Split(strings.TrimPrefix(slashPath, "/"), "/")
	for i := len(parts); i > 0; i-- {
		p := strings.Join(parts[:i], "/")
		if name, ok := c.ManifestProjectNames[p]; ok {
			return name
		}
	}
	return ""
}
