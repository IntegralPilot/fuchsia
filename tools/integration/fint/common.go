// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fint

import (
	"context"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"

	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
)

type subprocessRunner interface {
	Run(ctx context.Context, cmd []string, options subprocess.RunOptions) error
}

// thirdPartyPrebuilt returns the absolute path to a platform-specific prebuilt
// in the //prebuilt/third_party subdirectory of the checkout.
func thirdPartyPrebuilt(checkoutDir, platform, name string) string {
	return filepath.Join(checkoutDir, "prebuilt", "third_party", name, platform, name)
}

// makeAbsolute takes a root directory and a list of relative paths of files
// within that directory, and returns a list of absolute paths to those files.
func makeAbsolute(rootDir string, paths []string) []string {
	var res []string
	for _, path := range paths {
		res = append(res, filepath.Join(rootDir, path))
	}
	return res
}

// saveLogs writes the given set of logs to files in the artifact directory,
// and adds each path to the output artifacts.
func saveLogs(artifactDir string, artifacts *fintpb.BuildArtifacts, logs map[string]string) error {
	if artifactDir == "" {
		return nil
	}
	if artifacts.LogFiles == nil {
		artifacts.LogFiles = make(map[string]string)
	}
	for name, contents := range logs {
		dest := filepath.Join(
			artifactDir,
			url.QueryEscape(strings.ReplaceAll(name, " ", "_")))
		f, err := osmisc.CreateFile(dest)
		if err != nil {
			return err
		}
		defer f.Close()
		if _, err := f.WriteString(contents); err != nil {
			return fmt.Errorf("failed to write log file %q: %w", name, err)
		}
		artifacts.LogFiles[name] = f.Name()
	}
	return nil
}

func checkFileExists(filePath string) bool {
	_, error := os.Stat(filePath)
	return !errors.Is(error, os.ErrNotExist)
}

// newRunner returns a subprocess.Runner with PYTHONPYCACHEPREFIX set to a
// directory within the build directory, to avoid writing to the source tree.
func newRunner(contextSpec *fintpb.Context) *subprocess.Runner {
	runner := &subprocess.Runner{}
	if contextSpec != nil && contextSpec.BuildDir != "" && os.Getenv("PYTHONPYCACHEPREFIX") == "" {
		runner.Env = append(runner.Env, fmt.Sprintf("PYTHONPYCACHEPREFIX=%s", filepath.Join(contextSpec.BuildDir, "__pycache__")))
	}
	return runner
}
