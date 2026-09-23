// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package cli

import (
	"context"
	"flag"
	"os"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

type FfxConfig struct {
	ffxPath               string
	ffxRunDirPath         string
	ffxSubtoolsSearchPath string
	ffxLogLevel           string
	ffx                   *ffx.FFXTool
}

func NewFfxConfig(fs *flag.FlagSet) *FfxConfig {
	c := &FfxConfig{}
	fs.StringVar(&c.ffxPath, "ffx-path", "host-tools/ffx", "ffx tool path")
	fs.StringVar(&c.ffxRunDirPath, "ffx-run-dir", "", "ffx run dir path")
	fs.StringVar(&c.ffxSubtoolsSearchPath, "ffx-subtools-search-path", "", "ffx subtools search path")
	fs.StringVar(&c.ffxLogLevel, "ffx-log-level", "trace", "ffx log level (default: trace)")

	return c
}

func (c *FfxConfig) Validate() error {
	for _, s := range []string{
		c.ffxPath,
		c.ffxRunDirPath,
	} {
		if err := util.ValidatePath(s); err != nil {
			return err
		}
	}
	return nil
}

func (c *FfxConfig) LogLevel() string {
	return c.ffxLogLevel
}

func (c *FfxConfig) NewFfxTool(ctx context.Context, sshPrivateKeyPath string) (*ffx.FFXTool, func(), error) {
	var outputDir string
	var cleanupDir func()
	var err error
	if c.ffxRunDirPath == "" {
		outputDir, err = os.MkdirTemp("", "ffx-output-dir")
		if err != nil {
			return nil, func() {}, err
		}
		cleanupDir = func() {
			os.RemoveAll(outputDir)
		}
	} else {
		outputDir = c.ffxRunDirPath
		cleanupDir = func() {}
	}

	runDir := ffx.NewRunDirWithPrivKey(outputDir, sshPrivateKeyPath)
	ffxTool, err := ffx.NewFFXToolForVersion(ctx, c.ffxPath, runDir, ffx.FfxVersionPolicyLatest, c.ffxSubtoolsSearchPath)
	if err != nil {
		cleanupDir()
		return nil, func() {}, err
	}
	if c.ffxLogLevel != "" {
		ffxTool.SetLogLevel(c.ffxLogLevel)
	}

	return ffxTool, func() {
		if err := ffxTool.Close(ctx); err != nil {
			logger.Warningf(ctx, "failed to close ffx tool: %v", err)
		}
		cleanupDir()
	}, nil
}
