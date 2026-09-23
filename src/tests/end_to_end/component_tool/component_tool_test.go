// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/emulator"
	"go.fuchsia.dev/fuchsia/tools/emulator/emulatortest"
)

func TestComponentShow(t *testing.T) {
	exPath := execDir(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(exPath, "test_data"), emulator.DistributionParams{Emulator: emulator.Qemu})
	arch := distro.TargetCPU()
	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, "console.shell=true")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()
	i.WaitForLogMessage("console.shell: enabled")
	i.RunCommand("component show archivist")
	i.WaitForLogMessage("bootstrap/archivist")
}

func TestComponentList(t *testing.T) {
	exPath := execDir(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(exPath, "test_data"), emulator.DistributionParams{Emulator: emulator.Qemu})
	arch := distro.TargetCPU()
	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, "console.shell=true")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()
	i.WaitForLogMessage("console.shell: enabled")
	i.RunCommand("component list")
	i.WaitForLogMessage("bootstrap/archivist")
}

func execDir(t *testing.T) string {
	ex, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	return filepath.Dir(ex)
}
