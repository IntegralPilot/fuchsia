// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifacts

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"strconv"
	"strings"

	"golang.org/x/crypto/ssh"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/avb"
	pmBuild "go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/omaha_tool"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/packages"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/paver"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/zbi"
	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

type BlobFetchMode int

const (
	// PrefetchBlobs will download all the blobs from a build when `GetPackageRepository()` is called.
	PrefetchBlobs BlobFetchMode = iota

	// LazilyFetchBlobs will only download blobs when they are accessed.
	LazilyFetchBlobs

	// Product Bundle manifest which is used to locate VBmeta
	ProductBundleManifest = "product_bundle.json"

	relativeBootserverPath = "tools/linux-x64/bootserver"

	relativeFfxPath = "tools/linux-x64/ffx"

	relativeFlashManifest = "images/flash.json"

	productBundleBuildInfoPath = "build_api/build_info.json"
)

type Build interface {
	// String returns a string identifier for this build.
	String() string

	// OutputDir is a location where we can write generated test files.
	OutputDir() string

	// GetBootserver returns the path to the bootserver used for paving.
	GetBootserver(ctx context.Context) (string, error)

	// GetFfx returns the FFXTool from this build.
	GetFfx(
		ctx context.Context,
		ffxRunDir ffx.RunDir,
		version ffx.FfxVersionPolicy,
	) (*ffx.FFXTool, error)

	// GetFlashManifest returns the path to the flash manifest used for flashing.
	GetFlashManifest(ctx context.Context) (string, error)

	// GetProductBundleDir returns the path to the product bundle.
	GetProductBundleDir(ctx context.Context) (string, error)

	// GetPackageRepository returns a Repository for this build.
	GetPackageRepository(
		ctx context.Context,
		blobFetchMode BlobFetchMode,
		ffxRunDir ffx.RunDir,
		version ffx.FfxVersionPolicy,
		hostFfx *ffx.FFXTool,
	) (*packages.Repository, error)

	// GetPaverDir downloads and returns the directory containing the images
	// and image manifest.
	GetPaverDir(ctx context.Context) (string, error)

	// GetPaver downloads and returns a paver for the build.
	GetPaver(
		ctx context.Context,
		sshPublicKey ssh.PublicKey,
	) (paver.Paver, error)

	// GetVbmetaPath downloads and returns a path to the zircon-a vbmeta image.
	GetVbmetaPath(ctx context.Context) (string, error)
}

type BuildWithVersion struct {
	Build   Build
	Version ffx.FfxVersionPolicy
}

// ArtifactsBuild represents the build artifacts for a specific build.
type ArtifactsBuild struct {
	id               string
	archive          *Archive
	blobsDir         string
	buildDir         string
	packages         *packages.Repository
	buildImageDir    string
	productBundleDir string
	srcs             map[string]struct{}
}

func (b *ArtifactsBuild) OutputDir() string {
	return b.buildDir
}

func (b *ArtifactsBuild) GetBootserver(ctx context.Context) (string, error) {
	// We need build images to flash, so lets download them.
	_, err := b.GetBuildImages(ctx)
	if err != nil {
		return "", err
	}

	// Use the latest bootserver if possible because the one uploaded with the artifacts may not include bug fixes.
	currentBuildId := os.Getenv("BUILDBUCKET_ID")
	if currentBuildId == "" {
		currentBuildId = b.id
	}

	bootserverPath := filepath.Join(b.buildDir, relativeBootserverPath)
	if err := b.archive.download(
		ctx,
		currentBuildId,
		false,
		b.buildDir,
		[]string{relativeBootserverPath},
	); err != nil {
		return "", fmt.Errorf("failed to download bootserver: %w", err)
	}

	// Make bootserver executable.
	if err := os.Chmod(bootserverPath, os.ModePerm); err != nil {
		return "", fmt.Errorf("failed to make bootserver executable: %w", err)
	}

	return bootserverPath, nil
}

func (b *ArtifactsBuild) GetFfx(
	ctx context.Context,
	ffxRunDir ffx.RunDir,
	version ffx.FfxVersionPolicy,
) (*ffx.FFXTool, error) {
	// Get the ffx tool for this build.
	ffxPath := filepath.Join(b.buildDir, relativeFfxPath)

	if err := b.archive.download(
		ctx,
		b.id,
		false,
		b.buildDir,
		[]string{relativeFfxPath},
	); err != nil {
		return nil, fmt.Errorf("failed to download ffxPath: %w", err)
	}

	for _, subtool := range []string{"package", "product", "component", "emulator", "repository", "product-bundle"} {
		if err := b.downloadSubtool(ctx, subtool); err != nil {
			return nil, err
		}
	}

	// Make ffx executable.
	if err := os.Chmod(ffxPath, os.ModePerm); err != nil {
		return nil, fmt.Errorf("failed to make ffxPath executable: %w", err)
	}

	return ffx.NewFFXToolForVersion(ctx, ffxPath, ffxRunDir, version, "")
}

func (b *ArtifactsBuild) downloadSubtool(
	ctx context.Context,
	name string,
) error {
	ffxPath := filepath.Join(b.buildDir, relativeFfxPath)
	subtoolPath := ffxPath + "-" + name

	// Ignore error, not all builds upload all subtools
	if err := b.archive.download(
		ctx,
		b.id,
		false,
		b.buildDir,
		[]string{relativeFfxPath + "-" + name},
	); err == nil {
		if err := os.Chmod(subtoolPath, os.ModePerm); err != nil {
			return fmt.Errorf("failed to make subtool %s executable: %w", name, err)
		}
		// Try to download the manifest, but ignore error if it fails
		_ = b.archive.download(
			ctx,
			b.id,
			false,
			b.buildDir,
			[]string{relativeFfxPath + "-" + name + ".json"},
		)
	}
	return nil
}

func (b *ArtifactsBuild) GetFlashManifest(ctx context.Context) (string, error) {
	// We need build images to flash.
	_, err := b.GetBuildImages(ctx)
	if err != nil {
		return "", err
	}

	flashManifest := filepath.Join(b.buildDir, relativeFlashManifest)
	if err := b.archive.download(
		ctx,
		b.id,
		false,
		b.buildDir,
		[]string{relativeFlashManifest},
	); err != nil {
		return "", fmt.Errorf("failed to download flash.json for flasher: %w", err)
	}

	return flashManifest, nil
}

func (b *ArtifactsBuild) GetProductBundleDir(ctx context.Context) (string, error) {
	if b.productBundleDir != "" {
		return b.productBundleDir, nil
	}

	logger.Infof(ctx, "downloading product bundle for build %s", b.id)

	buildInfoPath := filepath.Join(b.buildDir, productBundleBuildInfoPath)

	// Fetch build_info.json, this is needed to construct the PB path in GCS
	if err := b.archive.download(
		ctx,
		b.id,
		false,
		b.buildDir,
		[]string{productBundleBuildInfoPath},
	); err != nil {
		logger.Errorf(ctx, "failed to download build info for build %s: %v", b.id, err)
		return "", fmt.Errorf("failed to download build info for build %s: %w", b.id, err)
	}

	f, err := os.Open(buildInfoPath)
	if err != nil {
		return "", err
	}
	defer f.Close()

	// BuildInfo represent the build info build-api.
	var buildInfo struct {
		// Conguration is the array of product/board dictionary.
		Configurations []struct {
			// Board is the board name. e.g. "x64", "vim3".
			Board string `json:"board"`

			// Product is the product name. e.g. "minimal", "core".
			Product string `json:"product"`
		} `json:"configurations"`

		// A unique version of this build.
		Version string `json:"version"`
	}

	if err := json.NewDecoder(f).Decode(&buildInfo); err != nil {
		return "", fmt.Errorf("failed to read build info: %w", err)
	}

	// The first entry is the product bundle for the build.
	config := buildInfo.Configurations[0]

	// Fetch PB from GCS
	pbPath := fmt.Sprintf("product_bundles/%s.%s", config.Product, config.Board)

	artifacts, err := b.archive.list(ctx, b.id)
	if err != nil {
		return "", err
	}

	pbArtifacts := []string{}
	prefix := pbPath + "/"
	for _, artifact := range artifacts {
		if strings.HasPrefix(artifact, prefix) {
			pbArtifacts = append(pbArtifacts, artifact)
		}
	}

	productBundleDir := filepath.Join(b.buildDir, pbPath)
	if err := b.archive.download(
		ctx,
		b.id,
		false,
		b.buildDir,
		pbArtifacts,
	); err != nil {
		logger.Errorf(ctx, "failed to download product bundle for build %s: %v", b.id, err)
		return "", fmt.Errorf("failed to download product bundle for build %s: %w", b.id, err)
	}

	b.productBundleDir = productBundleDir
	return b.productBundleDir, nil
}

// GetPackageRepository returns a Repository for this build. It tries to
// download a package when all the artifacts are stored in individual files,
// which is how modern builds publish their build artifacts.
func (b *ArtifactsBuild) GetPackageRepository(
	ctx context.Context,
	fetchMode BlobFetchMode,
	ffxRunDir ffx.RunDir,
	version ffx.FfxVersionPolicy,
	hostFfx *ffx.FFXTool,
) (*packages.Repository, error) {
	if b.packages != nil {
		return b.packages, nil
	}

	logger.Infof(ctx, "downloading package repository")

	// Make sure the blob contains the `packages/all_blobs.json`.
	if _, ok := b.srcs["packages/all_blobs.json"]; !ok {
		logger.Errorf(ctx, "blobs manifest doesn't exist for build %s", b.id)
		return nil, fmt.Errorf("blob manifest doesn't exist for build %s", b.id)
	}

	packageSrcs := []string{}
	for src := range b.srcs {
		if strings.HasPrefix(src, "packages/") {
			packageSrcs = append(packageSrcs, src)
		}
	}

	packagesDir := filepath.Join(b.buildDir, "packages")
	if err := b.archive.download(
		ctx,
		b.id,
		false,
		b.buildDir,
		packageSrcs,
	); err != nil {
		logger.Errorf(ctx, "failed to download packages for build %s to %s: %v", packagesDir, b.id, err)
		return nil, fmt.Errorf("failed to download packages for build %s to %s: %w", packagesDir, b.id, err)
	}

	blobsManifest := filepath.Join(packagesDir, "all_blobs.json")
	blobsData, err := os.ReadFile(blobsManifest)
	if err != nil {
		return nil, fmt.Errorf("failed to read blobs manifest: %w", err)
	}

	var blobs []build.Blob
	err = json.Unmarshal(blobsData, &blobs)
	if err != nil {
		return nil, fmt.Errorf("failed to unmarshal blobs JSON: %w", err)
	}

	deliveryBlobConfigPath := filepath.Join(packagesDir, "delivery_blob_config.json")
	relativeBlobsDir, err := build.GetBlobsDir(deliveryBlobConfigPath)
	if err != nil {
		return nil, fmt.Errorf("failed to get blobs dir: %w", err)
	}

	var blobsList []string
	for _, blob := range blobs {
		blobsList = append(blobsList, filepath.Join(relativeBlobsDir, blob.Merkle))
	}
	logger.Infof(ctx, "all_blobs contains %d blobs", len(blobs))

	if fetchMode == PrefetchBlobs {
		if err := b.archive.download(
			ctx,
			b.id,
			true,
			filepath.Join(b.blobsDir),
			blobsList,
		); err != nil {
			logger.Errorf(ctx, "failed to download blobs to %s: %v", b.blobsDir, err)
			return nil, fmt.Errorf("failed to download blobs to %s: %w", b.blobsDir, err)
		}
	}

	var ffxTool *ffx.FFXTool
	if version == ffx.FfxVersionPolicyLatest && hostFfx != nil {
		ffxTool = hostFfx
	} else {
		ffxTool, err = b.GetFfx(ctx, ffxRunDir, version)
		if err != nil {
			return nil, fmt.Errorf("failed to get ffx: %w", err)
		}
	}

	blobType, err := build.GetDeliveryBlobType(deliveryBlobConfigPath)
	if err != nil {
		return nil, fmt.Errorf("failed to get delivery blob type: %w", err)
	}

	p, err := packages.NewRepository(
		ctx,
		packagesDir,
		&proxyBlobStore{
			b:        b,
			ffx:      ffxTool,
			blobsDir: b.blobsDir,
			blobType: blobType,
		},
		ffxTool,
		blobType,
	)
	if err != nil {
		return nil, err
	}
	b.packages = p

	return b.packages, nil
}

type proxyBlobStore struct {
	b        *ArtifactsBuild
	ffx      *ffx.FFXTool
	blobsDir string
	blobType *int
}

func (fs *proxyBlobStore) PrefetchBlobs(
	ctx context.Context,
	deliveryBlobType *int,
	merkles []pmBuild.MerkleRoot,
) error {
	if len(merkles) == 0 {
		return nil
	}

	var relativeBlobsDir string

	if deliveryBlobType == nil {
		relativeBlobsDir = "blobs"
	} else {
		deliveryBlobType := strconv.Itoa(*deliveryBlobType)
		relativeBlobsDir = filepath.Join("blobs", deliveryBlobType)
	}

	srcs := []string{}
	for _, merkle := range merkles {
		src := filepath.Join(relativeBlobsDir, merkle.String())
		srcs = append(srcs, src)
	}

	// Start downloading the blobs. The package resolver will only fetch a blob
	// once, so we don't need to deduplicate requests on our side.

	return fs.b.archive.download(
		ctx,
		fs.b.id,
		true,
		filepath.Dir(fs.blobsDir),
		srcs,
	)
}

func (fs *proxyBlobStore) BlobPath(ctx context.Context, deliveryBlobType *int, merkle pmBuild.MerkleRoot) (string, error) {
	var path string
	if deliveryBlobType == nil {
		path = filepath.Join(fs.blobsDir, merkle.String())
	} else {
		path = filepath.Join(fs.blobsDir, strconv.Itoa(*deliveryBlobType), merkle.String())
	}

	// First, try to read the blob from the directory
	if _, err := os.Stat(path); err == nil {
		return path, nil
	}

	// Decompress delivery blob if possible.
	if deliveryBlobType == nil && fs.blobType != nil && fs.ffx.SupportsPackageBlob(ctx) {
		deliveryBlobPath := filepath.Join(fs.blobsDir, strconv.Itoa(*fs.blobType), merkle.String())
		_, err := os.Stat(deliveryBlobPath)
		if err != nil {
			err = fs.PrefetchBlobs(ctx, fs.blobType, []pmBuild.MerkleRoot{merkle})
			if err != nil {
				logger.Errorf(ctx, "failed to prefetch type %d blob %s: %v", *fs.blobType, merkle, err)
			}
		}
		if err == nil {
			err = fs.ffx.DecompressBlobs(ctx, []string{deliveryBlobPath}, fs.blobsDir)
			if err == nil {
				return path, nil
			}
			logger.Errorf(ctx, "failed to decompress blob %s: %v", deliveryBlobPath, err)
		}
	}

	if err := fs.PrefetchBlobs(ctx, deliveryBlobType, []pmBuild.MerkleRoot{merkle}); err != nil {
		return "", err
	}

	return path, nil
}

func (fs *proxyBlobStore) OpenBlob(ctx context.Context, deliveryBlobType *int, merkle pmBuild.MerkleRoot) (*os.File, error) {
	path, err := fs.BlobPath(ctx, deliveryBlobType, merkle)
	if err != nil {
		return nil, err
	}

	return os.Open(path)
}

func (fs *proxyBlobStore) BlobSize(ctx context.Context, deliveryBlobType *int, merkle pmBuild.MerkleRoot) (uint64, error) {
	f, err := fs.OpenBlob(ctx, deliveryBlobType, merkle)
	if err != nil {
		return 0, err
	}
	defer f.Close()

	if s, err := f.Stat(); err == nil {
		if s.Size() < 0 {
			return 0, fmt.Errorf("merkle %s has size less than zero: %d", merkle, s.Size())
		}
		return uint64(s.Size()), nil
	} else {
		return 0, err
	}
}

func (fs *proxyBlobStore) Dir() string {
	return fs.blobsDir
}

// GetBuildImages downloads the build images for a specific build id.
// Returns a path to the directory of the downloaded images or an error if it
// fails to download.
func (b *ArtifactsBuild) GetBuildImages(ctx context.Context) (string, error) {
	if b.buildImageDir != "" {
		return b.buildImageDir, nil
	}

	logger.Infof(ctx, "downloading build images")

	imageDir := filepath.Join(b.buildDir, "images")
	if err := b.archive.download(
		ctx,
		b.id,
		false,
		b.buildDir,
		[]string{path.Join("images", paver.ImageManifest)},
	); err != nil {
		return "", fmt.Errorf("failed to download image manifest: %w", err)
	}
	imagesJSON := filepath.Join(imageDir, paver.ImageManifest)
	f, err := os.Open(imagesJSON)
	if err != nil {
		return "", fmt.Errorf("failed to open %q: %w", imagesJSON, err)
	}
	defer f.Close()

	var items []build.Image
	if err := json.NewDecoder(f).Decode(&items); err != nil {
		return "", fmt.Errorf("failed to parse build image %q: %w", imagesJSON, err)
	}

	// Get list of all available images to download and only download
	// the ones needed for flashing or paving.
	imageSrcMap := make(map[string]struct{})
	for src := range b.srcs {
		if strings.HasPrefix(src, "images/") {
			imageSrcMap[src] = struct{}{}
		}
	}
	imageSrcs := []string{}
	for _, item := range items {
		src := path.Join("images", item.Path)
		if _, ok := imageSrcMap[src]; ok {
			imageSrcs = append(imageSrcs, src)
		}
	}

	if err := b.archive.download(
		ctx,
		b.id,
		false,
		b.buildDir,
		imageSrcs,
	); err != nil {
		return "", fmt.Errorf("failed to download images to %s: %w", imageDir, err)
	}

	b.buildImageDir = imageDir
	return b.buildImageDir, nil
}

func (b *ArtifactsBuild) GetPaverDir(ctx context.Context) (string, error) {
	return b.GetBuildImages(ctx)
}

// GetPaver downloads and returns a paver for the build.
func (b *ArtifactsBuild) GetPaver(
	ctx context.Context,
	sshPublicKey ssh.PublicKey,
) (paver.Paver, error) {
	return b.getPaver(ctx, sshPublicKey)
}

func (b *ArtifactsBuild) getPaver(
	ctx context.Context,
	sshPublicKey ssh.PublicKey,
) (*paver.BuildPaver, error) {
	buildImageDir, err := b.GetPaverDir(ctx)
	if err != nil {
		return nil, err
	}

	bootserverPath, err := b.GetBootserver(ctx)
	if err != nil {
		return nil, err
	}

	return paver.NewBuildPaver(bootserverPath, buildImageDir, paver.SSHPublicKey(sshPublicKey))
}

func (b *ArtifactsBuild) GetVbmetaPath(ctx context.Context) (string, error) {
	path, err := b.getVbmetaPathFromProductBundle(ctx)
	if err == nil {
		return path, nil
	}

	logger.Warningf(ctx, "failed to get vbmeta from product bundle, trying images: %v", err)
	path, err = b.getVbmetaPathFromImages(ctx)
	if err != nil {
		return "", err
	}

	return path, nil
}

func (b *ArtifactsBuild) getVbmetaPathFromProductBundle(ctx context.Context) (string, error) {
	productBundleDir, err := b.GetProductBundleDir(ctx)
	if err != nil {
		return "", err
	}

	productBundle, err := util.ParseProductBundle(productBundleDir)
	if err != nil {
		return "", err
	}

	return productBundle.GetSystemAImage("vbmeta", "zircon-a")
}

func (b *ArtifactsBuild) getVbmetaPathFromImages(ctx context.Context) (string, error) {
	buildImageDir, err := b.GetBuildImages(ctx)
	if err != nil {
		return "", err
	}
	imagesJSON := filepath.Join(buildImageDir, paver.ImageManifest)
	f, err := os.Open(imagesJSON)
	if err != nil {
		return "", fmt.Errorf("failed to open %q: %w", imagesJSON, err)
	}
	defer f.Close()

	var items []struct {
		Name string `json:"name"`
		Path string `json:"path"`
		Type string `json:"type"`
	}
	if err := json.NewDecoder(f).Decode(&items); err != nil {
		return "", fmt.Errorf("failed to parse vbmeta image %q: %w", imagesJSON, err)
	}

	for _, item := range items {
		if item.Name == "zircon-a" && item.Type == "vbmeta" {
			return filepath.Join(buildImageDir, item.Path), nil
		}
	}

	return "", fmt.Errorf("failed to file zircon-a vbmeta in %q", imagesJSON)
}

func (b *ArtifactsBuild) String() string {
	return b.id
}

type FuchsiaDirBuild struct {
	dir string
}

func NewFuchsiaDirBuild(
	dir string,
) *FuchsiaDirBuild {
	return &FuchsiaDirBuild{
		dir: dir,
	}
}

func (b *FuchsiaDirBuild) String() string {
	return b.dir
}

func (b *FuchsiaDirBuild) OutputDir() string {
	return b.dir
}

func (b *FuchsiaDirBuild) GetBootserver(ctx context.Context) (string, error) {
	return filepath.Join(b.dir, "host_x64/bootserver_new"), nil
}

func (b *FuchsiaDirBuild) GetFfx(
	ctx context.Context,
	ffxRunDir ffx.RunDir,
	version ffx.FfxVersionPolicy,
) (*ffx.FFXTool, error) {
	ffxPath := filepath.Join(b.dir, "host_x64/ffx")
	return ffx.NewFFXToolForVersion(ctx, ffxPath, ffxRunDir, version, "")
}

func (b *FuchsiaDirBuild) GetFlashManifest(ctx context.Context) (string, error) {
	return filepath.Join(b.dir, "flash.json"), nil
}

func (b *FuchsiaDirBuild) GetProductBundleDir(ctx context.Context) (string, error) {
	productBundlesPath := filepath.Join(b.dir, "product_bundles.json")
	f, err := os.Open(productBundlesPath)
	if err != nil {
		return "", fmt.Errorf("failed to open %s: %w", productBundlesPath, err)
	}
	defer f.Close()

	var productBundles []struct {
		Path string `json:"path"`
	}

	if err := json.NewDecoder(f).Decode(&productBundles); err != nil {
		return "", fmt.Errorf("failed to parse product bundles path %s: %w", productBundlesPath, err)
	}

	// The first entry is the product bundle for the build.
	return filepath.Join(b.dir, productBundles[0].Path), nil
}

func (b *FuchsiaDirBuild) GetPackageRepository(
	ctx context.Context,
	blobFetchMode BlobFetchMode,
	ffxRunDir ffx.RunDir,
	version ffx.FfxVersionPolicy,
	hostFfx *ffx.FFXTool,
) (*packages.Repository, error) {
	// hostFfx is ignored because FuchsiaDirBuild uses the ffx binary from
	// its own local build directory. Unlike ArtifactsBuild, it does not
	// download ffx and does not benefit from the host tool optimization.
	ffxTool, err := b.GetFfx(ctx, ffxRunDir, version)
	if err != nil {
		return nil, fmt.Errorf("failed to get ffx: %w", err)
	}

	blobFS := packages.NewDirBlobStore(filepath.Join(b.dir, "amber-files", "repository", "blobs"))
	blobType, err := build.GetDeliveryBlobType(filepath.Join(b.dir, "delivery_blob_config.json"))
	if err != nil {
		return nil, fmt.Errorf("failed to get delivery blob type: %w", err)
	}
	return packages.NewRepository(ctx, filepath.Join(b.dir, "amber-files"), blobFS, ffxTool, blobType)
}

func (b *FuchsiaDirBuild) GetPaverDir(ctx context.Context) (string, error) {
	return b.dir, nil
}

func (b *FuchsiaDirBuild) GetPaver(
	ctx context.Context,
	sshPublicKey ssh.PublicKey,
) (paver.Paver, error) {
	bootserverPath, err := b.GetBootserver(ctx)
	if err != nil {
		return nil, err
	}

	return paver.NewBuildPaver(bootserverPath, b.dir, paver.SSHPublicKey(sshPublicKey))
}

func (b *FuchsiaDirBuild) GetVbmetaPath(ctx context.Context) (string, error) {
	imagesJSON := filepath.Join(b.dir, paver.ImageManifest)
	f, err := os.Open(imagesJSON)
	if err != nil {
		return "", fmt.Errorf("failed to open %q: %w", imagesJSON, err)
	}
	defer f.Close()

	var items []build.Image
	if err := json.NewDecoder(f).Decode(&items); err != nil {
		return "", fmt.Errorf("failed to parse vbmeta imagesJSON %q: %w", imagesJSON, err)
	}

	for _, item := range items {
		if item.Name == "zircon-a" && item.Type == "vbmeta" {
			return filepath.Join(b.dir, item.Path), nil
		}
	}

	return "", fmt.Errorf("failed to file zircon-a vbmeta in %q", imagesJSON)
}

type ProductBundleDirBuild struct {
	productBundleDir string
	outputDir        string
}

func NewProductBundleDirBuild(
	productBundleDir string,
) *ProductBundleDirBuild {
	return &ProductBundleDirBuild{
		productBundleDir: productBundleDir,
	}
}

func (b *ProductBundleDirBuild) String() string {
	return b.productBundleDir
}

func (b *ProductBundleDirBuild) OutputDir() string {
	return b.outputDir
}

func (b *ProductBundleDirBuild) GetBootserver(ctx context.Context) (string, error) {
	// Only flashing is supported for ProductBundle
	return "", fmt.Errorf("paving is not supported with product bundles")
}

func (b *ProductBundleDirBuild) GetFfx(
	ctx context.Context,
	ffxRunDir ffx.RunDir,
	version ffx.FfxVersionPolicy,
) (*ffx.FFXTool, error) {
	ffxPath := filepath.Join(b.productBundleDir, "ffx")
	return ffx.NewFFXToolForVersion(ctx, ffxPath, ffxRunDir, version, "")
}

func (b *ProductBundleDirBuild) GetFlashManifest(ctx context.Context) (string, error) {
	return b.productBundleDir, nil
}

func (b *ProductBundleDirBuild) GetProductBundleDir(ctx context.Context) (string, error) {
	return b.productBundleDir, nil
}

func (b *ProductBundleDirBuild) GetPackageRepository(
	ctx context.Context,
	blobFetchMode BlobFetchMode,
	ffxRunDir ffx.RunDir,
	version ffx.FfxVersionPolicy,
	hostFfx *ffx.FFXTool,
) (*packages.Repository, error) {
	// hostFfx is ignored because ProductBundleDirBuild uses the ffx binary
	// from the local product bundle directory. It does not download ffx
	// and does not benefit from the host tool optimization.
	ffxTool, err := b.GetFfx(ctx, ffxRunDir, version)
	if err != nil {
		return nil, fmt.Errorf("failed to get ffx: %w", err)
	}

	productBundle, err := util.ParseProductBundle(b.productBundleDir)
	if err != nil {
		return nil, err
	}

	blobFS := packages.NewDirBlobStore(
		filepath.Join(b.productBundleDir, productBundle.Repositories[0].BlobsPath),
	)

	// TODO(https://fxbug.dev/42076853): Read delivery blob type from product bundle.
	return packages.NewRepository(ctx, b.productBundleDir, blobFS, ffxTool, nil)
}

func (b *ProductBundleDirBuild) GetPaverDir(ctx context.Context) (string, error) {
	// Only flashing is supported for product bundles
	return "", fmt.Errorf("paving is not supported with product bundles")
}

func (b *ProductBundleDirBuild) GetPaver(
	ctx context.Context,
	sshPublicKey ssh.PublicKey,
) (paver.Paver, error) {
	// Only flashing is supported for product bundles
	return nil, fmt.Errorf("paving is not supported with product bundles")
}

func (b *ProductBundleDirBuild) GetVbmetaPath(ctx context.Context) (string, error) {
	productBundle, err := util.ParseProductBundle(b.productBundleDir)
	if err != nil {
		return "", err
	}
	return productBundle.GetSystemAImage("vbmeta", "zircon-a")
}

type OmahaBuild struct {
	build     Build
	omahatool *omaha_tool.OmahaTool
	avbtool   *avb.AVBTool
	zbitool   *zbi.ZBITool
}

func NewOmahaBuild(
	build Build,
	omahatool *omaha_tool.OmahaTool,
	avbtool *avb.AVBTool,
	zbitool *zbi.ZBITool,
) *OmahaBuild {
	return &OmahaBuild{
		build:     build,
		omahatool: omahatool,
		avbtool:   avbtool,
		zbitool:   zbitool,
	}
}

func (b *OmahaBuild) String() string {
	return b.build.String()
}

func (b *OmahaBuild) OutputDir() string {
	return b.build.OutputDir()
}

func (b *OmahaBuild) GetBootserver(ctx context.Context) (string, error) {
	return b.build.GetBootserver(ctx)
}

func (b *OmahaBuild) GetFfx(
	ctx context.Context,
	ffxRunDir ffx.RunDir,
	version ffx.FfxVersionPolicy,
) (*ffx.FFXTool, error) {
	return b.build.GetFfx(ctx, ffxRunDir, version)
}

type versionedFlashManifest struct {
	Version  int           `json:"version"`
	Manifest flashManifest `json:"manifest"`
}

type flashManifest struct {
	Credentials []string       `json:"credentials,omitempty"`
	HwRevision  string         `json:"hw_revision"`
	Products    []flashProduct `json:"products,omitempty"`
}

type flashProduct struct {
	BootloaderPartitions []flashPartition `json:"bootloader_partitions,omitempty"`
	Name                 string           `json:"name"`
	Partitions           []flashPartition `json:"partitions,omitempty"`
	RequiresUnlock       bool             `json:"requires_unlock"`
}

type flashPartition struct {
	Name      string          `json:"name"`
	Path      string          `json:"path"`
	Condition *flashCondition `json:"condition,omitempty"`
}

type flashCondition struct {
	Value    string `json:"value"`
	Variable string `json:"variable"`
}

func (b *OmahaBuild) GetFlashManifest(ctx context.Context) (string, error) {
	flashManifestPath, err := b.build.GetFlashManifest(ctx)
	if err != nil {
		return "", err
	}

	fileInfo, err := os.Stat(flashManifestPath)
	if err != nil {
		return "", err
	}

	// Error out if we're dealing with a product bundle build.
	if fileInfo.IsDir() {
		return "", fmt.Errorf(
			"flashing product bundles with omaha builds currently not supported",
		)
	}

	f, err := os.Open(flashManifestPath)
	if err != nil {
		return "", err
	}
	defer f.Close()

	var manifest versionedFlashManifest
	decoder := json.NewDecoder(f)
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&manifest); err != nil {
		return "", err
	}

	if manifest.Version != 3 {
		return "", fmt.Errorf("Unknown flash manifest version %d", manifest.Version)
	}

	srcVbmetaPath, err := b.GetVbmetaPath(ctx)
	if err != nil {
		return "", fmt.Errorf("failed to find zircon-a vbmeta: %w", err)
	}

	paverDir, err := b.GetPaverDir(ctx)
	if err != nil {
		return "", err
	}

	destVbmetaPath := filepath.Join(paverDir, "zircon-a-omaha-test.vbmeta")

	if err := b.updateVBMeta(ctx, srcVbmetaPath, destVbmetaPath); err != nil {
		return "", err
	}

	// Update the manifest to point at the new vbmeta.
	for i, product := range manifest.Manifest.Products {
		if product.Name == "fuchsia" || product.Name == "fuchsia_only" {
			for j, partition := range product.Partitions {
				if partition.Name == "vbmeta_a" || partition.Name == "vbmeta_b" {
					manifest.Manifest.Products[i].Partitions[j].Path = destVbmetaPath
				}
			}
		}
	}

	// Write out the manifest to a new file and return it..
	updatedFlashManifest := filepath.Join(paverDir, "flash-omaha-test.json")
	f, err = os.Create(updatedFlashManifest)
	if err != nil {
		return "", err
	}
	defer f.Close()

	encoder := json.NewEncoder(f)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(&manifest); err != nil {
		return "", err
	}

	return updatedFlashManifest, nil
}

func (b *OmahaBuild) GetProductBundleDir(ctx context.Context) (string, error) {
	productBundleDir, err := b.build.GetProductBundleDir(ctx)
	if err != nil {
		return "", fmt.Errorf("failed to get product bundle dir for %s: %w", b, err)
	}

	// We need to copy the product bundle so we can overwrite the vbmeta to
	// inject the omaha url into the vbmeta.
	newProductBundleDir := filepath.Join(b.OutputDir(), "product_bundle_ota_test")

	if err := util.CopyDir(ctx, newProductBundleDir, productBundleDir); err != nil {
		return "", err
	}

	productBundle, err := util.ParseProductBundle(newProductBundleDir)
	if err != nil {
		return "", err
	}

	srcVbmetaPath, err := productBundle.GetSystemAImage("vbmeta", "zircon-a")
	if err != nil {
		return "", err
	}

	relativeDestVbmetaPath := "zircon-a-omaha-test.vbmeta"
	destVbmetaPath := filepath.Join(newProductBundleDir, relativeDestVbmetaPath)

	if err := b.updateVBMeta(ctx, srcVbmetaPath, destVbmetaPath); err != nil {
		return "", err
	}

	for i, image := range productBundle.SystemA {
		if image.Type == "vbmeta" && image.Name == "zircon-a" {
			productBundle.SystemA[i].Path = relativeDestVbmetaPath
		}
	}

	if err := util.UpdateProductBundle(newProductBundleDir, productBundle); err != nil {
		return "", err
	}

	return newProductBundleDir, nil
}

func (b *OmahaBuild) updateVBMeta(
	ctx context.Context,
	srcVbmetaPath string,
	destVbmetaPath string,
) error {
	// Create a ZBI with the omaha_url argument.
	tempDir, err := os.MkdirTemp("", "")
	if err != nil {
		return fmt.Errorf("failed to create temp directory: %w", err)
	}
	defer os.RemoveAll(tempDir)

	// Create a ZBI with the omaha_url argument.
	destZbiPath := path.Join(tempDir, "omaha_argument.zbi")
	imageArguments := map[string]string{
		"omaha_url":    b.omahatool.URL(),
		"omaha_app_id": b.omahatool.Args.AppId,
		"ota_channel":  "ota-test-channel",
	}

	if err := b.zbitool.MakeImageArgsZbi(ctx, destZbiPath, imageArguments); err != nil {
		return fmt.Errorf("Failed to create ZBI: %w", err)
	}

	// Create a vbmeta that includes the ZBI we just created.
	propFiles := map[string]string{
		"zbi": destZbiPath,
	}

	err = b.avbtool.MakeVBMetaImage(ctx, destVbmetaPath, srcVbmetaPath, propFiles)
	if err != nil {
		return fmt.Errorf("failed to create vbmeta: %w", err)
	}

	return nil
}

// GetPackageRepository returns a Repository for this build.
func (b *OmahaBuild) GetPackageRepository(
	ctx context.Context,
	blobFetchMode BlobFetchMode,
	ffxRunDir ffx.RunDir,
	version ffx.FfxVersionPolicy,
	hostFfx *ffx.FFXTool,
) (*packages.Repository, error) {
	return b.build.GetPackageRepository(ctx, blobFetchMode, ffxRunDir, version, hostFfx)
}

func (b *OmahaBuild) GetPaverDir(ctx context.Context) (string, error) {
	return b.build.GetPaverDir(ctx)
}

// GetPaver downloads and returns a paver for the build.
func (b *OmahaBuild) GetPaver(
	ctx context.Context,
	sshPublicKey ssh.PublicKey,
) (paver.Paver, error) {
	paverDir, err := b.GetPaverDir(ctx)
	if err != nil {
		return nil, err
	}
	bootserverPath, err := b.GetBootserver(ctx)
	if err != nil {
		return nil, err
	}

	srcVbmetaPath, err := b.GetVbmetaPath(ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to find zircon-a vbmeta: %w", err)
	}

	destVbmetaPath := filepath.Join(paverDir, "zircon-a-omaha-test.vbmeta")
	if err := b.updateVBMeta(ctx, srcVbmetaPath, destVbmetaPath); err != nil {
		return nil, err
	}

	return paver.NewBuildPaver(
		bootserverPath,
		paverDir,
		paver.SSHPublicKey(sshPublicKey),
		paver.OverrideVBMetaA(destVbmetaPath),
	)
}

func (b *OmahaBuild) GetVbmetaPath(ctx context.Context) (string, error) {
	return b.build.GetVbmetaPath(ctx)
}
