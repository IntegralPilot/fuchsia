// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package packages

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math/rand"
	"os"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
)

const (
	staticPackagesPath string = "data/static_packages"
	cachePackagesPath  string = "data/cache_packages.json"
)

type SystemImagePackage struct {
	p        Package
	packages map[string]build.MerkleRoot
}

func newSystemImagePackage(ctx context.Context, p Package) (*SystemImagePackage, error) {
	packages := make(map[string]build.MerkleRoot)

	// Parse the base packages list.
	if f, err := p.Open(ctx, staticPackagesPath); err == nil {
		defer f.Close()

		if contents, err := build.ParseMetaContents(f); err == nil {
			for path, merkle := range contents {
				packages[path] = merkle
			}
		} else {
			return nil, err
		}
	} else {
		return nil, fmt.Errorf("failed to open system image's base packages: %w", err)
	}

	// Parse the cache packages list.
	if b, err := p.ReadFile(ctx, cachePackagesPath); err == nil {
		contents, err := parseCachePackages(b)
		if err != nil {
			return nil, err
		}

		for path, merkle := range contents {
			packages[path] = merkle
		}
	} else if !errors.Is(err, os.ErrNotExist) {
		return nil, fmt.Errorf("failed to open system image's cache packages: %w", err)
	}

	return &SystemImagePackage{
		p:        p,
		packages: packages,
	}, nil
}

func (u *SystemImagePackage) Repository() *Repository {
	return u.p.Repository()
}

func (u *SystemImagePackage) String() string {
	return u.p.String()
}

func (u *SystemImagePackage) Path() string {
	return u.p.Path()
}

func (u *SystemImagePackage) Merkle() build.MerkleRoot {
	return u.p.Merkle()
}

func (u *SystemImagePackage) EditContents(
	ctx context.Context,
	dstSystemImagePath string,
	editFunc func(tempDir string) error,
) (*SystemImagePackage, error) {
	return u.EditPackage(ctx, func(p Package) (Package, error) {
		return p.EditContents(ctx, dstSystemImagePath, editFunc)
	})
}

func (u *SystemImagePackage) EditPackage(
	ctx context.Context,
	editFunc func(p Package) (Package, error),
) (*SystemImagePackage, error) {
	p, err := editFunc(u.p)
	if err != nil {
		return nil, err
	}

	return &SystemImagePackage{
		p:        p,
		packages: u.packages,
	}, nil
}

// SystemImageSize returns the transitive space needed to store all the blobs
// in the system image. It does not include the update image package
// blobs, since those are garbage collected during the OTA.
func (u *SystemImagePackage) SystemImageAlignedBlobSize(ctx context.Context) (uint64, error) {
	blobs, err := u.SystemImageBlobs(ctx)
	if err != nil {
		return 0, err
	}

	return u.p.repo.sumAlignedBlobSizes(ctx, blobs)
}

// SystemImageBlobs returns the transitive blobs in the system image and the
// available set package blobs contained in the system image.
func (u *SystemImagePackage) SystemImageBlobs(ctx context.Context) (map[build.MerkleRoot]struct{}, error) {
	visitedPackages := make(map[build.MerkleRoot]struct{})
	blobs := make(map[build.MerkleRoot]struct{})

	// This will contain all the blobs in the system image packages, and
	// any of its subpackages.
	if err := u.p.transitiveBlobs(ctx, visitedPackages, blobs); err != nil {
		return nil, err
	}

	if err := u.systemImagePackageBlobs(ctx, visitedPackages, blobs); err != nil {
		return nil, err
	}

	return blobs, nil
}

func (u *SystemImagePackage) systemImagePackageBlobs(
	ctx context.Context,
	visitedPackages map[build.MerkleRoot]struct{},
	blobs map[build.MerkleRoot]struct{},
) error {
	// Prefetch blobs in a single batch to speed up download.
	merkles := []build.MerkleRoot{}
	for _, merkle := range u.packages {
		merkles = append(merkles, merkle)
	}

	if err := u.p.repo.PrefetchUncompressedBlobs(ctx, merkles); err != nil {
		return err
	}

	for path, merkle := range u.packages {
		pkg, err := newPackage(ctx, u.p.repo, path, merkle)
		if err != nil {
			return err
		}

		if err := pkg.transitiveBlobs(ctx, visitedPackages, blobs); err != nil {
			return err
		}
	}
	return nil
}

// AddRandomFiles will extend the system image package with extra files that add
// up to `additionalBytes`
// the total size of the system image package blobs and the available set
// package blobs is less than or equal to `maxSize`.
func (u *SystemImagePackage) AddRandomFilesWithAdditionalBytes(
	ctx context.Context,
	rand *rand.Rand,
	dstSystemImagePath string,
	bytesToAdd uint64,
) (*SystemImagePackage, error) {
	return u.EditPackage(ctx, func(p Package) (Package, error) {
		return p.AddRandomFilesWithAdditionalBytes(
			ctx,
			rand,
			dstSystemImagePath,
			bytesToAdd,
		)
	})
}

// AddRandomFilesWithUpperBound will extend the system image package such that
// the total size of the system image package blobs and the available set
// package blobs is less than or equal to `maxSize`.
func (u *SystemImagePackage) AddRandomFilesWithUpperBound(
	ctx context.Context,
	rand *rand.Rand,
	dstSystemImagePath string,
	maxSize uint64,
) (*SystemImagePackage, error) {
	visitedPackages := make(map[build.MerkleRoot]struct{})
	packageBlobs := make(map[build.MerkleRoot]struct{})

	if err := u.systemImagePackageBlobs(ctx, visitedPackages, packageBlobs); err != nil {
		return nil, err
	}

	return u.EditPackage(ctx, func(p Package) (Package, error) {
		return p.addRandomFilesWithUpperBound(
			ctx,
			rand,
			dstSystemImagePath,
			maxSize,
			packageBlobs,
		)
	})
}

func parseCachePackages(b []byte) (map[string]build.MerkleRoot, error) {
	contents := make(map[string]build.MerkleRoot)

	// Treat an empty file as an empty map.
	if len(b) == 0 {
		return contents, nil
	}

	// Otherwise parse the json.
	var cachePackages struct {
		Content []string `json:"content"`
		Version string   `json:"version"`
	}

	if err := json.Unmarshal(b, &cachePackages); err != nil {
		return nil, fmt.Errorf("failed to parse cache packages: %w", err)
	}

	if cachePackages.Version != "1" {
		return nil, fmt.Errorf(
			"do not know how to parse %s version %s",
			cachePackagesPath,
			cachePackages.Version,
		)
	}

	for _, s := range cachePackages.Content {
		url, merkle, err := util.ParsePackageUrl(s)
		if err != nil {
			return nil, fmt.Errorf("failed to parse %s: %w", s, err)
		}

		contents[url.Path[1:]] = merkle
	}

	return contents, nil
}
