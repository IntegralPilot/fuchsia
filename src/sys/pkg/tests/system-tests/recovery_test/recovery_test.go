// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package recovery

import (
	"bytes"
	"context"
	"flag"
	"fmt"
	"log"
	"os"
	"strconv"
	"strings"
	"testing"

	"go.fuchsia.dev/fuchsia/src/sys/pkg/tests/system-tests/check"
	"go.fuchsia.dev/fuchsia/src/sys/pkg/tests/system-tests/flash"
	"go.fuchsia.dev/fuchsia/src/sys/pkg/tests/system-tests/pave"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/artifacts"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/device"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/errutil"
	ffxpkg "go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/sl4f"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/tools/lib/color"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

var c *config

func TestMain(m *testing.M) {
	log.SetPrefix("recovery-test: ")
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

func TestRecovery(t *testing.T) {
	ctx := context.Background()
	l := logger.NewLogger(
		c.logLevel,
		color.NewColor(color.ColorAuto),
		os.Stdout,
		os.Stderr,
		"recovery-test: ")
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
		return fmt.Errorf("failed to get output directory: %w", err)
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
		device.NewEstimatedMonotonicTime(deviceClient, "recovery-test: "),
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
		return initializeDevice(ctx, deviceClient, ffxTool, build, version)
	}); err != nil {
		return fmt.Errorf("initialization failed: %w", err)
	}

	return testRecovery(ctx, ffxTool, deviceClient, ffxTool.RunDir(), build, version)
}

func testRecovery(
	ctx context.Context,
	ffxTool *ffxpkg.FFXTool,
	device *device.Client,
	ffxRunDir ffxpkg.RunDir,
	build artifacts.Build,
	version ffxpkg.FfxVersionPolicy,
) error {
	for i := 1; i <= c.cycleCount; i++ {
		logger.Infof(ctx, "Recovery Attempt %d", i)

		// Protect against the test stalling out by wrapping it in a closure,
		// setting a timeout on the context, and running the actual test in a
		// closure.
		if err := util.RunWithTimeout(ctx, c.cycleTimeout, func() error {
			return doTestRecovery(ctx, ffxTool, device, ffxRunDir, build, version)
		}); err != nil {
			return fmt.Errorf("Recovery Cycle %d failed: %w", i, err)
		}
	}

	return nil
}

func doTestRecovery(
	ctx context.Context,
	ffxTool *ffxpkg.FFXTool,
	device *device.Client,
	ffxRunDir ffxpkg.RunDir,
	build artifacts.Build,
	version ffxpkg.FfxVersionPolicy,
) error {
	// We don't install an OTA, so we don't need to prefetch the blobs.
	repo, err := build.GetPackageRepository(ctx, artifacts.LazilyFetchBlobs, ffxRunDir, version, ffxTool)
	if err != nil {
		return fmt.Errorf("unable to get repository: %w", err)
	}

	// We should use this ffx after we reboot.
	// Get the FFX tool matching the version of the build we are testing to use for reconnection.
	nextFfxTool, err := build.GetFfx(ctx, ffxRunDir, version)
	if err != nil {
		return fmt.Errorf("failed to get ffx from build %s: %w", build, err)
	}

	updatePackage, err := repo.OpenUpdatePackage(ctx, "update/0")
	if err != nil {
		return fmt.Errorf("error opening update/0: %w", err)
	}

	expectedSystemImage, err := updatePackage.OpenSystemImagePackage(ctx)
	if err != nil {
		return fmt.Errorf("error extracting expected system image: %w", err)
	}

	expectedConfig, err := check.DetermineCurrentABRConfig(ctx, ffxTool, device, repo)
	if err != nil {
		return fmt.Errorf("error determining target config: %w", err)
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

	if err := device.RebootToRecovery(ctx); err != nil {
		return fmt.Errorf("error rebooting to recovery: %w", err)
	}

	if err := device.Reconnect(ctx, nextFfxTool); err != nil {
		return fmt.Errorf("failed to reconnect: %w", err)
	}

	// Validate that system_recovery.cm is running
	var b bytes.Buffer
	cmd := []string{"ps", "|", "grep", "system_recovery", "|", "wc", "-l"}
	if err := device.Run(ctx, cmd, &b, os.Stderr); err != nil {
		return fmt.Errorf("failed to run command: %w", err)
	}
	t, err := strconv.Atoi(strings.TrimSpace(b.String()))
	if err != nil {
		return fmt.Errorf("unable to convert result of command: %w", err)
	}
	if t == 0 {
		return fmt.Errorf("system_recovery does seem to be running")
	}

	return nil
}

func initializeDevice(
	ctx context.Context,
	device *device.Client,
	ffx *ffxpkg.FFXTool,
	build artifacts.Build,
	version ffxpkg.FfxVersionPolicy,
) error {
	logger.Infof(ctx, "Initializing device")

	repo, err := build.GetPackageRepository(ctx, artifacts.LazilyFetchBlobs, ffx.RunDir(), version, ffx)
	if err != nil {
		return err
	}

	updatePackage, err := repo.OpenUpdatePackage(ctx, "update/0")
	if err != nil {
		return fmt.Errorf("error opening update/0: %w", err)
	}

	expectedSystemImage, err := updatePackage.OpenSystemImagePackage(ctx)
	if err != nil {
		return fmt.Errorf("error extracting expected system image merkle: %w", err)
	}

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
			if err := flash.FlashDevice(ctx, device, ffx, build, sshPrivateKey.PublicKey(), version); err != nil {
				return fmt.Errorf("failed to flash device during initialization: %w", err)
			}
		} else {
			if err := pave.PaveDevice(ctx, device, ffx, build, sshPrivateKey.PublicKey(), version); err != nil {
				return fmt.Errorf("failed to pave device during initialization: %w", err)
			}
		}
	}

	expectedConfig, err := check.DetermineCurrentABRConfig(ctx, ffx, device, repo)
	if err != nil {
		return err
	}

	if !upToDate && expectedConfig != nil {
		config := sl4f.ConfigurationA
		expectedConfig = &config
	}

	if err := check.ValidateDevice(
		ctx,
		ffx,
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
