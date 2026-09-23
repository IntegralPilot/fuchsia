// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testsharder

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

const (
	productBundlesManifest = "product_bundles.json"
)

// for testability
type FFXInterface interface {
	Run(context.Context, ...string) error
	GetPBArtifacts(context.Context, string, string) ([]string, error)
	Stop() error
}

var GetFFX = func(ctx context.Context, ffxPath, outputsDir string) (FFXInterface, error) {
	return ffxutil.NewFFXInstance(ctx, ffxPath, "", []string{}, "", nil, outputsDir, ffxutil.UseFFXLegacy)
}

// AddImageDeps selects and adds the subset of images needed by a shard to
// that shard's list of dependencies.
func AddImageDeps(ctx context.Context, s *Shard, buildDir string, pbPath, ffxPath string) error {
	// Host-test only shards do not require any image deps because they are not running
	// against a Fuchsia target.
	if s.Env.Dimensions.DeviceType() == "" {
		return nil
	}
	// GCE test shards do not require any image deps as the build creates a
	// compute image with all the deps baked in.
	if s.Env.Dimensions.DeviceType() == "GCE" {
		return nil
	}

	// Add product bundle related artifacts.
	imageDeps := []string{productBundlesManifest}

	tmp, err := os.MkdirTemp("", "wt")
	if err != nil {
		return err
	}
	defer os.RemoveAll(tmp)

	ffxOutputsDir := filepath.Join(tmp, "ffx_outputs")
	ffx, err := GetFFX(ctx, ffxPath, ffxOutputsDir)
	if err != nil {
		return err
	}
	if ffx == nil {
		return fmt.Errorf("failed to initialize an ffx instance")
	}
	defer func() {
		if err := ffx.Stop(); err != nil {
			logger.Debugf(ctx, "failed to stop ffx: %s", err)
		}
	}()

	if err := ffx.Run(ctx, "config", "set", "daemon.autostart", "false"); err != nil {
		return err
	}
	artifactsGroup := "flash"
	if s.Env.TargetsEmulator() {
		artifactsGroup = "emu"
	}
	artifacts, err := ffx.GetPBArtifacts(ctx, filepath.Join(buildDir, pbPath), artifactsGroup)
	if err != nil {
		return err
	}
	for _, a := range artifacts {
		imageDeps = append(imageDeps, filepath.Join(pbPath, a))
	}
	bootloaderArtifacts, err := ffx.GetPBArtifacts(ctx, filepath.Join(buildDir, pbPath), "bootloader")
	if err != nil {
		return err
	}
	for _, a := range bootloaderArtifacts {
		parts := strings.SplitN(a, ":", 2)
		if parts[0] == "firmware_efi-shell" {
			imageDeps = append(imageDeps, filepath.Join(pbPath, parts[1]))
		}
	}

	s.AddDeps(imageDeps)
	return nil
}
