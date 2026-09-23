// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"fmt"

	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
)

// PrebuiltVersionsLocation describes the location of the prebuilt_versions
// manifest relative to the build dir.
type PrebuiltVersionsLocation struct {
	Location string `json:"location"`
}

// PrebuiltVersion represents an entry in the prebuilt_versions manifest.
type PrebuiltVersion struct {
	// Name is the name of the prebuilt package.
	Name string `json:"name"`

	// Version is the version of the package used in this build.
	Version string `json:"version"`
}

func GetPackageVersion(prebuiltVersions []PrebuiltVersion, packageName string) (string, error) {
	for _, pkg := range prebuiltVersions {
		if pkg.Name == packageName {
			return pkg.Version, nil
		}
	}
	return "", fmt.Errorf("could not find package %s in prebuilt_versions.json", packageName)
}

// LoadPrebuiltVersions loads the prebuilt_versions.json manifest at the provided path.
func LoadPrebuiltVersions(prebuiltVersionsPath string) ([]PrebuiltVersion, error) {
	var prebuiltVersions []PrebuiltVersion
	err := jsonutil.ReadFromFile(prebuiltVersionsPath, &prebuiltVersions)
	return prebuiltVersions, err
}
