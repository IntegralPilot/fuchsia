// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"path/filepath"

	"go.fuchsia.dev/fuchsia/tools/readme_fuchsia"
)

// ParseAnyMetadata parses a metadata file based on its filename (README.fuchsia, go.mod, Cargo.toml, pubspec.yaml)
// and returns the root Readme (if any) and a slice of sub-project Readmes.
// For go.mod, there is no root readme, only sub-projects.
func ParseAnyMetadata(path string) (rootReadmes []*readme_fuchsia.Readme, subReadmes []*readme_fuchsia.Readme, err error) {
	base := filepath.Base(path)
	var readmes []*readme_fuchsia.Readme

	if base == "go.mod" {
		readmes, err = ParseGoMod(path)
	} else if base == "Cargo.toml" {
		readmes, err = ParseCargoToml(path)
	} else if base == "pubspec.yaml" {
		readmes, err = ParsePubspecYaml(path)
	} else {
		readmes, err = readme_fuchsia.ParseFile(path)
	}

	if err != nil || len(readmes) == 0 {
		return nil, nil, err
	}

	if base != "go.mod" {
		rootReadmes = append(rootReadmes, readmes[0])
	}

	startIdx := 1
	if base == "go.mod" || (base == "Cargo.toml" && readmes[0].Location != ".") {
		startIdx = 0
	}

	if startIdx < len(readmes) {
		subReadmes = append(subReadmes, readmes[startIdx:]...)
	}

	return rootReadmes, subReadmes, nil
}

func ParseFile(path string) ([]*Readme, error) {
	return readme_fuchsia.ParseFile(path)
}

func Parse(data []byte) ([]*Readme, error) {
	return readme_fuchsia.Parse(data)
}
