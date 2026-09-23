// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package packages

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path"
	"path/filepath"
	"strings"

	versionHistory "go.fuchsia.dev/fuchsia/src/lib/versioning/version-history/go"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/pkg"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

type PackageBuilder struct {
	Name       string
	Repository string
	Version    string
	Cache      string
	Contents   map[string]string
}

func parsePackageJSON(path string) (string, string, error) {
	jsonData, err := os.ReadFile(path)
	if err != nil {
		return "", "", fmt.Errorf("failed to read file at %s: %w", path, err)
	}
	var packageInfo pkg.Package
	if err := json.Unmarshal(jsonData, &packageInfo); err != nil {
		return "", "", fmt.Errorf("failed to unmarshal json data: %w", err)
	}
	return packageInfo.Name, packageInfo.Version, nil
}

// NewPackageBuilder returns a PackageBuilder
// Must call `Close()` to clean up PackageBuilder
func NewPackageBuilder(name string, version string, repository string) (*PackageBuilder, error) {
	if name == "" || version == "" {
		return nil, fmt.Errorf("missing package info and version information")
	}

	// Create temporary directory to store any additions that come in.
	tempDir, err := os.MkdirTemp("", "pm-temp-resource")
	if err != nil {
		return nil, fmt.Errorf("failed to create temp directory: %w", err)
	}

	return &PackageBuilder{
		Name:       name,
		Repository: repository,
		Version:    version,
		Cache:      tempDir,
		Contents:   make(map[string]string),
	}, nil
}

// NewPackageBuilderFromDir returns a PackageBuilder that initializes from the `dir` package directory.
// Must call `Close()` to clean up PackageBuilder
func NewPackageBuilderFromDir(dir string, name string, version string, repository string) (*PackageBuilder, error) {
	pkg, err := NewPackageBuilder(name, version, repository)
	if err != nil {
		return nil, err
	}

	err = filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return fmt.Errorf("walk of %s failed: %w", dir, err)
		}
		if !info.IsDir() {
			relativePath := strings.Replace(path, dir+"/", "", 1)
			pkg.Contents[relativePath] = path
		}
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("error when walking the directory: %w", err)
	}

	return pkg, nil
}

// Close removes temporary directories created by PackageBuilder.
func (p *PackageBuilder) Close() {
	os.RemoveAll(p.Cache)
}

// AddResource adds a resource to the package at the given path.
func (p *PackageBuilder) AddResource(path string, contents io.Reader) error {
	if _, ok := p.Contents[path]; ok {
		return fmt.Errorf("a resource already exists at path %q", path)
	}
	data, err := io.ReadAll(contents)
	if err != nil {
		return fmt.Errorf("failed to read file: %w", err)
	}
	tempPath := filepath.Join(p.Cache, path)
	if err := os.MkdirAll(filepath.Dir(tempPath), os.ModePerm); err != nil {
		return fmt.Errorf("failed to create parent directories for %q: %w", tempPath, err)
	}
	if err = os.WriteFile(tempPath, data, 0644); err != nil {
		return fmt.Errorf("failed to write data to %q: %w", tempPath, err)
	}
	p.Contents[path] = tempPath
	return nil
}

func tempConfig(dir string, name string, version string, repository string) (*build.Config, error) {

	cfg := &build.Config{
		OutputDir:      filepath.Join(dir, "output"),
		ManifestPath:   filepath.Join(dir, "manifest"),
		KeyPath:        filepath.Join(dir, "key"),
		TempDir:        filepath.Join(dir, "tmp"),
		PkgName:        name,
		PkgVersion:     version,
		PkgRepository:  repository,
		PkgABIRevision: versionHistory.History().ExampleSupportedAbiRevisionForTests(),
	}

	for _, d := range []string{cfg.OutputDir, cfg.TempDir} {
		if err := os.MkdirAll(d, os.ModePerm); err != nil {
			return nil, err
		}
	}

	return cfg, nil
}

// Publish the package to the repository. Returns the TUF package path and
// merkle on success, or a error on failure.
func (p *PackageBuilder) Publish(ctx context.Context, pkgRepo *Repository) (Package, error) {
	// Create Config.
	dir, err := os.MkdirTemp("", "pm-temp-config")
	if err != nil {
		return Package{}, fmt.Errorf("failed to create temp directory for the config: %w", err)
	}
	defer os.RemoveAll(dir)

	cfg, err := tempConfig(dir, p.Name, p.Version, p.Repository)
	if err != nil {
		return Package{}, fmt.Errorf("failed to create temp config to fill with our data: %w", err)
	}

	pkgManifestPath := filepath.Join(filepath.Dir(cfg.ManifestPath), "package")
	if err := os.MkdirAll(filepath.Join(pkgManifestPath, "meta"), os.ModePerm); err != nil {
		return Package{}, fmt.Errorf("failed to make parent dirs for meta/package: %w", err)
	}
	mfst, err := os.Create(cfg.ManifestPath)
	if err != nil {
		return Package{}, fmt.Errorf("failed to create package manifest path: %w", err)
	}
	defer mfst.Close()

	// Fill config with our contents.
	for relativePath, sourcePath := range p.Contents {
		if relativePath == "meta/contents" {
			continue
		}
		if _, err := fmt.Fprintf(mfst, "%s=%s\n", relativePath, sourcePath); err != nil {
			return Package{}, fmt.Errorf("failed to record entry %q as %q into manifest: %w", p.Name, sourcePath, err)
		}
	}

	manifest, err := cfg.Manifest()
	if err != nil {
		return Package{}, fmt.Errorf("failed to create mainfest: %w", err)
	}

	abiPath, ok := manifest.Meta()["meta/fuchsia.abi/abi-revision"]
	if ok {
		abiRevision, err := build.ReadABIRevisionFromFile(abiPath)
		if err != nil {
			return Package{}, fmt.Errorf("failed to get abi revision: %w", err)
		}
		cfg.PkgABIRevision = *abiRevision
	}

	// Save changes to config.
	if err := build.Update(cfg); err != nil {
		return Package{}, fmt.Errorf("failed to update config: %w", err)
	}
	if _, err := build.Seal(cfg); err != nil {
		return Package{}, fmt.Errorf("failed to seal config: %w", err)
	}

	pkgPath := fmt.Sprintf("%s/%s", p.Name, p.Version)
	blobs, err := cfg.BlobInfo()
	if err != nil {
		return Package{}, fmt.Errorf("failed to extract blobs: %w", err)
	}

	foundPkgMerkle := false
	pkgMerkle := build.MerkleRoot{}
	for _, blob := range blobs {
		if blob.Path == "meta/" {
			foundPkgMerkle = true
			pkgMerkle = blob.Merkle
			break
		}
	}

	if !foundPkgMerkle {
		return Package{}, fmt.Errorf("could not find meta.far merkle")
	}

	logger.Infof(ctx, "publishing %q to merkle %q", pkgPath, pkgMerkle)

	outputManifest, err := cfg.OutputManifest()
	if err != nil {
		return Package{}, fmt.Errorf("failed to output manifest: %w", err)
	}

	content, err := json.Marshal(outputManifest)
	if err != nil {
		return Package{}, fmt.Errorf("failed to convert manifest to JSON: %w", err)
	}

	outputManifestPath := path.Join(cfg.OutputDir, "package_manifest.json")
	if err := os.WriteFile(outputManifestPath, content, os.ModePerm); err != nil {
		return Package{}, fmt.Errorf("failed to write manifest JSON to %q: %w", outputManifestPath, err)
	}

	// Publish new config to repo.
	err = pkgRepo.Publish(ctx, outputManifestPath)
	if err != nil {
		return Package{}, fmt.Errorf("failed to publish manifest: %w", err)
	}
	logger.Infof(ctx, "package %q as %q published and committed", pkgPath, pkgMerkle)

	return newPackage(ctx, pkgRepo, pkgPath, pkgMerkle)
}
