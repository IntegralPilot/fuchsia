// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package util

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
)

type Gn struct {
	gnPath string
	outDir string

	re *regexp.Regexp
}

// NewGn returns a GN object that is used to interface with the external GN
// tool. It can be used to discover the dependendcies of a GN target. The path
// to the external binary is taken from the command line argument (--gn_path).
// NewGn will return an error if gnPath is not a valid executable, or if
// --build_dir does not exist.
func NewGn(gnPath, buildDir string) (*Gn, error) {
	gn := &Gn{
		// Many rust_crate projects have a suffix in the label name that
		// doesn't map to a directory. We use a regular expression to
		// strip that part of the label text away. We store the regexp
		// in this GN struct so we don't have to recompile the regex on
		// each loop.
		re: regexp.MustCompile(`-v\d_\d+_\d+`),
	}

	path, err := exec.LookPath(gnPath)
	if err != nil {
		return nil, fmt.Errorf("Failed to find GN binary at path %v: %v", gnPath, err)
	}

	if _, err := os.Stat(buildDir); os.IsNotExist(err) {
		return nil, fmt.Errorf("out directory does not exist: %s", buildDir)
	}

	gn.gnPath = path
	gn.outDir = buildDir

	return gn, nil
}

func (gn *Gn) GenerateProjectFile(ctx context.Context) error {
	args := []string{
		"gen",
		gn.outDir,
		"--all",
		"--ide=json",
	}

	cmd := exec.CommandContext(ctx, gn.gnPath, args...)
	var output bytes.Buffer
	cmd.Stdout = &output
	cmd.Stderr = os.Stderr
	err := cmd.Run()
	if err != nil {
		return fmt.Errorf("util.GenerateProjectFile: error running command %v: %w", cmd, err)
	}
	projectJsonFile := filepath.Join(gn.outDir, "project.json")
	if _, err := os.Stat(projectJsonFile); os.IsNotExist(err) {
		return fmt.Errorf("util.GenerateProjectFile: project.json file was not generated: %w", err)
	}
	return nil
}
