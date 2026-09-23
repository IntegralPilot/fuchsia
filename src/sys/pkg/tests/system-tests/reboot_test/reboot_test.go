// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package reboot

import (
	"context"
	"flag"
	"fmt"
	"log"
	"os"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/src/sys/pkg/tests/system-tests/check"
	"go.fuchsia.dev/fuchsia/src/sys/pkg/tests/system-tests/flash"
	"go.fuchsia.dev/fuchsia/src/sys/pkg/tests/system-tests/pave"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/artifacts"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/device"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/errutil"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/sl4f"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/tools/lib/color"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

var c *config

func TestMain(m *testing.M) {
	log.SetPrefix("reboot-test: ")
	log.SetFlags(log.Ldate | log.Ltime | log.LUTC | log.Lshortfile)

	var err error
	c, err = newConfig(flag.CommandLine)
	if err != nil {
		log.Fatalf("failed to create config: %s", err)
	}

	flag.Parse()

	if flag.NArg() != 0 {
		log.Fatalf("unexpected positional arguments: %v", flag.Args())
	}

	if err = c.validate(); err != nil {
		log.Fatalf("config is invalid: %s", err)
	}

	os.Exit(m.Run())
}

func TestReboot(t *testing.T) {
	ctx := context.Background()
	l := logger.NewLogger(
		c.logLevel,
		color.NewColor(color.ColorAuto),
		os.Stdout,
		os.Stderr,
		"reboot-test: ")
	l.SetFlags(logger.Ldate | logger.Ltime | logger.LUTC | logger.Lshortfile)
	ctx = logger.WithLogger(ctx, l)

	if err := doTest(ctx); err != nil {
		logger.Errorf(ctx, "test failed: %v", err)
		errutil.HandleError(ctx, c.deviceConfig.SerialSocketPath, err)
		t.Fatal(err)
	}
}

func doTest(ctx context.Context) error {
	outputDir, archiveCleanup, err := c.archiveConfig.OutputDir()
	if err != nil {
		return fmt.Errorf("failed to get archive output directory: %w", err)
	}
	defer archiveCleanup()

	ffxTool, ffxCleanup, err := c.ffxConfig.NewFfxTool(ctx, c.deviceConfig.SSHKeyFile())
	if err != nil {
		return fmt.Errorf("failed to create ffx: %w", err)
	}
	defer ffxCleanup()

	deviceClient, err := c.deviceConfig.NewDeviceClient(ctx, ffxTool)
	if err != nil {
		return fmt.Errorf("failed to create ota test client: %w", err)
	}
	defer deviceClient.Close()

	l := logger.NewLogger(
		c.logLevel,
		color.NewColor(color.ColorAuto),
		os.Stdout,
		os.Stderr,
		device.NewEstimatedMonotonicTime(deviceClient, "reboot-test: "),
	)
	l.SetFlags(logger.Ldate | logger.Ltime | logger.LUTC | logger.Lshortfile)
	ctx = logger.WithLogger(ctx, l)

	builds, err := c.buildConfig.GetBuilds(ctx, deviceClient, outputDir)
	if err != nil {
		return fmt.Errorf("failed to get builds: %w", err)
	}
	if len(builds) == 0 {
		return fmt.Errorf("no builds configured")
	}
	build := builds[0].Build
	version := builds[0].Version

	if err := util.RunWithTimeout(ctx, c.paveTimeout, func() error {
		err := initializeDevice(ctx, deviceClient, ffxTool, build, version)
		return err
	}); err != nil {
		return fmt.Errorf("initialization failed: %w", err)
	}

	return testReboot(ctx, deviceClient, ffxTool, build, version)
}

func testReboot(
	ctx context.Context,
	device *device.Client,
	ffxTool *ffx.FFXTool,
	build artifacts.Build,
	version ffx.FfxVersionPolicy,
) error {
	if err := sleepAfterReboot(ctx, device); err != nil {
		return err
	}

	for i := 1; i <= c.cycleCount; i++ {
		logger.Infof(ctx, "Reboot Attempt %d", i)

		// Protect against the test stalling out by wrapping it in a closure,
		// setting a timeout on the context, and running the actual test in a
		// closure.
		if err := util.RunWithTimeout(ctx, c.cycleTimeout, func() error {
			return doTestReboot(ctx, device, ffxTool, build, version)
		}); err != nil {
			return fmt.Errorf("Reboot Cycle %d failed: %w", i, err)
		}
	}

	return nil
}

func doTestReboot(
	ctx context.Context,
	device *device.Client,
	ffxTool *ffx.FFXTool,
	build artifacts.Build,
	version ffx.FfxVersionPolicy,
) error {
	// We don't install an OTA, so we don't need to prefetch the blobs.
	repo, err := build.GetPackageRepository(
		ctx,
		artifacts.LazilyFetchBlobs,
		ffxTool.RunDir(),
		version,
		ffxTool,
	)
	if err != nil {
		return fmt.Errorf("unable to get repository: %w", err)
	}

	updatePackage, err := repo.OpenUpdatePackage(ctx, "update/0")
	if err != nil {
		return fmt.Errorf("error opening update/0: %w", err)
	}

	// Install version N on the device if it is not already on that version.
	expectedSystemImage, err := updatePackage.OpenSystemImagePackage(ctx)
	if err != nil {
		return fmt.Errorf("error extracting expected system image merkle: %w", err)
	}

	var expectedConfig *sl4f.Configuration
	if c.checkABR {
		config, err := check.DetermineCurrentABRConfig(ctx, ffxTool, device, repo)
		if err != nil {
			return fmt.Errorf("error determining target config: %w", err)
		}
		expectedConfig = config
	}

	if err := check.ValidateDevice(
		ctx,
		ffxTool,
		device,
		expectedSystemImage,
		expectedConfig,
		c.checkABR,
		"",
	); err != nil {
		return err
	}

	if err := device.Reboot(ctx, ffxTool); err != nil {
		return fmt.Errorf("error rebooting: %w", err)
	}

	if err := check.ValidateDevice(
		ctx,
		ffxTool,
		device,
		expectedSystemImage,
		expectedConfig,
		c.checkABR,
		"",
	); err != nil {
		return fmt.Errorf("failed to validate device: %w", err)
	}

	if err := sleepAfterReboot(ctx, device); err != nil {
		return err
	}

	return nil
}

func sleepAfterReboot(ctx context.Context, device *device.Client) error {
	if c.sleepAfterReboot != 0 {
		logger.Infof(ctx, "Sleeping %s seconds after reboot", c.sleepAfterReboot)

		disconnectionCh := device.DisconnectionListener()
		select {
		case <-time.After(c.sleepAfterReboot * time.Second):
		case <-disconnectionCh:
			return fmt.Errorf("Device unexpectedly disconnected")
		}
	}

	return nil
}

func initializeDevice(
	ctx context.Context,
	device *device.Client,
	ffxTool *ffx.FFXTool,
	build artifacts.Build,
	version ffx.FfxVersionPolicy,
) error {
	logger.Infof(ctx, "Initializing device")

	repo, err := build.GetPackageRepository(ctx, artifacts.LazilyFetchBlobs, ffxTool.RunDir(), version, ffxTool)
	if err != nil {
		return err
	}

	updatePackage, err := repo.OpenUpdatePackage(ctx, "update/0")
	if err != nil {
		return fmt.Errorf("error opening update/0: %w", err)
	}

	// Install version N on the device if it is not already on that version.
	expectedSystemImage, err := updatePackage.OpenSystemImagePackage(ctx)
	if err != nil {
		return fmt.Errorf("error extracting expected system image: %w", err)
	}

	// Only provision if the device is not running the expected version.
	upToDate, err := check.IsDeviceUpToDate(ctx, device, expectedSystemImage)
	if err != nil {
		return fmt.Errorf("failed to check if up to date during initialization: %w", err)
	}
	if upToDate {
		logger.Infof(ctx, "device already up to date")
	} else {
		sshPrivateKey, err := c.deviceConfig.SSHPrivateKey()
		if err != nil {
			return fmt.Errorf("failed to get ssh key: %w", err)
		}

		if c.useFlash {
			if err := flash.FlashDevice(ctx, device, ffxTool, build, sshPrivateKey.PublicKey(), version); err != nil {
				return fmt.Errorf("failed to flash device during initialization: %w", err)
			}
		} else {
			if err := pave.PaveDevice(ctx, device, ffxTool, build, sshPrivateKey.PublicKey(), version); err != nil {
				return fmt.Errorf("failed to pave device during initialization: %w", err)
			}
		}
	}

	var expectedConfig *sl4f.Configuration
	if c.checkABR {
		// Check if we support ABR. If so, we always boot into A after a pave.
		expectedConfig, err = check.DetermineCurrentABRConfig(ctx, ffxTool, device, repo)
		if err != nil {
			return err
		}

		if !upToDate && expectedConfig != nil {
			config := sl4f.ConfigurationA
			expectedConfig = &config
		}
	}

	if err := check.ValidateDevice(
		ctx,
		ffxTool,
		device,
		expectedSystemImage,
		expectedConfig,
		c.checkABR,
		"",
	); err != nil {
		return err
	}

	return nil
}
