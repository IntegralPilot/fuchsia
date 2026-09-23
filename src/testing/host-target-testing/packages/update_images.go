// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package packages

import (
	"context"
	"fmt"
	"math/rand"
	"net/url"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

type UpdateImages struct {
	repo       *Repository
	images     util.ImagesManifest
	zbiPackage Package

	// The zbiUrl and vbmetaUrl only differ on the fragment.
	zbiUrl    *url.URL
	vbmetaUrl *url.URL
}

func newUpdateImages(
	ctx context.Context,
	repo *Repository,
	images util.ImagesManifest,
) (*UpdateImages, error) {
	// Create a new zbi+vbmeta package that points at the system image package.
	zbiUrl, zbiMerkle, err := images.GetPartition("fuchsia", "zbi")
	if err != nil {
		return nil, fmt.Errorf("failed to read zbi from images manifest: %w", err)
	}

	if zbiUrl.Fragment == "" {
		return nil, fmt.Errorf("url must have fragment for zbi: %q", zbiUrl)
	}

	vbmetaUrl, _, err := images.GetPartition("fuchsia", "vbmeta")
	if err != nil {
		return nil, fmt.Errorf("failed to read vbmeta from images manifest: %w", err)
	}

	if vbmetaUrl.Fragment == "" {
		return nil, fmt.Errorf("url must have fragment for vbmeta: %q", vbmetaUrl)
	}

	// TODO(http://b/309019440): While it's possible to produce an `images.json`
	// file that has the `zbi` and `vbmeta` files in separate packages, we only
	// support these files being in the same package (which happens to be what
	// the fuchsia build produces). That's because when we use `avbtool.py` to
	// generate the new `vbmeta`, it'll open the `zbi` with `r+`. It's a lot
	// easier for us if both of these files are in the same package so we don't
	// need to copy files around or iteratively generate packages.
	//
	// Because of that, require that these urls are the same, other than the
	// fragment.
	zbiU := *zbiUrl
	vbmetaU := *vbmetaUrl
	zbiU.Fragment = ""
	vbmetaU.Fragment = ""

	if zbiU.String() != vbmetaU.String() {
		return nil, fmt.Errorf(
			"zbi and vbmeta package URLs should only differ on fragment: %s != %s",
			zbiUrl,
			vbmetaUrl,
		)
	}

	zbiPath := zbiUrl.Path[1:]
	zbiPackage, err := newPackage(ctx, repo, zbiPath, zbiMerkle)
	if err != nil {
		return nil, err
	}

	return &UpdateImages{
		repo:       repo,
		images:     images,
		zbiUrl:     zbiUrl,
		vbmetaUrl:  vbmetaUrl,
		zbiPackage: zbiPackage,
	}, nil
}

func (u *UpdateImages) replaceZbiPackage(
	ctx context.Context,
	dstZbi Package,
) (*UpdateImages, error) {
	images := u.images.Clone()

	// Update the zbi partition entry.
	dstZbiUrlString := fmt.Sprintf(
		"fuchsia-pkg://%s/%s?hash=%s#%s",
		u.zbiUrl.Host,
		dstZbi.Path(),
		dstZbi.Merkle(),
		u.zbiUrl.Fragment,
	)
	dstZbiUrl, err := url.Parse(dstZbiUrlString)
	if err != nil {
		return nil, fmt.Errorf("failed to parse url %s: %w", dstZbiUrlString, err)
	}
	logger.Infof(ctx, "updating update image zbi package url to %s", dstZbiUrl)

	zbiBlobPath, err := dstZbi.FilePath(ctx, u.zbiUrl.Fragment)
	if err != nil {
		return nil, fmt.Errorf("failed to find zbi blob path for %s: %w", u.zbiUrl.Fragment, err)
	}

	if err := images.SetPartition("fuchsia", "zbi", dstZbiUrl.String(), zbiBlobPath); err != nil {
		return nil, fmt.Errorf("failed to set zbi partition: %w", err)
	}

	dstVbmetaUrlString := fmt.Sprintf(
		"fuchsia-pkg://%s/%s?hash=%s#%s",
		u.vbmetaUrl.Host,
		dstZbi.Path(),
		dstZbi.Merkle(),
		u.vbmetaUrl.Fragment,
	)
	dstVbmetaUrl, err := url.Parse(dstVbmetaUrlString)
	if err != nil {
		return nil, fmt.Errorf("failed to parse url %s: %w", dstVbmetaUrlString, err)
	}
	logger.Infof(ctx, "updating update image vbmeta package url to %s", dstVbmetaUrl)

	vbmetaBlobPath, err := dstZbi.FilePath(ctx, u.vbmetaUrl.Fragment)
	if err != nil {
		return nil, fmt.Errorf("failed to find zbi blob path for %s: %w", u.vbmetaUrl.Fragment, err)
	}

	if err := images.SetPartition("fuchsia", "vbmeta", dstVbmetaUrl.String(), vbmetaBlobPath); err != nil {
		return nil, fmt.Errorf("failed to set vbmeta partition: %w", err)
	}

	return newUpdateImages(ctx, u.repo, images)
}

func (u *UpdateImages) zbiPath() string {
	return u.zbiUrl.Fragment
}

func (u *UpdateImages) vbmetaPath() string {
	return u.vbmetaUrl.Fragment
}

func (u *UpdateImages) Rehost(newHostname string) error {
	return u.images.Rehost(newHostname)
}

// Create a new zbi+vbmeta package
func (u *UpdateImages) EditZbiAndVbmetaPackage(
	ctx context.Context,
	editFunc func(p Package, zbiName string, vbmetaName string) (Package, error),
) (*UpdateImages, error) {
	dstZbi, err := editFunc(u.zbiPackage, u.zbiUrl.Fragment, u.vbmetaUrl.Fragment)
	if err != nil {
		return nil, err
	}

	return u.replaceZbiPackage(ctx, dstZbi)

}

func (u *UpdateImages) EditZbiAndVbmetaContents(
	ctx context.Context,
	dstPath string,
	editFunc func(tempDir string, zbiName string, vbmetaName string) error,
) (*UpdateImages, error) {
	return u.EditZbiAndVbmetaPackage(
		ctx,
		func(p Package, zbiName string, vbmetaName string) (Package, error) {
			return p.EditContents(ctx, dstPath, func(tempDir string) error {
				return editFunc(tempDir, zbiName, vbmetaName)
			})
		},
	)
}

func (u *UpdateImages) UpdateImagesAlignedBlobSize(ctx context.Context) (uint64, error) {
	blobs, err := u.UpdateImagesBlobs(ctx)
	if err != nil {
		return 0, err
	}

	return u.repo.sumAlignedBlobSizes(ctx, blobs)
}

func (u *UpdateImages) UpdateImagesBlobs(ctx context.Context) (map[build.MerkleRoot]struct{}, error) {
	return u.updateImagesBlobs(ctx, false)
}

func (u *UpdateImages) updateImagesBlobs(
	ctx context.Context,
	ignoreZbi bool,
) (map[build.MerkleRoot]struct{}, error) {
	visitedPackages := make(map[build.MerkleRoot]struct{})
	blobs := make(map[build.MerkleRoot]struct{})

	for _, partition := range u.images.Contents.Partitions {
		url, merkle, err := util.ParsePackageUrl(partition.Url)
		if err != nil {
			return nil, err
		}

		if ignoreZbi && u.zbiPackage.Merkle() == merkle {
			continue
		}

		pkg, err := newPackage(ctx, u.repo, url.Path[1:], merkle)
		if err != nil {
			return nil, err
		}

		if err := pkg.transitiveBlobs(ctx, visitedPackages, blobs); err != nil {
			return nil, err
		}
	}

	for _, firmware := range u.images.Contents.Firmware {
		url, merkle, err := util.ParsePackageUrl(firmware.Url)
		if err != nil {
			return nil, err
		}

		pkg, err := newPackage(ctx, u.repo, url.Path[1:], merkle)
		if err != nil {
			return nil, err
		}

		if err := pkg.transitiveBlobs(ctx, visitedPackages, blobs); err != nil {
			return nil, err
		}
	}

	return blobs, nil
}

func (u *UpdateImages) AddRandomFilesWithUpperBound(
	ctx context.Context,
	rand *rand.Rand,
	dstUpdateImagesPath string,
	maxSize uint64,
) (*UpdateImages, error) {
	additionalBlobs, err := u.updateImagesBlobs(ctx, true)
	if err != nil {
		return nil, err
	}

	return u.EditZbiAndVbmetaPackage(
		ctx,
		func(p Package, zbiName string, vbmetaName string) (Package, error) {
			return p.addRandomFilesWithUpperBound(
				ctx,
				rand,
				dstUpdateImagesPath,
				maxSize,
				additionalBlobs,
			)
		},
	)
}
