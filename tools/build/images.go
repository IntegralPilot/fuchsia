// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

// Select values of Image.Type.
const (
	ImageTypeFAT        string = "fat"
	ImageTypeQEMUKernel string = "kernel"
	ImageTypeZBI        string = "zbi"
	ImageTypeVBMeta     string = "vbmeta"
	ImageTypeBlk        string = "blk"
	ImageTypeFxfsBlk    string = "fxfs-blk"
)

// Image represents an entry in an image manifest.
type Image struct {
	// Name is the canonical name of the image.
	Name string `json:"name"`

	// CPU is the target arch this image was built for.
	CPU string `json:"cpu"`

	// Path is the path to the image within the build directory.
	Path string `json:"path"`

	// Label is the GN label of the image.
	Label string `json:"label"`

	// Type is the shorthand for the type of the image (e.g., "zbi" or "blk").
	Type string `json:"type"`

	// NetbootArgs is the list of associated arguments to pass to the bootserver
	// when netbooting.
	NetbootArgs []string `json:"bootserver_netboot,omitempty"`

	// FastbootBootArgs is the list of arguments to pass to fastboot boot.
	FastbootBootArgs []string `json:"fastboot_boot, omitempty"`

	// FastbootFlashArgs is the list of associated arguments to pass to fastboot
	// flash.
	FastbootFlashArgs []string `json:"fastboot_flash, omitempty"`
}

// ImageManifest is a JSON list of images produced by the Fuchsia build.
type ImageManifest = []Image
