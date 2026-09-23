// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package cli

import (
	"context"
	"flag"
	"fmt"
	"os"
	"time"

	"golang.org/x/crypto/ssh"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/device"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
)

type DeviceResolverMode = string

const (
	// Resolve devices with ffx.
	FfxResolver = "ffx"

	// Resolve devices with constant hostnames.
	ConstantResolver = "constant"

	// Resolve devices with mDNS directly, bypassing ffx.
	MdnsResolver = "mdns"
)

type DeviceConfig struct {
	sshKeyFile               string
	deviceFinderPath         string
	deviceName               string
	deviceHostname           string
	deviceResolverMode       DeviceResolverMode
	deviceSshPort            int
	repoPort                 int
	sshPrivateKey            ssh.Signer
	SerialSocketPath         string
	connectTimeout           time.Duration
	WorkaroundBrokenTimeSkip bool
	testDataPath             string
}

func NewDeviceConfig(fs *flag.FlagSet, testDataPath string) *DeviceConfig {
	c := &DeviceConfig{}
	c.testDataPath = testDataPath
	fs.StringVar(&c.sshKeyFile, "ssh-private-key", os.Getenv(constants.SSHKeyEnvKey), "SSH private key file that can access the device")
	fs.StringVar(&c.deviceName, "device", os.Getenv(constants.NodenameEnvKey), "device name")
	fs.StringVar(&c.deviceHostname, "device-hostname", os.Getenv(constants.DeviceAddrEnvKey), "device hostname or IPv4/IPv6 address")
	fs.StringVar(&c.deviceResolverMode, "resolver-mode", ConstantResolver, "device resolver (default: Constant Resolver)")
	fs.IntVar(&c.deviceSshPort, "device-ssh-port", 22, "device port")
	fs.StringVar(&c.deviceFinderPath, "device-finder-path", "", "device-finder tool path")
	fs.StringVar(&c.SerialSocketPath, "device-serial", "", "device serial path")
	fs.DurationVar(&c.connectTimeout, "device-connect-timeout", 5*time.Second, "device connection timeout (default 5 seconds)")
	fs.BoolVar(&c.WorkaroundBrokenTimeSkip, "workaround-broken-time-skip", false,
		"whether to sleep for 15 seconds after pave and then reconnect, to work around a known networking bug, https://fxbug.dev/42154590")
	fs.IntVar(&c.repoPort, "repo-port", 0, "default port to serve the repository")

	environmentSerialPath := os.Getenv(constants.SerialSocketEnvKey)
	if c.SerialSocketPath == "" && environmentSerialPath != "" {
		c.SerialSocketPath = environmentSerialPath
	}

	return c
}

func (c *DeviceConfig) Validate() error {
	for _, s := range []string{
		c.sshKeyFile,
		c.SerialSocketPath,
	} {
		if err := util.ValidatePath(s); err != nil {
			return err
		}
	}

	if c.deviceResolverMode == ConstantResolver && c.deviceHostname == "" {
		return fmt.Errorf("device hostname cannot be empty when using constant resolver")
	}

	if c.deviceResolverMode == FfxResolver && c.deviceName == "" && c.deviceHostname == "" {
		return fmt.Errorf("either device name or device hostname must be specified when using ffx resolver")
	}

	if c.deviceResolverMode == MdnsResolver && c.deviceName == "" {
		return fmt.Errorf("device name cannot be empty when using mDNS resolver")
	}
	return nil
}

func (c *DeviceConfig) deviceResolver(
	ctx context.Context,
	ffx *ffx.FFXTool,
) (device.DeviceResolver, error) {
	switch c.deviceResolverMode {
	case ConstantResolver:
		logger.Infof(ctx, "constant resolver uses device name '%v' and address '%v'", c.deviceName, c.deviceHostname)
		return device.NewConstantHostResolver(
			ctx,
			c.deviceName,
			c.deviceHostname,
			c.deviceSshPort,
		), nil
	case FfxResolver:
		// If the device name is not set, attempt to resolve it using the hostname.
		if c.deviceName == "" && c.deviceHostname != "" {
			target, err := ffx.WaitForTarget(ctx, c.deviceHostname)
			if err != nil {
				return nil, fmt.Errorf("failed to find target with hostname: %s: %w", c.deviceHostname, err)
			}
			logger.Infof(ctx, "resolved device name %v from hostname %v", target.NodeName, c.deviceHostname)
			c.deviceName = target.NodeName
		}
		if c.deviceName == "" {
			return nil, fmt.Errorf("device name cannot be empty when using ffx resolver")
		}
		logger.Infof(ctx, "ffx resolver uses device name '%v'", c.deviceName)
		return device.NewFfxResolver(ctx, ffx, c.deviceName, c.deviceHostname)
	case MdnsResolver:
		logger.Infof(ctx, "mDNS resolver uses device name '%v'", c.deviceName)
		return device.NewMdnsResolver(
			ctx,
			c.deviceName,
			c.deviceSshPort,
		), nil
	default:
		return nil, fmt.Errorf("Invalid device resolver mode %v", c.deviceResolverMode)
	}
}

func (c *DeviceConfig) SSHKeyFile() string {
	return c.sshKeyFile
}

func (c *DeviceConfig) DeviceAddress() string {
	return c.deviceHostname
}

func (c *DeviceConfig) SSHPrivateKey() (ssh.Signer, error) {
	if c.sshPrivateKey == nil {
		if c.sshKeyFile == "" {
			return nil, fmt.Errorf("ssh private key cannot be empty")
		}

		key, err := os.ReadFile(c.sshKeyFile)
		if err != nil {
			return nil, err
		}

		privateKey, err := ssh.ParsePrivateKey(key)
		if err != nil {
			return nil, err
		}
		c.sshPrivateKey = privateKey
	}

	return c.sshPrivateKey, nil
}

func (c *DeviceConfig) NewDeviceClient(
	ctx context.Context,
	ffx *ffx.FFXTool,
) (*device.Client, error) {

	sshPrivateKey, err := c.SSHPrivateKey()
	if err != nil {
		return nil, err
	}

	connectBackoff := retry.NewConstantBackoff(c.connectTimeout)

	var serialConn *device.SerialConn
	if c.SerialSocketPath != "" {
		serialConn, err = device.NewSerialConn(c.SerialSocketPath)
		if err != nil {
			return nil, err
		}
	}

	client, err := device.NewClient(
		ctx,
		c.repoPort,
		c.deviceResolverMode,
		c.deviceName,
		c.deviceHostname,
		c.deviceSshPort,
		sshPrivateKey,
		connectBackoff,
		c.WorkaroundBrokenTimeSkip,
		serialConn,
		ffx,
	)
	if err != nil {
		return nil, err
	}

	return client, nil
}
