// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package packages

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/avb"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/zbi"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

type UpdatePackage struct {
	p                 Package
	packages          map[string]build.MerkleRoot
	hasImagesManifest bool
	images            util.ImagesManifest
}

func newUpdatePackage(ctx context.Context, p Package) (*UpdatePackage, error) {
	// Parse the images manifest, if it exists.
	hasImagesManifest := false
	var images util.ImagesManifest
	if f, err := p.Open(ctx, "images.json"); err == nil {
		defer f.Close()

		i, err := util.ParseImagesJSON(f)
		if err != nil {
			return nil, err
		}

		images = i
		hasImagesManifest = true
	} else if !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}

	// Parse the packages list.
	f, err := p.Open(ctx, "packages.json")
	if err != nil {
		return nil, err
	}
	defer f.Close()

	packages, err := util.ParsePackagesJSON(f)
	if err != nil {
		return nil, err
	}

	return &UpdatePackage{
		p:                 p,
		packages:          packages,
		hasImagesManifest: hasImagesManifest,
		images:            images,
	}, nil
}

// Repository returns the repository that contains this package.
func (p *UpdatePackage) Repository() *Repository {
	return p.p.Repository()
}

func (p *UpdatePackage) String() string {
	return p.p.String()
}

func (u *UpdatePackage) Path() string {
	return u.p.Path()
}

func (u *UpdatePackage) Merkle() build.MerkleRoot {
	return u.p.Merkle()
}

func (u *UpdatePackage) HasImagesManifest() bool {
	return u.hasImagesManifest
}

func (u *UpdatePackage) OpenPackage(ctx context.Context, path string) (Package, error) {
	merkle, ok := u.packages[path]
	if !ok {
		return Package{}, fmt.Errorf("could not find %s merkle in update package %s", path, u.p.Path())
	}

	return newPackage(ctx, u.Repository(), path, merkle)
}

func (u *UpdatePackage) OpenSystemImagePackage(ctx context.Context) (*SystemImagePackage, error) {
	p, err := u.OpenPackage(ctx, "system_image/0")
	if err != nil {
		return nil, err
	}

	systemImagePackage, err := newSystemImagePackage(ctx, p)
	if err != nil {
		return nil, err
	}

	// Make sure that the system image and the `packages.json` file are
	// consistent.
	packagesMerkles := make(map[build.MerkleRoot]string)
	for path, merkle := range u.packages {
		// TODO(b/312027540): Ignore `subpackage_blobs/0` package merkle, which
		// shouldn't be in the static packages manifest. We can remove this
		// after a stepping stone release.
		if path == "subpackage_blobs/0" {
			continue
		}

		if merkle != p.Merkle() {
			packagesMerkles[merkle] = path
		}
	}

	systemImageMerkles := make(map[build.MerkleRoot]string)
	for path, merkle := range systemImagePackage.packages {
		systemImageMerkles[merkle] = path
	}

	for merkle, path := range packagesMerkles {
		if _, ok := systemImageMerkles[merkle]; ok {
			delete(systemImageMerkles, merkle)
		} else {
			return nil, fmt.Errorf(
				"update package %s `packages.json` has package %s merkle %s, but it is missing in system image %s",
				u.Path(),
				path,
				merkle,
				p.Path(),
			)
		}
	}

	if len(systemImageMerkles) != 0 {
		var b bytes.Buffer
		for merkle, path := range systemImageMerkles {
			b.WriteString(fmt.Sprintf("- %s %s\n", path, merkle.String()))
		}

		return nil, fmt.Errorf(
			"system image %s contains packages that are not in the update package %s `packages.json`:\n%s",
			p.Path(),
			u.Path(),
			b.String(),
		)

	}

	return systemImagePackage, nil
}

func (u *UpdatePackage) OpenUpdateImages(ctx context.Context) (*UpdateImages, error) {
	f, err := u.p.Open(ctx, "images.json")
	if err != nil {
		return nil, fmt.Errorf(
			"update package %s does not have an images.json",
			u.p.Path(),
		)
	}
	defer f.Close()

	images, err := util.ParseImagesJSON(f)
	if err != nil {
		return nil, err
	}

	return newUpdateImages(ctx, u.Repository(), images)
}

// Extract the update package `srcUpdatePackage` into a temporary directory,
// then build and publish it to the repository as the `dstUpdatePackage` name.
func (u *UpdatePackage) EditContents(
	ctx context.Context,
	dstUpdatePackagePath string,
	editFunc func(tempDir string) error,
) (*UpdatePackage, error) {
	p, err := u.p.EditContents(ctx, dstUpdatePackagePath, editFunc)
	if err != nil {
		return nil, err
	}

	return newUpdatePackage(ctx, p)
}

// Extract the update package `srcUpdatePackage` into a temporary directory,
// then build and publish it to the repository as the `dstUpdatePackage` name.
func (u *UpdatePackage) EditPackage(
	ctx context.Context,
	editFunc func(pkg Package) (Package, error),
) (*UpdatePackage, error) {
	p, err := editFunc(u.p)
	if err != nil {
		return nil, err
	}

	return newUpdatePackage(ctx, p)
}

// EditSystemImage will extract the system image into a temporary directory,
// provide it to the `editFunc`, then create a new update package that uses it.
func (u *UpdatePackage) EditSystemImagePackage(
	ctx context.Context,
	avbTool *avb.AVBTool,
	zbiTool *zbi.ZBITool,
	repoName string,
	dstUpdatePackagePath string,
	bootfsCompression string,
	editFunc func(systemImage *SystemImagePackage) (*SystemImagePackage, error),
) (*UpdatePackage, *SystemImagePackage, error) {
	srcSystemImage, err := u.OpenSystemImagePackage(ctx)
	if err != nil {
		return nil, nil, fmt.Errorf(
			"failed to open system_image/0 from %s update package: %w",
			u.p.Path(),
			err,
		)
	}

	dstSystemImage, err := editFunc(srcSystemImage)
	if err != nil {
		return nil, nil, err
	}

	dstUpdate, err := u.EditUpdatePackageWithNewSystemImage(
		ctx,
		avbTool,
		zbiTool,
		repoName,
		dstSystemImage,
		dstUpdatePackagePath,
		bootfsCompression,
	)
	if err != nil {
		return nil, nil, err
	}

	return dstUpdate, dstSystemImage, nil
}

// EditImagesPackage will extract the system image into a temporary directory,
// provide it to the `editFunc`, then create a new update package that uses it.
func (u *UpdatePackage) EditUpdateImages(
	ctx context.Context,
	dstUpdatePackagePath string,
	editFunc func(updateImages *UpdateImages) (*UpdateImages, error),
) (*UpdatePackage, *UpdateImages, error) {
	srcUpdateImages, err := u.OpenUpdateImages(ctx)
	if err != nil {
		return nil, nil, err
	}

	dstUpdateImages, err := editFunc(srcUpdateImages)
	if err != nil {
		return nil, nil, err
	}

	dstUpdate, err := u.EditContents(ctx, dstUpdatePackagePath, func(tempDir string) error {
		imagesPath := filepath.Join(tempDir, "images.json")
		f, err := os.Open(imagesPath)
		if err != nil {
			return err
		}
		defer f.Close()

		if err := util.UpdateImagesJSON(imagesPath, dstUpdateImages.images); err != nil {
			return err
		}

		return nil
	})
	if err != nil {
		return nil, nil, err
	}

	return dstUpdate, dstUpdateImages, nil
}

// RehostUpdatePackage will rewrite the `packages.json` and `images.json files
// to use `repoName` path, to avoid collisions with the `fuchsia.com`
// repository name.
func (u *UpdatePackage) RehostUpdatePackage(
	ctx context.Context,
	repoName string,
	dstUpdatePath string,
) (*UpdatePackage, error) {
	return u.EditContents(ctx, dstUpdatePath, func(tempDir string) error {
		return rehostUpdatePackageContents(ctx, tempDir, repoName)
	})
}

func (u *UpdatePackage) EditUpdatePackageWithNewSystemImage(
	ctx context.Context,
	avbTool *avb.AVBTool,
	zbiTool *zbi.ZBITool,
	repoName string,
	systemImage *SystemImagePackage,
	dstUpdatePackagePath string,
	bootfsCompression string,
) (*UpdatePackage, error) {
	return u.EditContents(
		ctx,
		dstUpdatePackagePath,
		func(tempDir string) error {
			// Update the update package's zbi and vbmeta to point at this system image.
			if err := u.editZbiAndVbmeta(
				ctx,
				repoName,
				dstUpdatePackagePath,
				tempDir,
				func(tempDir string, zbiName string, vbmetaName string) error {
					zbiPath := filepath.Join(tempDir, zbiName)
					vbmetaPath := filepath.Join(tempDir, vbmetaName)

					if err := zbiTool.UpdateZBIWithNewSystemImageMerkle(
						ctx,
						systemImage.Merkle(),
						zbiPath,
						zbiPath,
						bootfsCompression,
					); err != nil {
						return err
					}

					if err := avbTool.MakeVBMetaImageWithZbi(
						ctx,
						vbmetaPath,
						vbmetaPath,
						zbiPath,
					); err != nil {
						return err
					}

					return nil
				},
			); err != nil {
				return err
			}

			// Update the `system_image/0` entry for this new system image.
			packagesJsonPath := filepath.Join(tempDir, "packages.json")
			if err := util.UpdateHashValuePackagesJSON(
				packagesJsonPath,
				repoName,
				"system_image/0",
				systemImage.Merkle(),
			); err != nil {
				return err
			}

			return rehostUpdatePackageContents(ctx, tempDir, repoName)
		},
	)
}

// Extracts the update package into a temporary directory, and injects the
// specified vbmeta property files into the vbmeta.
func (u *UpdatePackage) EditUpdatePackageWithVBMetaProperties(
	ctx context.Context,
	avbTool *avb.AVBTool,
	repoName string,
	dstUpdatePackagePath string,
	vbmetaPropertyFiles map[string]string,
) (*UpdatePackage, error) {
	return u.EditContents(
		ctx,
		dstUpdatePackagePath,
		func(tempDir string) error {
			// Update the update package's zbi and vbmeta to point at this system image.
			if err := u.editZbiAndVbmeta(
				ctx,
				repoName,
				dstUpdatePackagePath,
				tempDir,
				func(tempDir string, zbiName string, vbmetaName string) error {
					vbmetaPath := filepath.Join(tempDir, vbmetaName)
					logger.Infof(ctx, "updating vbmeta %q", vbmetaPath)

					if err := util.AtomicallyWriteFile(vbmetaPath, 0600, func(f *os.File) error {
						if err := avbTool.MakeVBMetaImage(ctx, f.Name(), vbmetaPath, vbmetaPropertyFiles); err != nil {
							return fmt.Errorf("failed to update vbmeta: %w", err)
						}
						return nil
					}); err != nil {
						return fmt.Errorf("failed to atomically overwrite %q: %w", vbmetaPath, err)
					}

					return nil
				},
			); err != nil {
				return err
			}

			return rehostUpdatePackageContents(ctx, tempDir, repoName)
		},
	)
}

// editZbiAndVbmeta will allow the `editFunc` to modify the zbi and vbmeta from
// an update package, whether or not those files are in a side-package listed in
// the images.json file, or directly embedded in the update package.
func (u *UpdatePackage) editZbiAndVbmeta(
	ctx context.Context,
	repoName string,
	dstUpdatePackagePath string,
	tempDir string,
	editFunc func(tempDir string, zbiName string, vbmetaName string) error,
) error {
	// If the update package has an `images.json`, then edit the zbi found
	// inside it, rather than the one in the update package.
	if u.hasImagesManifest {
		updateImages, err := u.OpenUpdateImages(ctx)
		if err != nil {
			return err
		}

		return u.replaceUpdateImages(
			ctx,
			repoName,
			dstUpdatePackagePath,
			tempDir,
			updateImages,
			editFunc,
		)
	}

	// Otherwise we need to migrate to the new update package format.
	logger.Infof(ctx, "Converting update package %s to new update format", dstUpdatePackagePath)

	originalImagesName := "images.json.orig"
	originalImagesPath := filepath.Join(tempDir, originalImagesName)
	f, err := os.Open(originalImagesPath)
	if err != nil {
		return err
	}
	defer f.Close()

	images, err := util.ParseImagesJSON(f)
	if err != nil {
		return err
	}

	updateImages, err := newUpdateImages(ctx, u.Repository(), images)
	if err != nil {
		return err
	}

	namesToRemove := []string{
		originalImagesName,
		"zbi",
		"fuchsia.vbmeta",
		"recovery",
		"recovery.vbmeta",
	}

	// Add all the `firmware*` files.
	entries, err := os.ReadDir(tempDir)
	if err != nil {
		return err
	}
	for _, entry := range entries {
		if strings.HasPrefix(entry.Name(), "firmware") {
			namesToRemove = append(namesToRemove, entry.Name())
		}
	}

	for _, name := range namesToRemove {
		path := filepath.Join(tempDir, name)
		if _, err := os.Stat(path); err == nil {
			logger.Infof(ctx, "removing %q from the update package %s", name, dstUpdatePackagePath)
			if err := os.Remove(path); err != nil {
				return err
			}
		}
	}

	return u.replaceUpdateImages(
		ctx,
		repoName,
		dstUpdatePackagePath,
		tempDir,
		updateImages,
		editFunc,
	)
}

func (u *UpdatePackage) replaceUpdateImages(
	ctx context.Context,
	repoName string,
	dstUpdatePackagePath string,
	tempDir string,
	srcUpdateImages *UpdateImages,
	editFunc func(tempDir string, zbiName string, vbmetaName string) error,
) error {
	dstZbiPackagePath := util.AddSuffixToPackageName(dstUpdatePackagePath, "update-images-zbi")
	dstUpdateImages, err := srcUpdateImages.EditZbiAndVbmetaContents(
		ctx,
		dstZbiPackagePath,
		editFunc,
	)
	if err != nil {
		return err
	}

	if err := dstUpdateImages.Rehost(repoName); err != nil {
		return err
	}

	imagesPath := filepath.Join(tempDir, "images.json")
	if err := util.UpdateImagesJSON(imagesPath, dstUpdateImages.images); err != nil {
		return err
	}

	return nil
}

// UpdatePackageSize returns the transitive space needed to install the update
// package and all subpackage blobs. It does not include update image blobs, or
// system image blobs.
func (u *UpdatePackage) UpdatePackageSize(ctx context.Context) (uint64, error) {
	return u.p.TransitiveBlobSize(ctx)
}

func rehostUpdatePackageContents(ctx context.Context, tempDir string, repoName string) error {
	packagesJsonPath := filepath.Join(tempDir, "packages.json")
	logger.Infof(ctx, "setting host name in %q to %q", packagesJsonPath, repoName)

	// Rehost the rest of the packages.json urls.
	if err := util.AtomicallyWriteFile(packagesJsonPath, 0600, func(f *os.File) error {
		src, err := os.Open(packagesJsonPath)
		if err != nil {
			return fmt.Errorf("Failed to open packages.json %q: %w", packagesJsonPath, err)
		}
		if err := util.RehostPackagesJSON(bufio.NewReader(src), bufio.NewWriter(f), repoName); err != nil {
			return fmt.Errorf("Failed to rehost packages.json: %w", err)
		}

		return nil
	}); err != nil {
		return fmt.Errorf("Failed to atomically overwrite %q: %w", packagesJsonPath, err)
	}

	// Optionally rehost images.json if it exists.
	imagesJsonPath := filepath.Join(tempDir, "images.json")
	if _, err := os.Stat(imagesJsonPath); err == nil {
		logger.Infof(ctx, "setting host name in %q to %q", imagesJsonPath, repoName)

		src, err := os.Open(imagesJsonPath)
		if err != nil {
			return fmt.Errorf("failed to open %q: %w", imagesJsonPath, err)
		}
		defer src.Close()

		images, err := util.ParseImagesJSON(src)
		if err != nil {
			return fmt.Errorf("failed to parse %q: %w", imagesJsonPath, err)
		}

		if err := images.Rehost(repoName); err != nil {
			return fmt.Errorf("failed to rehost %q: %w", imagesJsonPath, err)
		}

		if err := util.UpdateImagesJSON(imagesJsonPath, images); err != nil {
			return fmt.Errorf("failed write %q: %w", imagesJsonPath, err)
		}
	}

	return nil
}
