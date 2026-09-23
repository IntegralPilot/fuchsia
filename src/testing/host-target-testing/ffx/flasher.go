// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffx

import (
	"context"
	"fmt"
	"os"
	"time"

	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"golang.org/x/crypto/ssh"
)

type Flasher struct {
	ffx                FFXToolImpl
	sshPublicKey       ssh.PublicKey
	target             string
	manifestPath       string
	productBundlePath  string
	discoveryTimeoutMs int
	retries            int
	retrySleep         time.Duration
}

// NewFlasher constructs a new flasher that uses `ffx` as the FFXTool used
// to flash a device using flash.json located at `path`. Also accepts a
// number of optional parameters.
func newFlasher(ffx FFXToolImpl) *Flasher {
	return &Flasher{
		ffx: ffx,
	}
}

// Sets the SSH public key that the Flasher will bake into the device as an
// authorized key.
func (f *Flasher) SetSSHPublicKey(publicKey ssh.PublicKey) {
	f.sshPublicKey = publicKey
}

func (f *Flasher) SetTarget(target string) {
	f.target = target
}

func (f *Flasher) SetManifest(manifestPath string) {
	f.manifestPath = manifestPath
}

func (f *Flasher) SetProductBundle(productBundlePath string) {
	f.productBundlePath = productBundlePath
}

func (f *Flasher) SetDiscoveryTimeout(ms int) {
	f.discoveryTimeoutMs = ms
}

func (f *Flasher) SetRetries(count int) {
	f.retries = count
}

func (f *Flasher) SetRetrySleep(d time.Duration) {
	f.retrySleep = d
}

// Flash a device with flash.json manifest.
func (f *Flasher) Flash(ctx context.Context) (string, error) {
	args := []string{}

	if f.discoveryTimeoutMs > 0 {
		args = append(args, "-c", fmt.Sprintf("discovery.timeout=%d", f.discoveryTimeoutMs))
	}

	if f.target != "" {
		args = append(args, "--target", f.target)
	}

	args = append(args, "target", "flash")

	// It's known that some devices take a longer time to flash. To avoid the ffx
	// command exiting before the device is fully flashed, we set the timeout
	// rate to 1(MB/s). This tells ffx to wait longer when transferring image
	// files. See https://fxbug.dev/42076970 for more details and the issue
	// previously noticed.
	args = append(args, "--timeout-rate", "1")
	// ffx doesn't allow the timeout rate to be less than 1MB/s, so also set the
	// minimum timeout to 10 minutes which makes the effective rate to ~0.5MB/s on
	// Astro with a roughly 300MB of fvm file.
	args = append(args, "--min-timeout-secs", "600")

	// Write out the public key's authorized keys.
	if f.sshPublicKey != nil {
		authorizedKeys, err := os.CreateTemp("", "")
		if err != nil {
			return "", err
		}
		defer os.Remove(authorizedKeys.Name())

		if _, err := authorizedKeys.Write(ssh.MarshalAuthorizedKey(f.sshPublicKey)); err != nil {
			return "", err
		}

		if err := authorizedKeys.Close(); err != nil {
			return "", err
		}

		args = append(args, "--authorized-keys", authorizedKeys.Name())
	}

	if f.productBundlePath != "" {
		args = append(args, "--product-bundle", f.productBundlePath)
	}

	if f.manifestPath != "" {
		args = append(args, "--manifest", f.manifestPath)
	}

	var stdout []byte
	var err error
	attempts := f.retries
	if attempts == 0 {
		attempts = 1
	}

	for i := 0; i < attempts; i++ {
		if f.retrySleep > 0 {
			time.Sleep(f.retrySleep)
		}
		if i > 0 {
			logger.Infof(ctx, "Retrying flash attempt %d/%d", i+1, attempts)
		}
		stdout, err = f.ffx.runFFXCmd(ctx, args...)
		if err == nil {
			return string(stdout), nil
		}
	}

	return "", fmt.Errorf("device failed to flash after %d attempts: %w", attempts, err)
}
