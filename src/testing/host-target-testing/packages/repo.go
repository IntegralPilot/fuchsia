// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package packages

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
)

type BlobStore interface {
	Dir() string

	// FetchBlobs will download the listed blobs.
	PrefetchBlobs(ctx context.Context, deliveryBlobType *int, merkles []build.MerkleRoot) error

	BlobPath(ctx context.Context, deliveryBlobType *int, merkle build.MerkleRoot) (string, error)

	OpenBlob(ctx context.Context, deliveryBlobType *int, merkle build.MerkleRoot) (*os.File, error)

	BlobSize(ctx context.Context, deliveryBlobType *int, merkle build.MerkleRoot) (uint64, error)
}

type DirBlobStore struct {
	dir string
}

func NewDirBlobStore(dir string) BlobStore {
	return &DirBlobStore{dir}
}

func (fs *DirBlobStore) PrefetchBlobs(
	ctx context.Context,
	deliveryBlobType *int,
	merkles []build.MerkleRoot,
) error {
	return nil
}

func (fs *DirBlobStore) BlobPath(ctx context.Context, deliveryBlobType *int, merkle build.MerkleRoot) (string, error) {
	if deliveryBlobType == nil {
		return filepath.Join(fs.dir, merkle.String()), nil
	} else {
		return filepath.Join(fs.dir, strconv.Itoa(*deliveryBlobType), merkle.String()), nil
	}
}

func (fs *DirBlobStore) OpenBlob(ctx context.Context, deliveryBlobType *int, merkle build.MerkleRoot) (*os.File, error) {
	path, err := fs.BlobPath(ctx, deliveryBlobType, merkle)
	if err != nil {
		return nil, err
	}
	return os.Open(path)
}

func (fs *DirBlobStore) BlobSize(ctx context.Context, deliveryBlobType *int, merkle build.MerkleRoot) (uint64, error) {
	path, err := fs.BlobPath(ctx, deliveryBlobType, merkle)
	if err != nil {
		return 0, err
	}

	s, err := os.Stat(path)
	if err != nil {
		return 0, err
	}

	size := s.Size()
	if size < 0 {
		return 0, fmt.Errorf("merkle %s has size less than zero: %d", merkle, size)
	}

	return uint64(size), nil
}

func (fs *DirBlobStore) Dir() string {
	return fs.dir
}

type Repository struct {
	rootDir          string
	metadataDir      string
	blobStore        BlobStore
	ffx              *ffx.FFXTool
	deliveryBlobType *int
}

type signed struct {
	Signed targets `json:"signed"`
}

type targets struct {
	Targets map[string]targetFile `json:"targets"`
}

type targetFile struct {
	Custom custom `json:"custom"`
}

type custom struct {
	Merkle string `json:"merkle"`
}

// NewRepository parses the repository from the specified directory. It returns
// an error if the repository does not exist, or it contains malformed metadata.
func NewRepository(
	ctx context.Context,
	dir string,
	blobStore BlobStore,
	ffx *ffx.FFXTool,
	deliveryBlobType *int,
) (*Repository, error) {
	logger.Infof(ctx, "creating a repository for %q and %q", dir, blobStore.Dir())

	return &Repository{
		rootDir:          dir,
		metadataDir:      filepath.Join(dir, "repository"),
		blobStore:        blobStore,
		ffx:              ffx,
		deliveryBlobType: deliveryBlobType,
	}, nil
}

func (r *Repository) SetFfx(ffx *ffx.FFXTool) {
	r.ffx = ffx
}

// NewRepositoryFromTar extracts a repository from a tar.gz, and returns a
// Repository parsed from it. It returns an error if the repository does not
// exist, or contains malformed metadata.
func NewRepositoryFromTar(ctx context.Context, dst string, src string, ffx *ffx.FFXTool, deliveryBlobType *int) (*Repository, error) {
	if err := util.Untar(ctx, dst, src); err != nil {
		return nil, fmt.Errorf("failed to extract packages: %w", err)
	}

	return NewRepository(
		ctx,
		filepath.Join(dst, "amber-files"),
		NewDirBlobStore(filepath.Join(dst, "amber-files", "repository", "blobs")),
		ffx,
		deliveryBlobType,
	)
}

// RefreshMetadata updates the expiration of TUF metadata.
func (r *Repository) RefreshMetadata(ctx context.Context) error {
	return r.RefreshMetadataWithFfx(ctx, r.ffx)
}

// RefreshMetadataWithFfx uses a custom ffx to update the expiration of TUF metadata.
func (r *Repository) RefreshMetadataWithFfx(ctx context.Context, ffx *ffx.FFXTool) error {
	logger.Infof(ctx, "Refreshing TUF metadata %s", r.metadataDir)

	// The repository may have out of date metadata. This updates the repository to
	// the latest version so TUF won't complain about the data being old.
	return ffx.RepositoryPublish(ctx, r.rootDir, []string{}, "--refresh-root")
}

// This clones this repository, copying the repository metadata into this
// directory.
func (r *Repository) CloneIntoDir(ctx context.Context, path string) (*Repository, error) {
	logger.Infof(ctx, "Cloning repository %s into %s", r.metadataDir, path)

	// CopyDir wants absolute paths.
	rootDir, err := filepath.Abs(r.rootDir)
	if err != nil {
		return nil, err
	}

	if _, err := osmisc.CopyDir(rootDir, path, osmisc.RaiseError); err != nil {
		return nil, err
	}

	return NewRepository(ctx, path, r.blobStore, r.ffx, r.deliveryBlobType)
}

// OpenPackage opens a package from the repository.
func (r *Repository) OpenPackage(ctx context.Context, path string) (Package, error) {
	// Parse the targets file so we can access packages locally.
	f, err := os.Open(filepath.Join(r.metadataDir, "targets.json"))
	if err != nil {
		return Package{}, err
	}
	defer f.Close()

	var s signed
	if err = json.NewDecoder(f).Decode(&s); err != nil {
		return Package{}, err
	}

	if target, ok := s.Signed.Targets[path]; ok {
		merkle, err := build.DecodeMerkleRoot([]byte(target.Custom.Merkle))
		if err != nil {
			return Package{}, fmt.Errorf(
				"failed to parse package %s merkle %q from TUF: %w",
				path,
				merkle,
				err,
			)
		}

		return newPackage(ctx, r, path, merkle)
	}

	return Package{}, fmt.Errorf("could not find package: %q", path)
}

func (r *Repository) PrefetchUncompressedBlobs(
	ctx context.Context,
	merkles []build.MerkleRoot,
) error {
	if r.deliveryBlobType == nil || !r.ffx.SupportsPackageBlob(ctx) {
		return r.blobStore.PrefetchBlobs(ctx, nil, merkles)
	}

	if err := r.blobStore.PrefetchBlobs(ctx, r.deliveryBlobType, merkles); err != nil {
		return err
	}

	blobs_to_decompress := []string{}
	for _, merkle := range merkles {
		path, err := r.blobStore.BlobPath(ctx, r.deliveryBlobType, merkle)
		if err != nil {
			return err
		}

		// Only decompress a blob if uncompressed blob does not exist, don't call BlobPath because
		// that might decompress the blob for us, but we want to decompress all blobs in one ffx
		// command so that it can be run in parallel.
		_, err = os.Stat(filepath.Join(r.blobStore.Dir(), merkle.String()))
		if err != nil {
			blobs_to_decompress = append(blobs_to_decompress, path)
		}
	}

	if len(blobs_to_decompress) == 0 {
		return nil
	}

	return r.ffx.DecompressBlobs(ctx, blobs_to_decompress, r.blobStore.Dir())
}

func (r *Repository) UncompressedBlobPath(ctx context.Context, merkle build.MerkleRoot) (string, error) {
	return r.blobStore.BlobPath(ctx, nil, merkle)
}

func (r *Repository) OpenUncompressedBlob(ctx context.Context, merkle build.MerkleRoot) (*os.File, error) {
	file, err := r.blobStore.OpenBlob(ctx, nil, merkle)
	if errors.Is(err, os.ErrNotExist) {
		err = r.PrefetchUncompressedBlobs(ctx, []build.MerkleRoot{merkle})
		if err != nil {
			return nil, err
		}
		file, err = r.blobStore.OpenBlob(ctx, nil, merkle)
	}
	if err != nil {
		return nil, fmt.Errorf("failed to open uncompressed blob for merkle %q from TUF: %w", merkle, err)
	}
	return file, nil
}

func (r *Repository) OpenUpdatePackage(ctx context.Context, path string) (*UpdatePackage, error) {
	p, err := r.OpenPackage(ctx, path)
	if err != nil {
		return nil, err
	}

	return newUpdatePackage(ctx, p)
}

func (r *Repository) OpenBlob(ctx context.Context, merkle build.MerkleRoot) (*os.File, error) {
	file, err := r.blobStore.OpenBlob(ctx, r.deliveryBlobType, merkle)
	if err != nil {
		return nil, fmt.Errorf("failed to open blob for merkle %q type %d from TUF: %w", merkle, r.deliveryBlobType, err)
	}
	return file, nil
}

func (r *Repository) BlobSize(ctx context.Context, merkle build.MerkleRoot) (uint64, error) {
	size, err := r.blobStore.BlobSize(ctx, r.deliveryBlobType, merkle)
	if err != nil {
		return 0, fmt.Errorf("failed to get blob size for merkle %q type %d from TUF: %w", merkle, r.deliveryBlobType, err)
	}
	return size, nil

}

func (r *Repository) AlignedBlobSize(ctx context.Context, merkle build.MerkleRoot) (uint64, error) {
	size, err := r.BlobSize(ctx, merkle)
	if err != nil {
		return 0, err
	}

	// Align the number to the next block.
	remainder := size % BlobBlockSize
	if remainder != 0 {
		size += BlobBlockSize - remainder
	}

	return size, nil

}

// sumBlobSizes sums up all the blob sizes from the blob store.
func (r *Repository) sumAlignedBlobSizes(ctx context.Context, blobs map[build.MerkleRoot]struct{}) (uint64, error) {
	// Prefetch the blobs into a single batch to speed up downloads.
	merkles := []build.MerkleRoot{}
	for blob := range blobs {
		merkles = append(merkles, blob)
	}

	if err := r.blobStore.PrefetchBlobs(ctx, r.deliveryBlobType, merkles); err != nil {
		return 0, err
	}

	totalSize := uint64(0)
	for blob := range blobs {
		size, err := r.AlignedBlobSize(ctx, blob)
		if err != nil {
			return 0, nil
		}

		totalSize += size
	}

	return totalSize, nil
}

func (r *Repository) Serve(ctx context.Context, localHostname string, repoName string, repoPort int) (*Server, error) {
	return newServer(ctx, r.metadataDir, r.blobStore, localHostname, repoName, repoPort)
}

func (r *Repository) VerifyMatchesAnyUpdateSystemImageMerkle(ctx context.Context, merkle build.MerkleRoot) error {
	update, err := r.OpenUpdatePackage(ctx, "update/0")
	if err != nil {
		return err
	}

	systemImage, err := update.OpenPackage(ctx, "system_image/0")
	if err != nil {
		return err
	}
	if merkle == systemImage.Merkle() {
		return nil
	}

	updatePrime, err := r.OpenUpdatePackage(ctx, "update_prime/0")
	if err != nil {
		return err
	}

	systemImagePrime, err := updatePrime.OpenPackage(ctx, "system_image/0")
	if err != nil {
		return err
	}
	if merkle == systemImagePrime.Merkle() {
		return nil
	}

	return fmt.Errorf("expected device to be running a system image of %s or %s, got %s",
		systemImage.Merkle(), systemImagePrime.Merkle(), merkle)
}

// CreatePackage creates a package in this repository named `packagePath` by:
// * creating a temporary directory
// * passing it to the `createFunc` closure. The closure then adds any necessary files.
// * creating a package from the directory contents.
// * publishing the package to the repository with the `packagePath` path.
func (r *Repository) CreatePackage(
	ctx context.Context,
	packagePath string,
	createFunc func(path string) error,
) (Package, error) {
	logger.Infof(ctx, "creating package %q", packagePath)

	// Extract the package name from the path. The variant currently is optional, but if specified, must be "0".
	packageName, packageVariant, found := strings.Cut(packagePath, "/")
	if found && packageVariant != "0" {
		return Package{}, fmt.Errorf("invalid package path found: %q", packagePath)
	}
	packageVariant = "0"

	// Create temp directory. The content of this directory will be included in the package.
	tempDir, err := os.MkdirTemp("", "")
	if err != nil {
		return Package{}, fmt.Errorf("failed to create a temp directory: %w", err)
	}
	defer os.RemoveAll(tempDir)

	// Package content will be created by the user by leveraging the createFunc closure.
	if err := createFunc(tempDir); err != nil {
		return Package{}, fmt.Errorf("failed to create content of the package: %w", err)
	}

	// Create package from the temp directory. The package builder doesn't use
	// the repository name, so it can be set as `testrepository.com`.
	pkgBuilder, err := NewPackageBuilderFromDir(tempDir, packageName, packageVariant, "testrepository.com")
	if err != nil {
		return Package{}, fmt.Errorf("failed to parse the package from %q: %w", tempDir, err)
	}

	// Publish the package and get the merkle of the package.
	pkg, err := pkgBuilder.Publish(ctx, r)
	if err != nil {
		return Package{}, fmt.Errorf("failed to publish the package %q: %w", packagePath, err)
	}

	return pkg, nil
}

// EditPackage takes the content of the source package from srcPackagePath,
// copies the content to destination package at dstPackagePath and edits the
// content at destination with the help of editFunc closure.
func (r *Repository) EditPackage(
	ctx context.Context,
	srcPackage Package,
	dstPackagePath string,
	editFunc func(path string) error,
) (Package, error) {
	logger.Infof(ctx, "editing package %q. will create %q", srcPackage.Path(), dstPackagePath)

	// Next create a destination package based on the content oft the source package.
	pkg, err := r.CreatePackage(ctx, dstPackagePath, func(tempDir string) error {
		if err := srcPackage.Expand(ctx, tempDir); err != nil {
			return fmt.Errorf("failed to expand the package to %s: %w", tempDir, err)
		}

		// User can edit the content and return it.
		return editFunc(tempDir)
	})
	if err != nil {
		return Package{}, fmt.Errorf("failed to create the package %q: %w", dstPackagePath, err)
	}

	return pkg, nil
}

func (r *Repository) Publish(ctx context.Context, packageManifestPath string) error {
	extraArgs := []string{"--blob-repo-dir", r.blobStore.Dir()}
	if r.deliveryBlobType != nil {
		extraArgs = append(extraArgs, "--delivery-blob-type", fmt.Sprint(*r.deliveryBlobType))
	}

	return r.ffx.RepositoryPublish(ctx, r.rootDir, []string{packageManifestPath}, extraArgs...)
}
