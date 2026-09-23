// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

// ProductBundle represents an entry in the product bundles build api.
type ProductBundle struct {
	// Label is the GN label for the product bundle.
	Label string `json:"label,omitempty"`

	// Name is the name of the product bundle, given by <product.board> in the
	// case of the main product bundle.
	Name string `json:"name"`

	// CPU is the target CPU architecture of the associated images.
	CPU string `json:"cpu"`

	// Path is the path to the product bundle directory.
	Path string `json:"path,omitempty"`

	// A unique version for this <product.board>.
	ProductVersion string `json:"product_version"`

	// Local path to the transfer manifest for the product bundle.
	TransferManifestPath string `json:"transfer_manifest_path,omitempty"`

	// The URL to the transfer manifest for the product bundle. i.e. it will
	// have a "file://" prefix if the file is local.
	TransferManifestUrl string `json:"transfer_manifest_url"`

	// Json is the path (relative to the build directory) of the JSON
	// representation of the product bundle.
	Json string `json:"json"`
}

// ProductBundlesManifest is a JSON list of product bundles produced by the
// Fuchsia build.
type ProductBundlesManifest = []ProductBundle

// GetPbPathByName gets the path to a product bundle based on name
func GetPbPathByName(pbs []ProductBundle, name string) string {
	for _, pb := range pbs {
		if pb.Name == name {
			return pb.Path
		}
	}
	return ""
}
