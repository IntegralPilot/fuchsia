// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package check

import (
	"context"
	"fmt"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/device"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/packages"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/sl4f"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

// IsDeviceUpToDate checks if the device's /system/meta matches the expected
// system image merkle.
func IsDeviceUpToDate(
	ctx context.Context,
	device *device.Client,
	expectedSystemImage *packages.SystemImagePackage,
) (bool, error) {
	remoteSystemImageMerkle, err := device.GetSystemImageMerkle(ctx)
	if err != nil {
		return false, fmt.Errorf("failed to get system image merkle while checking if device is up to date: %w", err)
	}

	logger.Infof(ctx, "current system image merkle:  %q", remoteSystemImageMerkle)
	logger.Infof(ctx, "expected system image: %q", expectedSystemImage)

	return expectedSystemImage.Merkle() == remoteSystemImageMerkle, nil
}

func determineActiveABRConfig(
	ctx context.Context,
	rpcClient *sl4f.Client,
) (*sl4f.Configuration, error) {
	if rpcClient == nil {
		logger.Infof(ctx, "sl4f not running, cannot determine active partition")
		return nil, nil
	}

	currentConfig, err := rpcClient.PaverQueryActiveConfiguration(ctx)
	if err == sl4f.ErrNotSupported {
		logger.Infof(ctx, "device does not support querying the active configuration")
		return nil, nil
	} else if err != nil {
		return nil, err
	}

	logger.Infof(ctx, "device booted to slot %s", currentConfig)

	return &currentConfig, nil
}

func DetermineCurrentABRConfig(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	device *device.Client,
	repo *packages.Repository,
) (*sl4f.Configuration, error) {
	rpcClient, err := device.StartRpcSession(ctx, ffxTool, repo)
	if err != nil {
		return nil, fmt.Errorf("unable to connect to sl4f: %w", err)
	}

	return determineCurrentABRConfig(ctx, rpcClient)
}

func determineCurrentABRConfig(
	ctx context.Context,
	rpcClient *sl4f.Client,
) (*sl4f.Configuration, error) {
	if rpcClient == nil {
		logger.Infof(ctx, "sl4f not running, cannot determine current partition")
		return nil, nil
	}

	currentConfig, err := rpcClient.PaverQueryCurrentConfiguration(ctx)
	if err == sl4f.ErrNotSupported {
		logger.Infof(ctx, "device does not support querying the current configuration")
		return nil, nil
	} else if err != nil {
		return nil, err
	}

	logger.Infof(ctx, "device booted to slot %s", currentConfig)

	return &currentConfig, nil
}

func checkABRConfig(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	device *device.Client,
	repo *packages.Repository,
	expectedConfig *sl4f.Configuration,
) error {
	rpcClient, err := device.StartRpcSession(ctx, ffxTool, repo)
	if err != nil {
		return fmt.Errorf("unable to connect to sl4f: %w", err)
	}

	if expectedConfig == nil {
		logger.Infof(ctx, "no configuration expected, so not checking ABR configuration")
		return nil
	}

	if rpcClient == nil {
		logger.Infof(ctx, "not connected to sl4f, cannot check ABR configuration")
		return nil
	}

	// Ensure the device is booting from the expected boot slot.
	currentConfig, err := determineCurrentABRConfig(ctx, rpcClient)
	if err != nil {
		return fmt.Errorf("unable to determine current boot configuration: %w", err)
	}

	if currentConfig == nil {
		return fmt.Errorf("expected device to boot from slot %q, got <nil>", *expectedConfig)
	} else if *currentConfig != *expectedConfig {
		return fmt.Errorf("expected device to boot from slot %q, got %q", *expectedConfig, *currentConfig)
	}

	return nil
}

func ValidateDevice(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	device *device.Client,
	expectedSystemImage *packages.SystemImagePackage,
	expectedConfig *sl4f.Configuration,
	checkABR bool,
	expectedRebootReason string,
) error {
	// At the this point the system should have been updated to the target
	// system version. Confirm the update by fetching the device's current
	// /system/meta, and making sure it is the correct version.
	if expectedSystemImage != nil {
		upToDate, err := IsDeviceUpToDate(ctx, device, expectedSystemImage)
		if err != nil {
			return fmt.Errorf("failed to check if device is up to date: %w", err)
		}
		if !upToDate {
			return fmt.Errorf("system version failed to update to %q", expectedSystemImage)
		}
	}

	// Make sure the device doesn't have any broken static packages.
	if err := device.ValidateStaticPackages(ctx); err != nil {
		return fmt.Errorf("failed to validate static packages without sl4f: %w", err)
	}

	if checkABR {
		if err := checkABRConfig(ctx, ffxTool, device, expectedSystemImage.Repository(), expectedConfig); err != nil {
			return fmt.Errorf("failed to validate device: %w", err)
		}
	}

	if expectedRebootReason != "" {
		reason, err := ffxTool.TargetGetLastRebootReason(ctx, device.Name())
		if err != nil {
			return fmt.Errorf("failed to get last reboot reason: %w", err)
		}
		if reason != expectedRebootReason {
			return fmt.Errorf("expected reboot reason %q, got %q", expectedRebootReason, reason)
		}
	}

	return nil
}
