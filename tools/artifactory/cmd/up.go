// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path"

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/artifactory"
	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

const (
	// Relative path within the build directory to the repo produced by a build.
	repoSubpath = "amber-files"
	// Names of the repository metadata, key, blob, and target directories within a repo.
	metadataDirName = "repository"
	keyDirName      = "keys"
	blobDirName     = "blobs"
	targetDirName   = "targets"

	// Names of directories to be uploaded to in GCS.
	assemblyInputArchivesDirName    = "assembly"
	assemblyManifestsDirName        = "assembly_manifests"
	productSizeCheckerOutputDirName = "product_size_checker"
	buildAPIDirName                 = "build_api"
	buildidDirName                  = "buildid"
	debugDirName                    = "debug"
	hostTestDirName                 = "host_tests"
	imageDirName                    = "images"
	licenseDirName                  = "licenses"
	packageDirName                  = "packages"
	sdkArchivesDirName              = "sdk"
	toolDirName                     = "tools"
	productBundleDirName            = "product_bundles"

	// A record of all of the fuchsia debug symbols processed.
	// This is eventually consumed by crash reporting infrastructure.
	// TODO(https://fxbug.dev/42155140): Have the crash reporting infrastructure
	// consume build-ids.json instead.
	buildIDsTxt = "build-ids.txt"

	// A mapping of build ids to binary labels.
	buildIDsToLabelsManifestName = "build-ids.json"

	// The blobs manifest.
	blobManifestName = "all_blobs.json"

	// The delivery blob config.
	deliveryBlobConfigName = "delivery_blob_config.json"

	// A list of all Public Platform Surface Areas.
	ctsPlasaReportName = "test_coverage_report.plasa.json"

	// A mapping of fidl mangled names to api functions.
	fidlMangledToApiMappingManifestName = "ctf_fidl_mangled_to_api_mapping.json"

	// A list of bazel build invocation details.
	// LINT.IfChange(last_bazel_build_invocations_file)
	lastBazelBuildInvocationsName = "last_bazel_build_invocations.json"
	// LINT.ThenChange(//build/bazel/scripts/build_utils.py:last_bazel_build_invocations_file)
)

type upCommand struct {
	// Unique namespace under which to index artifacts.
	namespace string
	// Whether to emit upload manifest JSON to this path instead of executing
	// uploads.
	uploadManifestJSONOutput string
}

func (upCommand) Name() string { return "up" }

func (upCommand) Synopsis() string { return "emit a GCS upload manifest for a build" }

func (upCommand) Usage() string {
	return `
artifactory up -namespace $NAMESPACE <build directory>

Emits a GCS upload manifest for a build with the following structure:

├── $GCS_BUCKET
│   │   ├── assembly
│   │   │   └── <assembly input archives>
│   │   ├── blobs
│   │   │   └── <blob names>
│   │   │   └── <blob type>
│   │   │   │   └── <blob names>
│   │   ├── debug
│   │   │   └── <debug binaries in zxdb format>
│   │   ├── buildid
│   │   │   └── <debug binaries in debuginfod format>
│   │   ├── $NAMESPACE
│   │   │   ├── build-ids.json
│   │   │   ├── build-ids.txt
│   │   │   ├── jiri.snapshot
│   │   │   ├── objs_to_refresh_ttl.txt
│   │   │   ├── publickey.pem
│   │   │   ├── images
│   │   │   │   └── <images>
│   │   │   │   └── transfer.json
│   │   │   │   └── product_bundle
│   │   │   │   └── elf_sizes.json
│   │   │   ├── packages
│   │   │   │   ├── all_blobs.json
│   │   │   │   ├── delivery_blob_config.json
│   │   │   │   ├── repository
│   │   │   │   │   ├── targets
│   │   │   │   │   │   └── <package repo target files>
│   │   │   │   │   └── <package repo metadata files>
│   │   │   │   └── keys
│   │   │   │       └── <package repo keys>
│   │   │   ├── sdk
│   │   │   │   ├── <host-independent SDK archives>
│   │   │   │   └── <OS-CPU>
│   │   │   │       └── <host-specific SDK archives>
│   │   │   ├── build_api
│   │   │   │   └── <build API module JSON>
|   |   |   ├── host_tests
│   │   │   │   └── <host tests and deps, same hierarchy as build dir>
│   │   │   ├── tools
│   │   │   │   └── <OS>-<CPU>
│   │   │   │       └── <tool names>
│   │   │   ├── licenses
│   │   │   │   ├── texts/
│   │   │   │   │   └── <License text files>
│   │   │   │   ├── compliance.csv
│   │   │   │   ├── runfiles.tar.gz (zipped check-licenses configs & artifacts)
│   │   │   │   └── license_review.zip (output of fuchsia_license_review bazel rule)
│   │   │   ├── product_bundles
│   │   │   │   └── <product bundle name>
│   │   │   │       └── <artifacts>

Where $GCS_BUCKET is defined by the infrastructure.

flags:

`
}

func (cmd *upCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&cmd.namespace, "namespace", "", "Namespace under which to index artifacts.")
	f.StringVar(&cmd.uploadManifestJSONOutput, "upload-manifest-json-output", "", "Whether to emit upload manifest to this path instead of executing uploads.")
}

func (cmd upCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...any) subcommands.ExitStatus {
	args := f.Args()
	if len(args) != 1 {
		logger.Errorf(ctx, "exactly one positional argument expected: the build directory root")
		return subcommands.ExitFailure
	}

	if err := cmd.execute(ctx, args[0]); err != nil {
		logger.Errorf(ctx, "%v", err)
		return subcommands.ExitFailure
	}
	return subcommands.ExitSuccess
}

func (cmd upCommand) execute(ctx context.Context, buildDir string) error {
	if cmd.namespace == "" {
		return fmt.Errorf("-namespace is required")
	}
	if cmd.uploadManifestJSONOutput == "" {
		return fmt.Errorf("-upload-manifest-json-output is required")
	}

	m, err := build.NewModules(buildDir)
	if err != nil {
		return err
	}

	repo := path.Join(buildDir, repoSubpath)
	metadataDir := path.Join(repo, metadataDirName)
	keyDir := path.Join(repo, keyDirName)
	blobDir := path.Join(metadataDir, blobDirName)
	blobManifestPath := path.Join(buildDir, blobManifestName)
	deliveryBlobConfigPath := path.Join(buildDir, deliveryBlobConfigName)
	targetDir := path.Join(metadataDir, targetDirName)
	packageNamespaceDir := path.Join(cmd.namespace, packageDirName)
	imageNamespaceDir := path.Join(cmd.namespace, imageDirName)
	licenseNamespaceDir := path.Join(cmd.namespace, licenseDirName)
	productBundleDir := path.Join(cmd.namespace, productBundleDirName)

	uploads := []artifactory.Upload{
		{
			Source:      metadataDir,
			Destination: path.Join(packageNamespaceDir, metadataDirName),
			Deduplicate: false,
		},
		{
			Source:      keyDir,
			Destination: path.Join(packageNamespaceDir, keyDirName),
			Deduplicate: false,
		},
		{
			Source:      targetDir,
			Destination: path.Join(packageNamespaceDir, metadataDirName, targetDirName),
			Deduplicate: false,
			Recursive:   true,
		},
		{
			Source:      blobManifestPath,
			Destination: path.Join(packageNamespaceDir, blobManifestName),
			Compress:    true,
		},
		{
			Source:      deliveryBlobConfigPath,
			Destination: path.Join(packageNamespaceDir, deliveryBlobConfigName),
		},
		// Used for CTS test coverage.
		{
			Source:      path.Join(buildDir, fidlMangledToApiMappingManifestName),
			Destination: path.Join(cmd.namespace, fidlMangledToApiMappingManifestName),
		},
		{
			Source:      path.Join(buildDir, ctsPlasaReportName),
			Destination: path.Join(cmd.namespace, ctsPlasaReportName),
		},
		// Used to inspect bazel build invocations and bazel_gn_target_action.py timings.
		{
			Source:      path.Join(buildDir, lastBazelBuildInvocationsName),
			Destination: path.Join(cmd.namespace, lastBazelBuildInvocationsName),
		},
	}

	blobs, err := artifactory.BlobsUploads(blobManifestPath, deliveryBlobConfigPath, blobDir, blobDirName)
	if err != nil {
		return err
	}
	uploads = append(uploads, blobs...)

	images, err := artifactory.ImageUploads(m, imageNamespaceDir)
	if err != nil {
		return err
	}
	uploads = append(uploads, images...)

	licenses, err := artifactory.LicenseUploads(m, licenseNamespaceDir)
	if err != nil {
		return err
	}
	uploads = append(uploads, licenses...)

	// Upload the product bundles.
	pbUploads, err := artifactory.ProductBundle2Uploads(m, blobDirName, productBundleDir)
	if err != nil {
		return err
	}
	if pbUploads != nil {
		uploads = append(uploads, pbUploads...)
	}

	buildAPIs := artifactory.BuildAPIModuleUploads(m, path.Join(cmd.namespace, buildAPIDirName))
	uploads = append(uploads, buildAPIs...)

	assemblyInputArchives := artifactory.AssemblyInputArchiveUploads(m, path.Join(cmd.namespace, assemblyInputArchivesDirName))
	uploads = append(uploads, assemblyInputArchives...)

	sdkArchives := artifactory.SDKArchiveUploads(m, path.Join(cmd.namespace, sdkArchivesDirName))
	uploads = append(uploads, sdkArchives...)

	tools := artifactory.ToolUploads(m, path.Join(cmd.namespace, toolDirName))
	uploads = append(uploads, tools...)

	assemblyManifests := artifactory.AssemblyManifestsUploads(m, path.Join(cmd.namespace, assemblyManifestsDirName))
	uploads = append(uploads, assemblyManifests...)

	productSizeCheckerUploads, err := artifactory.ProductSizeCheckerOutputUploads(m, path.Join(cmd.namespace, productSizeCheckerOutputDirName))
	if err != nil {
		return err
	}
	uploads = append(uploads, productSizeCheckerUploads...)

	debugSymbols, err := artifactory.DebugSymbolUploads(m, cmd.namespace, debugDirName, buildidDirName)
	if err != nil {
		return err
	}
	uploads = append(uploads, debugSymbols...)

	uploads, err = filterNonExistentFiles(ctx, uploads)
	if err != nil {
		return err
	}

	out, err := os.Create(cmd.uploadManifestJSONOutput)
	if err != nil {
		return err
	}
	defer out.Close()
	data, err := json.MarshalIndent(uploads, "", "  ")
	if err != nil {
		return err
	}
	if _, err := out.Write(data); err != nil {
		return err
	}

	if collisions := checkForCollisions(ctx, uploads); len(collisions) > 0 {
		return fmt.Errorf("multiple files point to the same destinations: %v\n"+
			"Colliding entries can be found in the generated upload manifest.", collisions)
	}
	return nil
}

// filterNonExistentFiles filters out files which do not exist. The associated
// artifacts referenced by the build API manifests may not have been created,
// and this is valid.
func filterNonExistentFiles(ctx context.Context, uploads []artifactory.Upload) ([]artifactory.Upload, error) {
	var filtered []artifactory.Upload
	for _, u := range uploads {
		if len(u.Source) != 0 {
			_, err := os.Stat(u.Source)
			if err != nil {
				if os.IsNotExist(err) {
					logger.Infof(ctx, "%s does not exist; skipping upload", u.Source)
					continue
				}
				return nil, err
			}
		}
		filtered = append(filtered, u)
	}
	return filtered, nil
}

func checkForCollisions(ctx context.Context, uploads []artifactory.Upload) []string {
	numUploadsPerDest := make(map[string]int)
	for _, u := range uploads {
		if u.Deduplicate {
			// Deduplicate means collisions are ok, so ignore.
			continue
		}
		numUploadsPerDest[u.Destination] += 1
	}
	var collisions []string
	for dest, numUploads := range numUploadsPerDest {
		if numUploads > 1 {
			collisions = append(collisions, dest)
		}
	}
	return collisions
}
