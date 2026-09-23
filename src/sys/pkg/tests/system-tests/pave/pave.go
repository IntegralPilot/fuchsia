// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package pave

import (
	"context"
	"fmt"
	"os"
	"time"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/artifacts"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/device"
	ffxpkg "go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"golang.org/x/crypto/ssh"
)

func PaveDevice(
	ctx context.Context,
	d *device.Client,
	ffxTool *ffxpkg.FFXTool,
	build artifacts.Build,
	sshPublicKey ssh.PublicKey,
	version ffxpkg.FfxVersionPolicy,
) error {
	logger.Infof(ctx, "Starting to pave device")
	startTime := time.Now()

	// Fetch the FFX tool associated with the build we are paving to use for reconnection after the device reboots.
	nextFfxTool, err := build.GetFfx(ctx, ffxTool.RunDir(), version)
	if err != nil {
		return fmt.Errorf("failed to get ffx from build: %w", err)
	}

	if err := d.Pave(ctx, build, sshPublicKey); err != nil {
		return fmt.Errorf("device failed to pave: %w", err)
	}

	if err := d.Reconnect(ctx, nextFfxTool); err != nil {
		return fmt.Errorf("device failed to connect after pave: %w", err)
	}

	logger.Infof(ctx, "device booted")
	logger.Infof(ctx, "Paving successful in %s", time.Now().Sub(startTime))

	startTime = time.Now()
	cmd := []string{"/bin/update", "wait-for-commit"}
	if err := d.Run(ctx, cmd, os.Stdout, os.Stderr); err != nil {
		return fmt.Errorf("update wait-for-commit failed after pave: %w", err)
	}
	logger.Infof(ctx, "Commit successful in %s", time.Now().Sub(startTime))

	return nil
}
