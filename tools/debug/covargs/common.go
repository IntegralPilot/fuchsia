// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package covargs

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

func SplitVersion(arg string) (version, value string) {
	version = ""
	s := strings.SplitN(arg, "=", 2)
	if len(s) > 1 {
		version = s[1]
	}
	return version, s[0]
}

func MergeSameVersionProfiles(ctx context.Context, tempDir string, profiles []string, outputFile, llvmProfData, version string, numThreads int, debuginfodServers []string, debuginfodCache string, buildIDDirs []string) error {
	// Make the llvm-profdata response file.
	profdataFile, err := os.Create(filepath.Join(tempDir, fmt.Sprintf("llvm-profdata%s.rsp", version)))
	if err != nil {
		return fmt.Errorf("creating llvm-profdata.rsp file: %w", err)
	}

	for _, profile := range profiles {
		fmt.Fprintf(profdataFile, "%s\n", profile)
	}
	profdataFile.Close()

	// Merge all raw profiles.
	args := []string{
		"merge",
		"--sparse",
		"--output", outputFile,
	}

	useDebugInfod := false
	// Use debuginfod during profile merging if it is enabled.
	if len(debuginfodServers) > 0 {
		useDebugInfod = true
		// TODO(https://fxbug.dev/368375861): Switch to --failure-mode=any by default when missing build id issue is resolved.
		args = append(args, "--debuginfod", "--correlate=binary", "--failure-mode=all")
	} else if len(buildIDDirs) > 0 {
		for _, dir := range buildIDDirs {
			// llvm-profdata expects the provided directory to contain a .build-id subdirectory.
			args = append(args, "--debug-file-directory", filepath.Dir(dir))
		}
		// TODO(https://fxbug.dev/507307138): Switch to --failure-mode=any by default when unable to merge issue is solved in some binaries
		args = append(args, "--correlate=binary", "--failure-mode=all")
	} else {
		args = append(args, "--failure-mode=any")
	}

	if numThreads != 0 {
		args = append(args, "--num-threads", strconv.Itoa(numThreads))
	}
	args = append(args, "@"+profdataFile.Name())

	mergeCmd := exec.Command(llvmProfData, args...)
	logger.Debugf(ctx, "%s\n", mergeCmd)
	if useDebugInfod {
		if err := setDebuginfodEnv(mergeCmd, debuginfodServers, debuginfodCache); err != nil {
			return fmt.Errorf("failed to set debuginfod environment: %w", err)
		}
	}
	data, err := mergeCmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("%s failed with %v:\n%s", mergeCmd.String(), err, string(data))
	}
	return nil
}

// setDebuginfodEnv sets the environment variables for debuginfod.
func setDebuginfodEnv(cmd *exec.Cmd, debuginfodServers []string, debuginfodCache string) error {
	if len(debuginfodServers) > 0 {
		if debuginfodCache == "" {
			return fmt.Errorf("-debuginfod-cache must be set if debuginfod server is used")
		}

		debuginfodUrls := strings.Join(debuginfodServers, " ")
		cmd.Env = os.Environ()
		// Set DEBUGINFOD_URLS environment variable that is a string of a space-separated URLs.
		cmd.Env = append(cmd.Env, "DEBUGINFOD_URLS="+debuginfodUrls)
		cmd.Env = append(cmd.Env, "DEBUGINFOD_CACHE_PATH="+debuginfodCache)
	}

	return nil
}
