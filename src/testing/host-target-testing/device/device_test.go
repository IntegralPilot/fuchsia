// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package device

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/artifacts"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/packages"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/paver"
)

type fakeResolver struct {
	nodeName string
}

func (r fakeResolver) NodeName() string                                      { return r.nodeName }
func (r fakeResolver) ResolveSshAddress(ctx context.Context) (string, error) { return "", nil }
func (r fakeResolver) WaitToFindDeviceInFastboot(ctx context.Context) (string, error) {
	return r.nodeName, nil
}
func (r fakeResolver) WaitToFindDeviceInNetboot(ctx context.Context) (string, error) {
	return r.nodeName, nil
}

type fakeBuild struct{}

func (b fakeBuild) String() string                                    { return "fake-build" }
func (b fakeBuild) OutputDir() string                                 { return "fake-output-dir" }
func (b fakeBuild) GetBootserver(ctx context.Context) (string, error) { return "", nil }
func (b fakeBuild) GetFfx(ctx context.Context, ffxRunDir ffx.RunDir, version ffx.FfxVersionPolicy) (*ffx.FFXTool, error) {
	return nil, nil
}
func (b fakeBuild) GetFlashManifest(ctx context.Context) (string, error) {
	return "dir/flash.json", nil
}
func (b fakeBuild) GetProductBundleDir(ctx context.Context) (string, error) {
	return "", fmt.Errorf("no product bundle")
}
func (b fakeBuild) GetPackageRepository(ctx context.Context, blobFetchMode artifacts.BlobFetchMode, ffxRunDir ffx.RunDir, version ffx.FfxVersionPolicy, hostFfx *ffx.FFXTool) (*packages.Repository, error) {
	return nil, nil
}
func (b fakeBuild) GetPaverDir(ctx context.Context) (string, error) { return "", nil }
func (b fakeBuild) GetPaver(ctx context.Context, sshPublicKey ssh.PublicKey) (paver.Paver, error) {
	return nil, nil
}
func (b fakeBuild) GetVbmetaPath(ctx context.Context) (string, error) { return "", nil }

func generatePublicKey(t *testing.T) ssh.PublicKey {
	privateKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	pub, err := ssh.NewPublicKey(&privateKey.PublicKey)
	if err != nil {
		t.Fatal(err)
	}
	return pub
}

func TestFlashUsesSerialNumber(t *testing.T) {
	nodeName := "fuchsia-14c1-4eda-ba4d"
	serial := "11261J3DA017CR"

	// Create a fake ffx script
	ffxPath := filepath.Join(t.TempDir(), "ffx.sh")

	// Script behavior:
	// If arguments include "target list", return JSON with serial.
	// If arguments include "target flash", echo arguments to file.
	argsFile := filepath.Join(t.TempDir(), "args.txt")
	contents := `#!/bin/sh
for arg in "$@"; do
  if [ "$arg" = "list" ]; then
    echo '[{"nodename": "'"` + nodeName + `"'", "serial": "'"` + serial + `"'", "target_state": "Fastboot"}]'
    exit 0
  fi
done
echo "$@" > ` + argsFile + `
`
	if err := os.WriteFile(ffxPath, []byte(contents), 0o700); err != nil {
		t.Fatal(err)
	}

	tmpDir := t.TempDir()
	ffxRunDir := ffx.NewRunDir(tmpDir)
	ffxTool, err := ffx.NewFFXTool(ffxPath, ffxRunDir)
	if err != nil {
		t.Fatal(err)
	}

	client := &Client{
		resolverMode:    "constant",
		nodeName:        nodeName,
		flashRetrySleep: 1 * time.Millisecond,
	}

	sshKey := generatePublicKey(t)
	build := fakeBuild{}

	err = client.Flash(context.Background(), ffxTool, build, sshKey)
	if err != nil {
		t.Fatal(err)
	}

	// Read the arguments written by the fake script
	argsData, err := os.ReadFile(argsFile)
	if err != nil {
		t.Fatal(err)
	}
	argsStr := string(argsData)

	// Verify that the serial number was passed to --target
	expectedTargetArg := "--target " + serial
	if !strings.Contains(argsStr, expectedTargetArg) {
		t.Fatalf("Expected arguments to contain %q, but got %q", expectedTargetArg, argsStr)
	}
}

func TestFlashFailsOnTargetListError(t *testing.T) {
	nodeName := "fuchsia-14c1-4eda-ba4d"

	// Create a fake ffx script that fails on "target list"
	ffxPath := filepath.Join(t.TempDir(), "ffx.sh")
	contents := `#!/bin/sh
for arg in "$@"; do
  if [ "$arg" = "list" ]; then
    echo "error: ffx target list crashed" >&2
    exit 1
  fi
done
`
	if err := os.WriteFile(ffxPath, []byte(contents), 0o700); err != nil {
		t.Fatal(err)
	}

	tmpDir := t.TempDir()
	ffxRunDir := ffx.NewRunDir(tmpDir)
	ffxTool, err := ffx.NewFFXTool(ffxPath, ffxRunDir)
	if err != nil {
		t.Fatal(err)
	}

	client := &Client{
		resolverMode:    "constant",
		nodeName:        nodeName,
		flashRetrySleep: 1 * time.Millisecond,
	}

	sshKey := generatePublicKey(t)
	build := fakeBuild{}

	err = client.Flash(context.Background(), ffxTool, build, sshKey)
	if err == nil {
		t.Fatalf("Expected Flash to fail on target list error, but it succeeded")
	}

	expectedErrorStr := "failed to list devices in Product state"
	if !strings.Contains(err.Error(), expectedErrorStr) {
		t.Fatalf("Expected error to contain %q, but got %v", expectedErrorStr, err)
	}
}
