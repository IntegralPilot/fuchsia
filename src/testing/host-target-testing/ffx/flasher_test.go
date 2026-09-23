// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffx

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"golang.org/x/crypto/ssh"
)

// The easiest way to make a fake key is to just generate a real one.
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

func createAndRunFlasher(t *testing.T, setupFlasher func(t *testing.T, flasher *Flasher)) string {
	// createScript returns the path to a bash script that outputs its name and
	// all its arguments.
	ffxPath := filepath.Join(t.TempDir(), "ffx.sh")
	contents := "#!/bin/sh\necho \"$0 $@\"\n"
	if err := os.WriteFile(ffxPath, []byte(contents), 0o700); err != nil {
		t.Fatal(err)
	}

	ffxRunDir := NewRunDir(filepath.Join(t.TempDir(), "ffx-run-dir"))
	ffx, err := NewFFXTool(ffxPath, ffxRunDir)
	if err != nil {
		t.Fatal(err)
	}

	flasher := ffx.Flasher()
	flasher.SetManifest("dir/flash.json")
	setupFlasher(t, flasher)

	stdout, err := flasher.Flash(context.Background())
	if err != nil {
		t.Fatal(err)
	}

	return strings.Trim(strings.ReplaceAll(stdout, ffxPath, "ffx"), "\n")
}

func TestDefault(t *testing.T) {
	result := createAndRunFlasher(t, func(t *testing.T, flasher *Flasher) {})
	expected_result := "target flash --timeout-rate 1 --min-timeout-secs 600 --manifest dir/flash.json"
	if !strings.HasPrefix(result, "ffx") || !strings.HasSuffix(result, expected_result) {
		t.Fatalf("target flash result mismatched: " + result)
	}
}

func TestSSHKeys(t *testing.T) {
	sshKey := generatePublicKey(t)
	result := createAndRunFlasher(t, func(t *testing.T, flasher *Flasher) {
		flasher.SetSSHPublicKey(sshKey)
	})
	segs := strings.Fields(result)
	result = strings.Join(segs[:len(segs)-3], " ")
	expected_result := "target flash --timeout-rate 1 --min-timeout-secs 600 --authorized-keys"
	if !strings.HasPrefix(result, "ffx") || !strings.HasSuffix(result, expected_result) {
		t.Fatalf("target flash result mismatched: " + result)
	}
}

func TestDiscoveryTimeout(t *testing.T) {
	result := createAndRunFlasher(t, func(t *testing.T, flasher *Flasher) {
		flasher.SetDiscoveryTimeout(12000)
	})
	expected_result := "-c discovery.timeout=12000 target flash --timeout-rate 1 --min-timeout-secs 600 --manifest dir/flash.json"
	if !strings.HasPrefix(result, "ffx") || !strings.HasSuffix(result, expected_result) {
		t.Fatalf("target flash result mismatched: " + result)
	}
}
