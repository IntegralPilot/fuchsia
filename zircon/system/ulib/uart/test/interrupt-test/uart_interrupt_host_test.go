// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package uart_interrupt_test

import (
	"context"
	"math/rand"
	"os"
	"path/filepath"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/tools/emulator"
	"go.fuchsia.dev/fuchsia/tools/emulator/emulatortest"
)

const (
	// LINT.IfChange

	// String printed when the test is listening for characters in serial.
	serialInputReady string = "uart-interrupt-test: Uart Ready"
	// LINT.ThenChange(./uart-interrupt-test.cc)

	zbiName string = "uart-interrupt-test-zbi.eng"
)

func getCwd(t *testing.T) string {
	ex, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	return filepath.Dir(ex)
}

func GenRandomMessage(length int, seed int64) string {
	var random *rand.Rand = rand.New(rand.NewSource(seed))
	// Codering for generating random string.
	const charset string = "acbdefghijklmnopqrstuvwxyz" + "ABCDEFGHIJKLMNOPQRSTUVWXYZ" + "0123456789"

	bytes := make([]byte, length)
	for i := range bytes {
		bytes[i] = charset[random.Intn(len(charset))]
	}

	return string(bytes)
}

func TestLegacyUartSmallMessage(t *testing.T) {
	seed := time.Now().UnixNano()
	t.Logf("seed=%v", seed)
	msg := GenRandomMessage(128, seed)
	t.Logf("msg=%v", msg)
	cwd := getCwd(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(cwd, "test_data"), emulator.DistributionParams{Emulator: emulator.Qemu})
	arch := distro.TargetCPU()
	t.Log(arch)

	// TODO(https://fxbug.dev/42079799): Disabled for legacy x86 uart driver.
	if arch == "x64" {
		t.Skip()
	}

	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, "uart_test.message_size=128", "kernel.enable-debugging-syscalls=true", "kernel.enable-serial-syscalls=true")

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()

	i.WaitForLogMessages([]string{serialInputReady})
	t.Log("Serial Ready detected. Writing serial message.")
	i.RunCommand(msg)
	t.Log("Serial message written. Waiting for echo.")
	i.WaitForLogMessages([]string{msg})
}

func TestLegacyUartLargeMessage(t *testing.T) {
	seed := time.Now().UnixNano()
	t.Logf("seed=%v", seed)
	msg := GenRandomMessage(2048, seed)
	t.Logf("msg=%v", msg)
	cwd := getCwd(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(cwd, "test_data"), emulator.DistributionParams{Emulator: emulator.Qemu})
	arch := distro.TargetCPU()

	// TODO(https://fxbug.dev/42079799): Disabled for legacy x86 uart driver.
	if arch == "x64" {
		t.Skip()
	}

	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, "uart_test.message_size=2048", "kernel.enable-debugging-syscalls=true", "kernel.enable-serial-syscalls=true")

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()

	i.WaitForLogMessages([]string{serialInputReady})
	t.Log("Serial Ready detected. Writing serial message.")
	i.RunCommand(msg)
	t.Log("Serial message written. Waiting for echo.")
	i.WaitForLogMessages([]string{msg})
}

func TestMigratedUartSmallMessage(t *testing.T) {
	seed := time.Now().UnixNano()
	t.Logf("seed=%v", seed)
	msg := GenRandomMessage(128, seed)
	t.Logf("msg=%v", msg)
	cwd := getCwd(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(cwd, "test_data"), emulator.DistributionParams{Emulator: emulator.Qemu})
	arch := distro.TargetCPU()
	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, "uart_test.message_size=128", "kernel.enable-debugging-syscalls=true", "kernel.enable-serial-syscalls=true", "kernel.experimental.serial_migration=true")

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()

	i.WaitForLogMessages([]string{serialInputReady})
	t.Log("Serial Ready detected. Writing serial message.")
	i.RunCommand(msg)
	t.Log("Serial message written. Waiting for echo.")
	i.WaitForLogMessages([]string{msg})
}

func TestMigratedUartLargeMessage(t *testing.T) {
	seed := time.Now().UnixNano()
	t.Logf("seed=%v", seed)
	msg := GenRandomMessage(2048, seed)
	t.Logf("msg=%v", msg)
	cwd := getCwd(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(cwd, "test_data"), emulator.DistributionParams{Emulator: emulator.Qemu})
	arch := distro.TargetCPU()
	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, "uart_test.message_size=2048", "kernel.enable-debugging-syscalls=true", "kernel.enable-serial-syscalls=true", "kernel.experimental.serial_migration=true")

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()

	i.WaitForLogMessages([]string{serialInputReady})
	t.Log("Serial Ready detected. Writing serial message.")
	i.RunCommand(msg)
	t.Log("Serial message written. Waiting for echo.")
	i.WaitForLogMessages([]string{msg})
}
