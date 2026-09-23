// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package emulator

import (
	"bytes"
	"context"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	ffxlib "go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/ffx"
	fvdpb "go.fuchsia.dev/fuchsia/tools/virtual_device/proto"
)

const NETWORK_TEST_REALM_COMPONENT_NAME = "net-test-realm-controller"

const NETWORK_TEST_REALM_TEST_COLLECTION_MONIKER = "core/testing/netstack-tests"

const NETWORK_TEST_REALM_MONIKER = "/" + NETWORK_TEST_REALM_TEST_COLLECTION_MONIKER + ":" + NETWORK_TEST_REALM_COMPONENT_NAME

const NETWORK_TEST_REALM_URL = "fuchsia-pkg://fuchsia.com/network-test-realm#meta/controller.cm"

const NETSTACK_VERSION = "v2"

func TestEmulatorWorksWithFfx(t *testing.T) {
	var wg sync.WaitGroup
	defer wg.Wait()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	executablePath, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	hostOutDir, err := filepath.Abs(filepath.Dir(executablePath))
	if err != nil {
		t.Fatal(err)
	}

	sourceRootRelativeDir := filepath.Join(
		hostOutDir,
		"..",
		"host-tools",
	)

	nodename := "TestEmulatorWorksWithFfx-Nodename"

	tempDir := t.TempDir()
	ffxPath := filepath.Join(sourceRootRelativeDir, "ffx")
	ffx, err := ffxlib.NewFfxInstance(
		ctx,
		ffxlib.FfxInstanceOptions{
			Target:        nodename,
			TestOutputDir: tempDir,
			FfxBinPath:    ffxPath,
		},
	)
	if err != nil {
		t.Fatal(err)
	}

	ssh_auth_keys := ffx.GetSshAuthorizedKeys()

	defer func() {
		// Just log errors since ffx.Stop() is expected to return a DeadlineExceeded
		// when the daemon takes longer than usual to shut down (which is not
		// actionable for us).
		if err := ffx.Stop(); err != nil {
			t.Logf("ffx.Stop() = %s", err)
		}
	}()

	// Note: To run this test locally on linux, you must create the TAP interface:
	// $ sudo ip tuntap add mode tap qemu; sudo ip link set dev qemu up
	// The qemu tap interface is assumed to exist on infra.
	if runtime.GOOS == "linux" {
		cmd := exec.Command("ip", "tuntap", "show")
		// We don't check the error, because `ip tuntap show` may return an error
		// when there are no devices. We only care about the output.
		out, _ := cmd.CombinedOutput()

		if !strings.Contains(string(out), "qemu:") {
			t.Fatal("tun/tap device 'qemu' not found. To create one, run:\n\n$ sudo ip tuntap add dev qemu mode tap user $USER\n$ sudo ip link set qemu up")
		}
	}
	netdevs := []*fvdpb.Netdev{{
		Id:   "qemu",
		Kind: "tap",
		Device: &fvdpb.Device{
			Model: "virtio-net-pci",
			Options: []string{
				"mac=00:00:00:00:00:0a",
				"addr=0a",
			},
		},
	}}

	i, err := NewQemuInstance(ctx,
		ssh_auth_keys,
		QemuInstanceArgs{
			Nodename:       nodename,
			HostX64Path:    hostOutDir,
			NetworkDevices: netdevs,
		})
	if err != nil {
		t.Fatal(err)
	}

	emulatorDone := make(chan error)
	defer func() {
		for err := range emulatorDone {
			if err != nil {
				t.Error(err)
			}
		}
	}()

	// Ensure that cancel() is run before we try to drain the emulator error
	// channel.
	defer cancel()

	go func() {
		_, err := i.Wait()
		emulatorDone <- err
		close(emulatorDone)
	}()

	netTestRealmExperimentalFlagPointer := "net.test.realm"
	if err := ffx.ConfigSet(ctx, netTestRealmExperimentalFlagPointer, "true"); err != nil {
		t.Fatalf(
			"ffx.ConfigSet(%q, true) = %s",
			netTestRealmExperimentalFlagPointer,
			err,
		)
	}

	runFfx := func(ctx context.Context, args ...string) error {
		return ffx.RunWithTarget(ctx, args...)
	}

	if err := runFfx(ctx, "target", "wait"); err != nil {
		t.Fatal(err)
	}

	t.Run("starts network test realm", func(t *testing.T) {
		var buf bytes.Buffer
		ffx.SetStdoutStderr(io.MultiWriter(os.Stdout, &buf), os.Stderr)

		if err := runFfx(ctx, "--machine", "json", "component", "show", NETWORK_TEST_REALM_MONIKER); err == nil {
			t.Logf(
				"ffx --machine json component show %s should have failed (component shouldn't exist)",
				NETWORK_TEST_REALM_MONIKER,
			)
		}

		if err := runFfx(
			ctx,
			"component",
			"create",
			NETWORK_TEST_REALM_MONIKER,
			NETWORK_TEST_REALM_URL,
		); err != nil {
			t.Fatalf(
				"ffx component create %s %s = %s",
				NETWORK_TEST_REALM_MONIKER,
				NETWORK_TEST_REALM_URL,
				err,
			)
		}

		if err := runFfx(
			ctx,
			"net-test-realm",
			NETWORK_TEST_REALM_MONIKER,
			"start-hermetic-network-realm",
			NETSTACK_VERSION,
		); err != nil {
			t.Fatalf(
				"ffx net-test-realm %s start-hermetic-network-realm %s = %s",
				NETWORK_TEST_REALM_MONIKER,
				NETSTACK_VERSION,
				err,
			)
		}

		// Check that ffx log can be run for some time without an error.
		ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
		defer cancel()

		if err := runFfx(
			ctx,
			"log",
			"--filter",
			NETWORK_TEST_REALM_TEST_COLLECTION_MONIKER,
		); err != nil && !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf(
				"ffx log --filter %s = %s",
				NETWORK_TEST_REALM_TEST_COLLECTION_MONIKER,
				err,
			)
		}
	})
}
