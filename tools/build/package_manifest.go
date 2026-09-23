// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
)

// ErrInvalidMerkleRootLength indicates a MerkleRoot string did not contain 32 hex bytes
var ErrInvalidMerkleRootLength = errors.New("decoded merkle root does not contain 32 bytes")

// MerkleRoot is the root hash of a merkle tree
type MerkleRoot [32]byte

// String encodes a MerkleRoot as a lower case 32 byte hex string
func (m MerkleRoot) String() string {
	return hex.EncodeToString(m[:])
}

// MarshalText implements encoding.TextMarshaler
func (m MerkleRoot) MarshalText() ([]byte, error) {
	return []byte(m.String()), nil
}

// UnmarshalText implements encoding.TextUnmarshaler
func (m *MerkleRoot) UnmarshalText(text []byte) error {
	n, err := hex.Decode(m[:], text)
	if err != nil {
		return err
	} else if n != 32 {
		return ErrInvalidMerkleRootLength
	}
	return nil
}

// Package is a representation of basic package metadata
type Package struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// PackageBlobInfo contains metadata for a single blob in a package
type PackageBlobInfo struct {
	// The path of the blob relative to the output directory
	SourcePath string `json:"source_path"`

	// The path within the package
	Path string `json:"path"`

	// Merkle root for the blob
	Merkle MerkleRoot `json:"merkle"`

	// Size of blob, in bytes
	Size uint64 `json:"size"`
}

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

// PackageManifest is the json structure representation of a full package
// manifest.
type PackageManifest struct {
	Version     string                  `json:"version"`
	Repository  string                  `json:"repository,omitempty"`
	Package     Package                 `json:"package"`
	Blobs       []PackageBlobInfo       `json:"blobs"`
	Subpackages []PackageSubpackageInfo `json:"subpackages,omitempty"`
}

// packageManifestMaybeRelative is the json structure representation of a package
// manifest that may contain file-relative source paths. This is a separate type
// from PackageManifest so we don't need to touch every use of PackageManifest to
// avoid writing invalid blob_sources_relative values to disk.
type packageManifestMaybeRelative struct {
	Version     string                  `json:"version"`
	Repository  string                  `json:"repository,omitempty"`
	Package     Package                 `json:"package"`
	Blobs       []PackageBlobInfo       `json:"blobs"`
	Subpackages []PackageSubpackageInfo `json:"subpackages,omitempty"`
	// TODO(https://fxbug.dev/42066050): rename the json field to `paths_relative` since it
	// applies to both blobs and subpackages.
	PathsRelativeTo string `json:"blob_sources_relative"`
}

// LoadPackageManifest parses the package manifest for a particular package,
// resolving file-relative blob source paths before returning if needed.
func LoadPackageManifest(packageManifestPath string) (*PackageManifest, error) {
	fileContents, err := os.ReadFile(packageManifestPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read %s: %w", packageManifestPath, err)
	}

	rawManifest := &packageManifestMaybeRelative{}
	if err := json.Unmarshal(fileContents, rawManifest); err != nil {
		return nil, fmt.Errorf("failed to unmarshal %s: %w", packageManifestPath, err)
	}

	manifest := &PackageManifest{}
	manifest.Version = rawManifest.Version
	manifest.Repository = rawManifest.Repository
	manifest.Package = rawManifest.Package

	if manifest.Version != "1" {
		return nil, fmt.Errorf("unknown version %q, can't load manifest", manifest.Version)
	}

	// if the manifest has file-relative paths, make them relative to the working directory
	if rawManifest.PathsRelativeTo == "file" {
		basePath := filepath.Dir(packageManifestPath)
		for i := 0; i < len(rawManifest.Blobs); i++ {
			blob := rawManifest.Blobs[i]
			blob.SourcePath = filepath.Join(basePath, blob.SourcePath)
			manifest.Blobs = append(manifest.Blobs, blob)
		}

		for i := 0; i < len(rawManifest.Subpackages); i++ {
			subpackage := rawManifest.Subpackages[i]
			subpackage.ManifestPath = filepath.Join(basePath, subpackage.ManifestPath)
			manifest.Subpackages = append(manifest.Subpackages, subpackage)
		}
	} else {
		manifest.Blobs = rawManifest.Blobs
		manifest.Subpackages = rawManifest.Subpackages
	}

	return manifest, nil
}
