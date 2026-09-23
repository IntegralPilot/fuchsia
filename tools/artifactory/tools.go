// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifactory

import (
	"fmt"
	"path"
	"path/filepath"

	"go.fuchsia.dev/fuchsia/tools/build"
)

var (
	toolsToUpload = map[string]string{
		"zbi":      "zbi",
		"fvm":      "fvm",
		"blobfs":   "blobfs",
		"encipher": "encipher",
		// TODO(https://fxbug.dev/42114276): We can remove this different destination
		// name once the go bootserver has replaced the old c bootserver
		// and is called bootserver instead of bootserver_new.
		"bootserver_new": "bootserver",
		"ffx":            "ffx",
		"ffx-package":    "ffx-package",
		"device-finder":  "device-finder",
		"pbtool":         "pbtool",
		"symbolize":      "symbolize",
		// Undercoat is used by fuzzing infrastructure.
		"undercoat": "undercoat",
		// Used by release-updater-processor.
		"fastboot": "fastboot",
	}
)

// ToolUploads returns a set of tools to upload.
func ToolUploads(mods *build.Modules, namespace string) []Upload {
	return toolUploads(mods, toolsToUpload, namespace)
}

func toolUploads(mods toolModules, allowlist map[string]string, namespace string) []Upload {
	var uploads []Upload
	for _, tool := range mods.Tools() {
		if _, ok := allowlist[tool.Name]; !ok {
			continue
		}
		uploads = append(uploads, Upload{
			Source:      filepath.Join(mods.BuildDir(), tool.Path),
			Destination: path.Join(namespace, fmt.Sprintf("%s-%s", tool.OS, tool.CPU), allowlist[tool.Name]),
			// Tools should be signed for release builds.
			Signed: true,
		})
	}
	return uploads
}

type toolModules interface {
	BuildDir() string
	Tools() build.Tools
}
