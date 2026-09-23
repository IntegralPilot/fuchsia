// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package upgrade

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/cli"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

type config struct {
	ffxConfig                  *cli.FfxConfig
	archiveConfig              *cli.ArchiveConfig
	installerConfig            *cli.InstallerConfig
	deviceConfig               *cli.DeviceConfig
	chainedBuildConfig         *cli.RepeatableBuildConfig
	paveTimeout                time.Duration
	cycleCount                 uint
	cycleTimeout               time.Duration
	useFlash                   bool
	downgradeOTAAttempts       uint
	bootfsCompression          string
	buildExpectUnknownFirmware bool
	maxUpdatePackageSize       uint64
	maxUpdateImagesSize        uint64
	maxSystemImageSize         uint64
	checkABR                   bool
	checkRebootReason          bool
	logLevel                   logger.LogLevel
	modifyUpdatePackage        bool
}

func newConfig(fs *flag.FlagSet) (*config, error) {
	testDataPath := filepath.Join(filepath.Dir(os.Args[0]), "test_data", "system-tests")

	ffxConfig := cli.NewFfxConfig(fs)
	installerConfig, err := cli.NewInstallerConfig(fs, testDataPath)
	if err != nil {
		return nil, err
	}

	archiveConfig := cli.NewArchiveConfig(fs, testDataPath)
	deviceConfig := cli.NewDeviceConfig(fs, testDataPath)
	c := &config{
		ffxConfig:          ffxConfig,
		archiveConfig:      archiveConfig,
		deviceConfig:       deviceConfig,
		installerConfig:    installerConfig,
		chainedBuildConfig: cli.NewRepeatableBuildConfig(fs, archiveConfig, deviceConfig, os.Getenv("BUILDBUCKET_ID"), ""),
		logLevel:           logger.TraceLevel,
	}

	fs.DurationVar(&c.paveTimeout, "pave-timeout", 5*time.Minute, "Err if a pave takes longer than this time (default is 5 minutes)")
	fs.UintVar(&c.cycleCount, "cycle-count", 1, "How many cycles to run the test before completing (default is 1)")
	fs.DurationVar(&c.cycleTimeout, "cycle-timeout", 20*time.Minute, "Err if a test cycle takes longer than this time (default is 20 minutes)")
	fs.BoolVar(&c.useFlash, "use-flash", true, "Provision device using flashing instead of paving")
	fs.UintVar(&c.downgradeOTAAttempts, "downgrade-ota-attempts", 1, "Number of times to try to OTA from the downgrade build to the upgrade build before failing.")
	fs.StringVar(&c.bootfsCompression, "bootfs-compression", "zstd.max", "compress storage images, default is zstd.max")
	fs.BoolVar(&c.buildExpectUnknownFirmware, "build-expect-unknown-firmware", false, "Ignore 'Unknown Firmware' during OTAs")
	fs.Uint64Var(&c.maxUpdatePackageSize, "max-update-package-size", 0, "Maximum size of all the blobs in the update package")
	fs.Uint64Var(&c.maxUpdateImagesSize, "max-update-images-size", 0, "Maximum size of all the blobs in the update images")
	fs.Uint64Var(&c.maxSystemImageSize, "max-system-image-size", 0, "Maximum size of all the blobs in the system image")
	fs.BoolVar(&c.checkABR, "check-abr", true, "Check that the device booted into the expected ABR slot (default is true)")
	fs.BoolVar(&c.checkRebootReason, "check-reboot-reason", false, "Verify the device reboot reason after OTA (default is false)")
	fs.Var(&c.logLevel, "log-level", "log level (no, fatal, error, warning, info, debug, trace)")
	fs.BoolVar(&c.modifyUpdatePackage, "modify-update-package", true, "modify update packages to make them unique (default is true)")

	return c, nil
}

func (c *config) validate() error {
	if c.cycleCount < 1 {
		return fmt.Errorf("-cycle-count must be >= 1")
	}

	if err := c.ffxConfig.Validate(); err != nil {
		return err
	}

	if err := c.deviceConfig.Validate(); err != nil {
		return err
	}

	if err := c.installerConfig.Validate(); err != nil {
		return err
	}

	return nil
}
