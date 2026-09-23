// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/BurntSushi/toml"
)

type cargoToml struct {
	Package   *cargoPackage   `toml:"package"`
	Workspace *cargoWorkspace `toml:"workspace"`
}

type cargoPackage struct {
	Name        string `toml:"name"`
	Version     string `toml:"version"`
	Description string `toml:"description"`
	License     string `toml:"license"`
	Repository  string `toml:"repository"`
	Homepage    string `toml:"homepage"`
}

type cargoWorkspace struct {
	Members []string      `toml:"members"`
	Package *cargoPackage `toml:"package"`
}

// ParseCargoToml reads a Cargo.toml file and returns a slice of synthetic Readme structs.
func ParseCargoToml(path string) ([]*Readme, error) {
	bytes, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	var cargo cargoToml
	if _, err := toml.Decode(string(bytes), &cargo); err != nil {
		return nil, err
	}

	var readmes []*Readme

	// 1. Handle standalone package
	if cargo.Package != nil {
		if cargo.Package.Name == "fuchsia-third-party" {
			// Top-level container manifest for third_party/rust_crates, not a 3P crate.
			return nil, nil
		}
		url := cargo.Package.Repository
		if url == "" {
			url = cargo.Package.Homepage
		}
		if url == "" && cargo.Package.Name != "" {
			url = "https://crates.io/crates/" + cargo.Package.Name
		}

		readmes = append(readmes, &Readme{
			Name:    cargo.Package.Name,
			Version: cargo.Package.Version,
			URL:     url,
			// Standalone Cargo.toml describes the project at its own directory
			Location: ".",
		})
	} else if cargo.Workspace != nil && strings.Contains(path, "third_party/rust_crates/mirrors/") {
		// Special case for mirrors: workspace without package (e.g. google-cloud-rust)
		readmes = append(readmes, &Readme{
			Name:     filepath.Base(filepath.Dir(path)), // Use folder name as project name
			Location: ".",
		})
	}

	// 2. Handle workspace members
	// Default behavior: add members as sub-projects.
	// For mirrors: skip members to keep them grouped under the workspace project.
	if cargo.Workspace != nil && !strings.Contains(path, "third_party/rust_crates/mirrors/") {
		for _, member := range cargo.Workspace.Members {
			// We only handle simple member paths for now.
			// Full glob support would requires more logic.
			if !filepath.IsAbs(member) {
				readmes = append(readmes, &Readme{
					Name:     filepath.Base(member),
					Location: member,
				})
			}
		}
	}

	return readmes, nil
}
