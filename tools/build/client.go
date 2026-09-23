// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// BuildAPIClient is a convenience interface for accessing the build API module
// files from the build system, using the //build/api/client script.
type BuildAPIClient struct {
	buildDir string
	toolPath string
}

// NewBuildAPIClient returns a BuildAPIClient associated with a given build
// directory. Note that `gn gen` must be run once to generate the
// $buildDir/build_api_client_path file which will be used to locate
// the script from the checkout directory.
func NewBuildAPIClient(buildDir string) (*BuildAPIClient, error) {
	relativePath, err := os.ReadFile(filepath.Join(buildDir, "build_api_client_path"))
	if err != nil {
		return nil, err
	}
	toolPath := filepath.Join(buildDir, string(relativePath))
	if _, err := os.Stat(toolPath); err != nil {
		return nil, err
	}
	c := &BuildAPIClient{buildDir, toolPath}
	return c, nil
}

// GetRaw returns the content of a build API module file as a raw string.
func (c BuildAPIClient) GetRaw(name string) ([]byte, error) {
	cmd := exec.Command(c.toolPath, "--build-dir", c.buildDir, "print", name)
	cmd.Stderr = os.Stderr
	output, err := cmd.Output()
	if err != nil {
		log.Fatal(err)
	}
	return output, err
}

// GetJson reads build API module file as JSON.
func (c BuildAPIClient) GetJSON(name string, v any) error {
	content, err := c.GetRaw(name)
	if err != nil {
		return err
	}
	return json.Unmarshal(content, v)
}

func (c BuildAPIClient) GetModulePaths() ([]string, error) {
	var paths []string
	cmd := exec.Command(c.toolPath, "--build-dir", c.buildDir, "print_all", "--pretty")
	cmd.Stderr = os.Stderr
	output, err := cmd.Output()
	if err != nil {
		return paths, err
	}
	var buildAPIModules map[string]struct {
		File string `json:"file"`
	}
	if err := json.Unmarshal(output, &buildAPIModules); err != nil {
		return paths, err
	}
	for _, module := range buildAPIModules {
		paths = append(paths, module.File)
	}
	return paths, nil
}

func (c BuildAPIClient) ExportDebugSymbols(ctx context.Context, outputDir string, withBreakpad bool) error {
	args := []string{
		"--build-dir",
		c.buildDir,
		"export_last_build_debug_symbols",
		fmt.Sprintf("--output-dir=%s", outputDir),
	}
	if withBreakpad {
		args = append(args, "--with-breakpad-symbols")
	}
	cmd := exec.CommandContext(ctx, c.toolPath, args...)
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("export_last_build_debug_symbols failed: %w", err)
	}
	return nil
}

// AffectedTests returns the list of affected test targets by calling the
// build/api/client affected_tests command with the given files list.
func (c BuildAPIClient) AffectedTests(ctx context.Context, filesListPath string) ([]string, error) {
	args := []string{
		"--build-dir",
		c.buildDir,
		"affected_tests",
		fmt.Sprintf("--files-list=%s", filesListPath),
	}
	cmd := exec.CommandContext(ctx, c.toolPath, args...)
	cmd.Stderr = os.Stderr
	output, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("affected_tests failed: %w", err)
	}
	var tests []string
	for _, line := range strings.Split(string(output), "\n") {
		line = strings.TrimSpace(line)
		if line != "" {
			tests = append(tests, line)
		}
	}
	return tests, nil
}
