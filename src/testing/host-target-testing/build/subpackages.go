// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"encoding/json"
	"fmt"
	"os"
)

// PackageSubpackageInfo contains metadata for a single subpackage in a package
type PackageSubpackageInfo struct {
	// The path of the subpackage relative to the output directory
	Name string `json:"name"`

	// Merkle root for the subpackage
	Merkle MerkleRoot `json:"merkle"`

	// Path to the package_manifest.json for the subpackage,
	// relative to the output directory
	ManifestPath string `json:"manifest_path"`
}

// LoadSubpackages attempts to read and parse a subpackages manifest from the
// given path.
func LoadSubpackages(path string) ([]PackageSubpackageInfo, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	var members []PackageSubpackageInfo

	err = json.Unmarshal(data, &members)
	if err != nil {
		return nil, err
	}

	packagePaths := make(map[string]struct{})
	for _, subpackage := range members {
		if _, ok := packagePaths[subpackage.ManifestPath]; ok {
			return nil, fmt.Errorf("%q contains more than one entry with path = %q", path, subpackage.ManifestPath)
		}
		packagePaths[subpackage.ManifestPath] = struct{}{}
	}

	return members, nil
}
