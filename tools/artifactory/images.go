// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifactory

import (
	"fmt"
	"path"
	"path/filepath"

	"go.fuchsia.dev/fuchsia/tools/build"
)

const (

	// elfSizesName is the canonical expected name of ELF sizes JSON file.
	elfSizesName = "elf_sizes.json"
)

// ImageUploads parses the image manifest located in the build and returns a
// list of Uploads for the images used for testing.
func ImageUploads(mods *build.Modules, namespace string) ([]Upload, error) {
	return imageUploads(mods, namespace)
}

func imageUploads(mods imgModules, namespace string) ([]Upload, error) {
	manifestName := filepath.Base(mods.ImageManifest())

	files := []Upload{
		{
			Source:      mods.ImageManifest(),
			Destination: path.Join(namespace, manifestName),
			Signed:      true,
		},
	}

	// The same image might appear in multiple entries.
	seen := make(map[string]struct{})
	var elfSizesPath string
	for _, img := range mods.Images() {
		if _, ok := seen[img.Path]; ok {
			continue
		}
		seen[img.Path] = struct{}{}

		switch img.Name {
		case elfSizesName:
			if elfSizesPath != "" {
				return nil, fmt.Errorf("found multiple elf_sizes.json, this is unexpected, fix this by including only one elf_sizes.json target in the build graph: %s, %s", elfSizesPath, img.Path)
			}
			elfSizesPath = img.Path
			// Upload elf_sizes.json to the root of images directory, so it's easily
			// accessible in GCS.
			files = append(files, Upload{
				Source:      filepath.Join(mods.BuildDir(), elfSizesPath),
				Destination: path.Join(namespace, elfSizesName),
			})

		}
	}
	return files, nil
}

type imgModules interface {
	BuildDir() string
	Images() []build.Image
	ImageManifest() string
}
