// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"testing"
)

func TestGetPackageVersion(t *testing.T) {
	prebuiltVersions := []PrebuiltVersion{
		{
			Name:    "path/to/packageA",
			Version: "versionA",
		}, {
			Name:    "path/to/packageB/${platform}",
			Version: "versionB-x64",
		}, {
			Name:    "path/to/packageB/linux-arm64",
			Version: "versionB-arm64",
		},
	}
	testCases := []struct {
		name            string
		packageName     string
		expectedVersion string
		wantError       bool
	}{
		{
			name:            "packageA",
			packageName:     "path/to/packageA",
			expectedVersion: "versionA",
		}, {
			name:            "packageB for ${platform}",
			packageName:     "path/to/packageB/${platform}",
			expectedVersion: "versionB-x64",
		}, {
			name:            "packageB for arm64",
			packageName:     "path/to/packageB/linux-arm64",
			expectedVersion: "versionB-arm64",
		}, {
			name:        "missing package",
			packageName: "path/to/packageC",
			wantError:   true,
		},
	}
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			version, err := GetPackageVersion(prebuiltVersions, tc.packageName)
			if version != tc.expectedVersion {
				t.Errorf("got %s, want %s", version, tc.expectedVersion)
			}
			if (err == nil) == tc.wantError {
				t.Errorf("got error [%v], want error? %t", err, tc.wantError)
			}
		})
	}
}
