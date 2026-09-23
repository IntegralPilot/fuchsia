// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package packages

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math/rand"
	"os"
	"path/filepath"

	far "go.fuchsia.dev/fuchsia/src/sys/pkg/lib/far/go"
	"go.fuchsia.dev/fuchsia/src/sys/pkg/lib/merkle"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

const (
	BlobBlockSize   = 8192
	maxBlobFileSize = BlobBlockSize * 1000
)

type FileData []byte

type Package struct {
	merkle      build.MerkleRoot
	repo        *Repository
	path        string
	contents    build.MetaContents
	subpackages map[string]build.MerkleRoot
}

// newPackage extracts out a package from the repository.
func newPackage(
	ctx context.Context,
	repo *Repository,
	path string,
	merkle build.MerkleRoot,
) (Package, error) {
	// Need to parse out the package meta.far to find the package contents.
	blob, err := repo.OpenUncompressedBlob(ctx, merkle)
	if err != nil {
		return Package{}, err
	}
	defer blob.Close()

	f, err := far.NewReader(blob)
	if err != nil {
		return Package{}, err
	}
	defer f.Close()

	b, err := f.ReadFile("meta/contents")
	if err != nil {
		return Package{}, err
	}

	contents, err := build.ParseMetaContents(bytes.NewReader(b))
	if err != nil {
		return Package{}, err
	}

	subpackages := make(map[string]build.MerkleRoot)
	if b, err := f.ReadFile("meta/fuchsia.pkg/subpackages"); err == nil {
		var metaSubpackages *build.MetaSubpackages
		if err := json.Unmarshal(b, &metaSubpackages); err != nil {
			return Package{}, fmt.Errorf("Unable to parse subpackage for package %s: %w", merkle, err)
		}

		for path, merkleString := range metaSubpackages.Subpackages {
			merkle, err := build.DecodeMerkleRoot([]byte(merkleString))
			if err != nil {
				return Package{}, err
			}

			subpackages[path] = merkle
		}
	}

	return Package{
		merkle:      merkle,
		repo:        repo,
		path:        path,
		contents:    contents,
		subpackages: subpackages,
	}, nil
}

// Repository returns the repository that contains this package.
func (p *Package) Repository() *Repository {
	return p.repo
}

func (p *Package) String() string {
	return fmt.Sprintf("%s @ %s", p.Path(), p.Merkle())
}

// Path returns the package path.
func (p *Package) Path() string {
	return p.path
}

// Merkle returns the meta.far merkle.
func (p *Package) Merkle() build.MerkleRoot {
	return p.merkle
}

// Returns the package's blobs.
func (p *Package) Blobs() map[build.MerkleRoot]struct{} {
	blobs := make(map[build.MerkleRoot]struct{})
	blobs[p.merkle] = struct{}{}

	for _, merkle := range p.contents {
		blobs[merkle] = struct{}{}
	}

	return blobs
}

// Returns the package's subpackages.
func (p *Package) Subpackages() map[string]build.MerkleRoot {
	subpackages := make(map[string]build.MerkleRoot)
	for path, merkle := range p.subpackages {
		subpackages[path] = merkle
	}

	return subpackages
}

// Open opens a file in the package.
func (p *Package) Open(ctx context.Context, path string) (*os.File, error) {
	merkle, ok := p.contents[path]
	if !ok {
		return nil, os.ErrNotExist
	}

	return p.repo.OpenUncompressedBlob(ctx, merkle)
}

func (p *Package) FilePath(ctx context.Context, path string) (string, error) {
	merkle, ok := p.contents[path]
	if !ok {
		return "", os.ErrNotExist
	}

	err := p.repo.PrefetchUncompressedBlobs(ctx, []build.MerkleRoot{merkle})
	if err != nil {
		return "", fmt.Errorf("failed to make uncompressed blob available for %s: %w", path, err)
	}

	return p.repo.UncompressedBlobPath(ctx, merkle)
}

// ReadFile reads a file from a package.
func (p *Package) ReadFile(ctx context.Context, path string) ([]byte, error) {
	r, err := p.Open(ctx, path)
	if err != nil {
		return nil, err
	}

	return io.ReadAll(r)
}

func (p *Package) Expand(ctx context.Context, dir string) error {
	// Prefetch all the package blobs, so we can download them in a single batch.
	merkles := []build.MerkleRoot{}
	for _, merkle := range p.contents {
		merkles = append(merkles, merkle)
	}

	if err := p.repo.PrefetchUncompressedBlobs(ctx, merkles); err != nil {
		return err
	}

	for path := range p.contents {
		data, err := p.ReadFile(ctx, path)
		if err != nil {
			return fmt.Errorf("invalid path. %w", err)
		}
		realPath := filepath.Join(dir, path)
		if err := os.MkdirAll(filepath.Dir(realPath), 0755); err != nil {
			return fmt.Errorf("could not create parent directories for %s, %w", realPath, err)
		}
		if err = os.WriteFile(realPath, data, 0644); err != nil {
			return fmt.Errorf("could not export %s to %s. %w", path, realPath, err)
		}
	}
	blob, err := p.repo.OpenUncompressedBlob(ctx, p.merkle)
	if err != nil {
		return fmt.Errorf("failed to open meta.far blob. %w", err)
	}
	defer blob.Close()

	f, err := far.NewReader(blob)
	if err != nil {
		return fmt.Errorf("failed to open reader on blob. %w", err)
	}
	defer f.Close()

	for _, path := range f.List() {
		data, err := f.ReadFile(path)
		if err != nil {
			fmt.Errorf("failed to read %s. %w", path, err)
		}
		realPath := filepath.Join(dir, path)
		if err := os.MkdirAll(filepath.Dir(realPath), 0755); err != nil {
			return fmt.Errorf("could not create parent directories for %s, %w", realPath, err)
		}
		if err = os.WriteFile(realPath, data, 0644); err != nil {
			fmt.Errorf("failed to write file %s. %w", realPath, err)
		}
	}

	return nil
}

func (p *Package) EditContents(
	ctx context.Context,
	dstPath string,
	editFunc func(tempDir string) error,
) (Package, error) {
	return p.repo.EditPackage(ctx, *p, dstPath, editFunc)
}

// TransitiveBlobs returns all the blobs in this package and subpackages.
func (p *Package) TransitiveBlobs(ctx context.Context) (map[build.MerkleRoot]struct{}, error) {
	visitedPackages := make(map[build.MerkleRoot]struct{})
	blobs := make(map[build.MerkleRoot]struct{})

	if err := p.transitiveBlobs(ctx, visitedPackages, blobs); err != nil {
		return nil, err
	}

	return blobs, nil
}

func (p *Package) transitiveBlobs(
	ctx context.Context,
	visitedPackages map[build.MerkleRoot]struct{},
	blobs map[build.MerkleRoot]struct{},
) error {
	// Exit early if we've already processed this package.
	if _, ok := visitedPackages[p.merkle]; ok {
		return nil
	}
	visitedPackages[p.merkle] = struct{}{}

	for blob := range p.Blobs() {
		blobs[blob] = struct{}{}
	}

	// Recurse into any subpackages.
	for path, merkle := range p.Subpackages() {
		pkg, err := newPackage(ctx, p.repo, path, merkle)
		if err != nil {
			return err
		}

		if err := pkg.transitiveBlobs(ctx, visitedPackages, blobs); err != nil {
			return err
		}
	}

	return nil
}

// TransitiveBlobSize computes total size of all the blobs in this package and subpackages.
func (p *Package) TransitiveBlobSize(ctx context.Context) (uint64, error) {
	blobs, err := p.TransitiveBlobs(ctx)
	if err != nil {
		return 0, err
	}

	return p.repo.sumAlignedBlobSizes(ctx, blobs)
}

// AddRandomFilesWithAdditionalBytes will add random files to this package that
// will increase the package size with `bytesToAdd` additional bytes.
func (p *Package) AddRandomFilesWithAdditionalBytes(
	ctx context.Context,
	rand *rand.Rand,
	dstPath string,
	bytesToAdd uint64,
) (Package, error) {
	logger.Infof(ctx, "Adding %d bytes to package %s", bytesToAdd, p.path)

	return p.EditContents(ctx, dstPath, func(tempDir string) error {
		otaTestDir := filepath.Join(tempDir, "ota-test")
		if err := os.Mkdir(otaTestDir, 0700); err != nil {
			return fmt.Errorf("failed to create %s: %w", otaTestDir, err)
		}

		for bytesToAdd > 0 {
			// Create blobs up to the max blob size.
			var blobSize uint64
			if maxBlobFileSize < bytesToAdd {
				blobSize = maxBlobFileSize
			} else {
				blobSize = bytesToAdd
			}

			if err := generateRandomFile(ctx, rand, otaTestDir, blobSize); err != nil {
				return err
			}
			bytesToAdd -= blobSize
		}

		return nil
	})
}

// AddRandomFilesWithUpperBound will add random files to this package that
// increases the package size up to `maxSize` bytes.
func (p *Package) AddRandomFilesWithUpperBound(
	ctx context.Context,
	rand *rand.Rand,
	dstPath string,
	maxSize uint64,
) (Package, error) {
	return p.addRandomFilesWithUpperBound(
		ctx,
		rand,
		dstPath,
		maxSize,
		map[build.MerkleRoot]struct{}{},
	)
}

func (p *Package) addRandomFilesWithUpperBound(
	ctx context.Context,
	rand *rand.Rand,
	dstPath string,
	maxSize uint64,
	additionalBlobs map[build.MerkleRoot]struct{},
) (Package, error) {
	// Merge the package blobs with the additional blobs to compute the
	// initial size.
	packageBlobs, err := p.TransitiveBlobs(ctx)
	if err != nil {
		return Package{}, err
	}

	initialBlobs := make(map[build.MerkleRoot]struct{})
	for blob := range packageBlobs {
		initialBlobs[blob] = struct{}{}
	}

	for blob := range additionalBlobs {
		initialBlobs[blob] = struct{}{}
	}

	initialSize, err := p.repo.sumAlignedBlobSizes(ctx, initialBlobs)
	if err != nil {
		return Package{}, err
	}

	logger.Infof(
		ctx,
		"Trying to grow package %s with %d blobs and size %d to up to %d bytes",
		p.path,
		len(initialBlobs),
		initialSize,
		maxSize,
	)

	if maxSize < initialSize {
		return Package{}, fmt.Errorf(
			"initial package %s with size %d is greater than the max size %d",
			p.path,
			initialSize,
			maxSize,
		)
	}

	bytesToAdd := maxSize - initialSize
	for bytesToAdd != 0 {
		dstPackage, err := p.AddRandomFilesWithAdditionalBytes(
			ctx,
			rand,
			dstPath,
			bytesToAdd,
		)
		if err != nil {
			return Package{}, fmt.Errorf(
				"failed to create the package %s: %w",
				dstPath,
				err,
			)
		}

		// Find all the blobs we just added to the package.
		blobs, err := dstPackage.TransitiveBlobs(ctx)
		if err != nil {
			return Package{}, err
		}

		// Merge the package blobs with our initial blob set, since there could
		// be external blobs we want to include, such as from the system image
		// packages or the update images.
		for blob := range additionalBlobs {
			blobs[blob] = struct{}{}
		}

		size, err := p.repo.sumAlignedBlobSizes(ctx, blobs)
		if err != nil {
			return Package{}, err
		}

		// Otherwise reduce the amount of bytes we're trying to add, and try
		// again.
		logger.Infof(
			ctx,
			"Generated package %s by adding %d bytes, produced package size %d",
			dstPackage.Path(),
			bytesToAdd,
			size,
		)

		if size <= maxSize {
			logger.Infof(
				ctx,
				"Accepting package %s with size %d is smaller than %d",
				dstPackage.Path(),
				size,
				maxSize,
			)
			return dstPackage, nil
		}

		// Otherwise we overshot our target.
		delta := size - maxSize
		logger.Warningf(
			ctx,
			"Generated package %s overshot %d by %d.",
			dstPackage.Path(),
			maxSize,
			delta,
		)

		// Error out if we failed to add a single byte.
		if bytesToAdd == 1 {
			break
		}

		// Otherwise, subtract some bytes from the amount we're trying to add to
		// the package.
		if delta < bytesToAdd {
			bytesToAdd -= delta
		} else {
			// If our delta is larger than the amount of bytes we're trying to
			// add, check if just adding a single byte is sufficient.
			bytesToAdd = 1
		}
	}

	return Package{}, fmt.Errorf(
		"failed to generate package %s with size less than %d",
		dstPath,
		maxSize,
	)
}

func generateRandomFile(
	ctx context.Context,
	rand *rand.Rand,
	dir string,
	blobSize uint64,
) error {
	b := make([]byte, blobSize)

	if _, err := rand.Read(b); err != nil {
		return fmt.Errorf("failed to read %d random bytes: %w", blobSize, err)
	}

	var tree merkle.Tree
	if _, err := tree.ReadFrom(bytes.NewReader(b)); err != nil {
		return fmt.Errorf("failed to compute merkle: %w", err)
	}
	merkle := hex.EncodeToString(tree.Root())

	blobPath := filepath.Join(dir, merkle)
	if err := os.WriteFile(blobPath, b, 0600); err != nil {
		return fmt.Errorf("failed to write %s: %w", blobPath, err)
	}

	logger.Infof(ctx, "generated blob %s with %d size", merkle, blobSize)

	return nil
}
