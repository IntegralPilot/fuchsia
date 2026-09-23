// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"fmt"

	"go.fuchsia.dev/fuchsia/tools/lib/hostplatform"
)

// Tool represents a host tool in the build.
type Tool struct {
	// Name is the canonical name of the image.
	Name string `json:"name"`

	// Path is relative path to the tool within the build directory.
	Path string `json:"path"`

	// OS is the operating system the tool is meant to run on.
	OS string `json:"os"`

	// CPU is the architecture the tool is meant to run on.
	CPU string `json:"cpu"`

	// RuntimeFiles is a list of files that are also required for the
	// tool to function. They are relative paths to the build directory.
	RuntimeFiles []string `json:"runtime_files"`
}

type Tools []Tool

// LookupTool returns the Tool object corresponding to the named tool built
// for the specified platform. It will return an error if the platform/tool
// combination cannot be found, generally because the platform is not
// supported or because the tool is not listed in tool_paths.json.
func (t Tools) LookupTool(platform, name string) (*Tool, error) {
	for _, tool := range t {
		toolPlatform := hostplatform.MakeName(tool.OS, tool.CPU)
		if name == tool.Name && platform == toolPlatform {
			return &tool, nil
		}
	}
	return nil, fmt.Errorf("no tool with platform %q and name %q", platform, name)
}
