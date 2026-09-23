// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package cli

import (
	"context"
	"flag"
	"fmt"
	"log"
	"path/filepath"
	"time"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/artifacts"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/avb"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/device"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/omaha_tool"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/updater"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/zbi"
)

type InstallerMode = string

const (
	// Install OTAs with the omaha-client.
	Omaha = "omaha"

	// Install OTAs with the system-update-checker.
	SystemUpdateChecker = "system-update-checker"

	// Install OTAs directly with the system-updater.
	SystemUpdater = "system-updater"
)

type InstallerConfig struct {
	installerMode               InstallerMode
	avbToolPath                 string
	zbiToolPath                 string
	keyPath                     string
	keyMetadataPath             string
	avbTool                     *avb.AVBTool
	zbiTool                     *zbi.ZBITool
	updater                     updater.Updater
	omahaTool                   *omaha_tool.OmahaTool
	omahaToolPath               string
	privateKeyId                string
	privateKeyPath              string
	omahaAddress                string
	omahaRequireCup             bool
	workaroundOtaNoRewriteRules bool
	testDataPath                string
}

func NewInstallerConfig(fs *flag.FlagSet, testDataPath string) (*InstallerConfig, error) {
	c := &InstallerConfig{}

	c.testDataPath = testDataPath
	fs.StringVar(&c.avbToolPath, "avbtool-path", filepath.Join(testDataPath, "avbtool.py"), "path to the avbtool binary")
	fs.StringVar(&c.installerMode, "installer", SystemUpdateChecker, "the installation mode (default: system-update-checker)")
	fs.StringVar(&c.keyMetadataPath, "vbmeta-key-metadata", "", "path to the vbmeta public key metadata")
	fs.StringVar(&c.keyPath, "vbmeta-key", "", "path to the vbmeta private key")
	fs.StringVar(&c.omahaAddress, "omaha-address", ":0", "which address to serve omaha server on (default random)")
	fs.StringVar(&c.omahaToolPath, "omaha-tool-path", filepath.Join(testDataPath, "mock-omaha-server"), "the path of the mock-omaha-server binary to invoke.")
	// This must match the key_id in
	// src/sys/pkg/bin/omaha-client:empty_eager_package_config, which relies
	// on src/sys/pkg/bin/omaha-client/test_data/key_config.json.
	fs.StringVar(&c.privateKeyId, "omaha-key-id", "123456789", "the integer private key ID to use for CUP within Omaha requests.")
	fs.StringVar(&c.privateKeyPath, "omaha-key-path", filepath.Join(testDataPath, "test_private_key.pem"), "the path of the private key .pem to use for CUP within Omaha requests.")
	fs.StringVar(&c.zbiToolPath, "zbitool-path", filepath.Join(testDataPath, "zbi"), "path to the zbi binary")
	fs.BoolVar(&c.omahaRequireCup, "require-cup", false, "if true, mock-omaha-server will assert that all incoming requests have CUP enabled.")
	fs.BoolVar(&c.workaroundOtaNoRewriteRules, "workaround-downgrade-ota-no-rewrite-rules", false, "if true, omaha updater will not set workaround downgrade ota rewrite rules.")

	return c, nil
}

func (c *InstallerConfig) Validate() error {
	for _, s := range []string{
		c.avbToolPath,
		c.keyMetadataPath,
		c.keyPath,
		c.omahaToolPath,
		c.privateKeyPath,
		c.zbiToolPath,
	} {
		if err := util.ValidatePath(s); err != nil {
			return err
		}
	}
	return nil
}

func (c *InstallerConfig) AVBTool() (*avb.AVBTool, error) {
	if c.avbTool == nil {
		if c.keyPath == "" {
			c.keyPath = filepath.Join(c.testDataPath, "atx_psk.pem")
		}
		if c.keyMetadataPath == "" {
			c.keyMetadataPath = filepath.Join(c.testDataPath, "avb_atx_metadata.bin")
		}

		avbTool, err := avb.NewAVBTool(c.avbToolPath, c.keyPath, c.keyMetadataPath)
		if err != nil {
			return nil, err
		}
		c.avbTool = avbTool
	}

	return c.avbTool, nil
}

func (c *InstallerConfig) ZBITool() (*zbi.ZBITool, error) {
	if c.zbiTool == nil {
		zbiTool, err := zbi.NewZBITool(c.zbiToolPath)
		if err != nil {
			return nil, err
		}
		c.zbiTool = zbiTool
	}

	return c.zbiTool, nil
}

func (c *InstallerConfig) OmahaTool(ctx context.Context, device *device.Client) (*omaha_tool.OmahaTool, error) {
	localHostname, err := device.GetSSHConnection(ctx)
	if err != nil {
		return nil, err
	}
	omahaTool, err := omaha_tool.NewOmahaServer(ctx, omaha_tool.OmahaToolArgs{
		ToolPath:       c.omahaToolPath,
		PrivateKeyId:   c.privateKeyId,
		PrivateKeyPath: c.privateKeyPath,
		AppId:          "e2e-ota-test-app-id",
		LocalHostname:  localHostname,
		RequireCup:     c.omahaRequireCup,
	}, /*stdout=*/ nil /*stderr=*/, nil)
	if err != nil {
		return nil, err
	}
	return omahaTool, nil
}

func (c *InstallerConfig) NeedsInitialization() bool {
	// The omaha-client needs the device to be flashed or paved since the
	// tests start a fake omaha server and inject's the address into a
	// custom vbmeta.
	return c.installerMode == Omaha
}

// ConfigureBuild configures a build for the updater.
func (c *InstallerConfig) ConfigureBuild(
	ctx context.Context,
	device *device.Client,
	build artifacts.Build,
) (artifacts.Build, error) {
	switch c.installerMode {
	case Omaha:
		if c.omahaTool == nil {
			omahaTool, err := c.OmahaTool(ctx, device)
			if err != nil {
				return nil, err
			}
			c.omahaTool = omahaTool
		}

		avbTool, err := c.AVBTool()
		if err != nil {
			return nil, err
		}

		zbiTool, err := c.ZBITool()
		if err != nil {
			return nil, err
		}

		return artifacts.NewOmahaBuild(
			build,
			c.omahaTool,
			avbTool,
			zbiTool,
		), nil

	case SystemUpdateChecker:
		return build, nil

	case SystemUpdater:
		return build, nil

	default:
		return nil, fmt.Errorf("Invalid installer mode %v", c.installerMode)
	}
}

// Updater returns the configured updater.
func (c *InstallerConfig) Updater(checkForUnkownFirmware bool) (updater.Updater, error) {
	switch c.installerMode {
	case Omaha:
		avbTool, err := c.AVBTool()
		if err != nil {
			return nil, err
		}

		zbiTool, err := c.ZBITool()
		if err != nil {
			return nil, err
		}

		return updater.NewOmahaUpdater(
			c.omahaTool,
			avbTool,
			zbiTool,
			c.workaroundOtaNoRewriteRules,
			checkForUnkownFirmware,
		), nil

	case SystemUpdateChecker:
		return updater.NewSystemUpdateChecker(
			checkForUnkownFirmware,
		), nil

	case SystemUpdater:
		return updater.NewSystemUpdater(checkForUnkownFirmware), nil

	default:
		return nil, fmt.Errorf("Invalid installer mode: %v", c.installerMode)
	}
}

func (c *InstallerConfig) Shutdown(ctx context.Context) {
	if c.omahaTool == nil {
		return
	}
	ch := make(chan error)
	go func() {
		ch <- c.omahaTool.Shutdown(ctx)
	}()

	select {
	case err := <-ch:
		if err != nil {
			log.Printf("caught an error: %w", err)
		}
	case <-time.After(5 * time.Second):
		log.Printf("took longer than 5 seconds to shut down the installer")
	}
}
