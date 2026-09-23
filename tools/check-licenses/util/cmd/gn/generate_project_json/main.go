// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This binary runs "fx gn gen" to generate a project.json file.
//
// Once complete, it copies the project.json file to a location defined
// by "--gen_output".
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/util"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/util/cmd"
)

var (
	gnPath          = flag.String("gn_path", "", "Path to GN executable. Required when target is specified.")
	buildDir        = flag.String("build_dir", os.Getenv("FUCHSIA_BUILD_DIR"), "Location of GN build directory.")
	genOutput       = flag.String("gen_output", "", "Location to save intermediate GN gen output.")
	stampOutput     = flag.String("stamp_output", "", "Location to save stamp file.")
	alwaysRunGnDesc = flag.Bool("always_run_gn_desc", true, "Run 'fx gn desc' even if a project.json file already exists.")
)

func main() {
	flag.Parse()

	if *gnPath == "" {
		cmd.Exit(fmt.Errorf("--gn_path must be provided."))
	}
	if *buildDir == "" {
		cmd.Exit(fmt.Errorf("--build_dir must be provided."))
	}
	if *genOutput == "" {
		cmd.Exit(fmt.Errorf("--gen_output must be provided."))
	}
	if *stampOutput == "" {
		cmd.Exit(fmt.Errorf("--stamp_output must be provided."))
	}

	projectJson := filepath.Join(*buildDir, "project.json")

	_, err := os.Stat(projectJson)
	projectJsonExists := err == nil

	if shouldRegenerateProjectJson(projectJson) {

		gn, err := util.NewGn(*gnPath, *buildDir)
		if err != nil {
			cmd.Exit(err)
		}

		if *alwaysRunGnDesc || !projectJsonExists {
			if err := gn.GenerateProjectFile(context.Background()); err != nil {
				cmd.Exit(err)
			}
		}

		if err := cmd.CopyFile(projectJson, *genOutput); err != nil {
			cmd.Exit(fmt.Errorf("error copying project file %s -> %s: %w", projectJson, *genOutput, err))
		}

		// Save stamp file
		cmd.Exit(cmd.SaveFile([]byte{}, *stampOutput))
	}
}

func shouldRegenerateProjectJson(projectJson string) bool {
	// Ensure the input file exists.
	origStats, err := os.Stat(projectJson)
	if err != nil {
		// If it doesn't exist, we need to generate it.
		return true
	}

	// Ensure we already have a copy of the file.
	copyStats, err := os.Stat(*genOutput)
	if err != nil {
		// If we don't have a copy of the file, we should regenerate it.
		return true
	}

	// Regenerate the file if the root file is newer than our copy.
	if copyStats.ModTime().After(origStats.ModTime()) {
		return true
	}

	// Regenerate the file if it is more than 1 hour old.
	return copyStats.ModTime().Before(time.Now().Add(-time.Hour * 1))
}
