// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifactory

import (
	"encoding/json"
	"fmt"
	"os"
	"path"
	"path/filepath"

	"go.fuchsia.dev/fuchsia/tools/build"
)

type TransferManifest struct {
	Version string                  `json:"version"`
	Entries []TransferManifestEntry `json:"entries"`
}

type TransferManifestEntry struct {
	Type    string          `json:"type"`
	Local   string          `json:"local"`
	Remote  string          `json:"remote"`
	Entries []ArtifactEntry `json:"entries"`
}

type ArtifactEntry struct {
	Name string `json:"name"`
}

type productBundlesModules interface {
	BuildDir() string
	ProductBundles() []build.ProductBundle
}

// ProductBundle2Uploads parses the product bundle upload manifests, creates
// absolute paths for each artifact by appending the |buildDir|, and sets
// a destination path in GCS inside |outDir|.
func ProductBundle2Uploads(mods *build.Modules, blobsRemote string, productBundleRemote string) ([]Upload, error) {
	return productBundle2Uploads(mods, blobsRemote, productBundleRemote)
}

func productBundle2Uploads(mods productBundlesModules, blobsRemote string, productBundleRemote string) ([]Upload, error) {
	var uploads []Upload
	for _, pb := range mods.ProductBundles() {
		pbRemoteDir := path.Join(productBundleRemote, pb.Name)
		pbUploads, err := uploadsFromProductBundle(mods, pb.TransferManifestPath, blobsRemote, pbRemoteDir)
		if err != nil {
			return nil, err
		}
		uploads = append(uploads, pbUploads...)
	}
	return uploads, nil
}

// Return a list of Uploads that must happen for a specific product bundle
// transfer manifest.
func uploadsFromProductBundle(mods productBundlesModules, transferManifestPath string, blobsRemote string, productBundleRemote string) ([]Upload, error) {
	transferManifestParentPath := filepath.Dir(transferManifestPath)

	data, err := os.ReadFile(path.Join(mods.BuildDir(), transferManifestPath))
	if err != nil {
		// Return an empty list to upload if the build did not produce a
		// transfer manifest.
		if os.IsNotExist(err) {
			return []Upload{}, nil
		} else {
			return nil, fmt.Errorf("failed to read product bundle transfer manifest: %w", err)
		}
	}

	var transferManifest TransferManifest
	err = json.Unmarshal(data, &transferManifest)
	if err != nil {
		return nil, fmt.Errorf("unable to unmarshal product bundle transfer manifest: %w", err)
	}
	if transferManifest.Version != "1" {
		return nil, fmt.Errorf("product bundle transfer manifest must be version 1")
	}

	var uploads []Upload
	var newTransferEntries []TransferManifestEntry
	for _, entry := range transferManifest.Entries {
		remote := ""
		if entry.Type == "files" {
			remote = path.Join(productBundleRemote, entry.Remote)
			for _, artifact := range entry.Entries {
				uploads = append(uploads, Upload{
					Source:      path.Join(mods.BuildDir(), transferManifestParentPath, entry.Local, artifact.Name),
					Destination: path.Join(remote, artifact.Name),
					Signed:      true,
				})
			}
		} else if entry.Type == "blobs" {
			remote = path.Join(blobsRemote, entry.Remote)
			uploads = append(uploads, Upload{
				Source:      path.Join(mods.BuildDir(), transferManifestParentPath, entry.Local),
				Destination: remote,
				Deduplicate: true,
				Signed:      true,
			})
		} else {
			return nil, fmt.Errorf("unrecognized transfer entry type: %s", entry.Type)
		}

		// Modify the `remote` field inside the entry, so that we can upload
		// this transfer manifest and use it to download the artifacts.
		entry.Remote = remote
		newTransferEntries = append(newTransferEntries, entry)
	}

	// Upload the transfer manifest itself so that it can be used for downloading
	// the artifacts.
	transferManifest.Entries = newTransferEntries
	updatedTransferManifest, err := json.MarshalIndent(&transferManifest, "", "  ")
	if err != nil {
		return nil, err
	}
	uploads = append(uploads, Upload{
		Compress: true,
		Contents: updatedTransferManifest,
		// Consumers rely on the manifest being named transfer.json.
		Destination: path.Join(productBundleRemote, "transfer.json"),
		Signed:      true,
	})

	return uploads, nil
}
