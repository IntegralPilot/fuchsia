// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/google/go-cmp/cmp"
)

// MustDecodeMerkleRoot parses a MerkleRoot from a string, or panics
func MustDecodeMerkleRoot(s string) MerkleRoot {
	m, err := DecodeMerkleRoot([]byte(s))
	if err != nil {
		panic(err)
	}
	return m
}

// DecodeMerkleRoot attempts to parse a MerkleRoot from a string
func DecodeMerkleRoot(text []byte) (MerkleRoot, error) {
	var m MerkleRoot
	if err := m.UnmarshalText(text); err != nil {
		return m, err
	}
	return m, nil
}

// Verifies package manifests can be properly loaded.
func TestLoadPackageManifest(t *testing.T) {
	testCases := []struct {
		name               string
		buildDirContents   map[string]string
		manifestPathToLoad string
		expectedManifest   PackageManifest
		wantError          bool
	}{
		{
			name: "success valid path, blobs, and subpackages",
			buildDirContents: map[string]string{
				"package_manifest.json": `{
					"version": "1",
					"blobs": [
						{ "merkle": "0000000000000000000000000000000000000000000000000000000000000000" }
					],
					"subpackages": [
						{
							"name": "some_subpackage",
							"merkle": "1111111111111111111111111111111111111111111111111111111111111111",
							"manifest_path": "subpackage_manifest.json"
						}
					]
				}`,
			},
			manifestPathToLoad: "package_manifest.json",
			expectedManifest: PackageManifest{
				Version: "1",
				Blobs: []PackageBlobInfo{
					{Merkle: MustDecodeMerkleRoot("0000000000000000000000000000000000000000000000000000000000000000")},
				},
				Subpackages: []PackageSubpackageInfo{
					{
						Name:         "some_subpackage",
						Merkle:       MustDecodeMerkleRoot("1111111111111111111111111111111111111111111111111111111111111111"),
						ManifestPath: "subpackage_manifest.json",
					},
				},
			},
		},
		{
			name: "failure incompatible version",
			buildDirContents: map[string]string{
				"package_manifest.json": `{
					"version": "2",
				}`,
			},
			manifestPathToLoad: "package_manifest.json",
			wantError:          true,
		},
		{
			name: "failure json improperly formatted",
			buildDirContents: map[string]string{
				"package_manifest.json": `{
					Oops. This is not valid json.
				}`,
			},
			manifestPathToLoad: "package_manifest.json",
			wantError:          true,
		}, {
			name:               "failure package manifest does not exist",
			manifestPathToLoad: "non_existent_manifest.json",
			wantError:          true,
		},
	}
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			// Generate test env based on input.
			tempDirPath := createBuildDir(t, tc.buildDirContents)

			// Now that we're set up, we can actually load the package manifest.
			actualManifest, err := LoadPackageManifest(filepath.Join(tempDirPath, tc.manifestPathToLoad))

			// Ensure the results match the expectations.
			if (err == nil) == tc.wantError {
				t.Fatalf("got error [%v], want error? %t", err, tc.wantError)
			}
			if diff := cmp.Diff(actualManifest, &tc.expectedManifest); err == nil && diff != "" {
				t.Fatalf("got manifest %#v, expected %#v", actualManifest, tc.expectedManifest)
			}
		})
	}
}

func createBuildDir(t *testing.T, files map[string]string) string {
	dir := t.TempDir()
	for filename, data := range files {
		if err := os.MkdirAll(filepath.Join(dir, filepath.Dir(filename)), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, filename), []byte(data), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}
