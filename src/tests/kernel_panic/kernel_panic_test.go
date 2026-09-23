// Copyright 2020 The Fuchsia Authors. All rights reserved.
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

var cmdline = []string{
	"kernel.halt-on-panic=true",
	"kernel.bypass-debuglog=true",
	"zircon.autorun.boot=/boot/bin/sh+-c+k",
	// The parent build configuration may have the pmm-checker already enabled in its command line,
	// which conflicts with our desire to turn it on with specific settings, so re-set it to be
	// disabled at startup.
	"kernel.pmm-checker.enable=false",
}

// See that `k crash` crashes the kernel.
func TestBasicCrash(t *testing.T) {
	exDir := execDir(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(exDir, "test_data"), emulator.DistributionParams{
		Emulator: emulator.Qemu,
	})
	arch := distro.TargetCPU()
	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, cmdline...)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()

	// Wait for the system to finish booting.
	i.WaitForLogMessage("usage: k <command>")

	// Crash the kernel.
	i.RunCommand("k crash deref")

	// See that it panicked.
	i.WaitForLogMessage("ZIRCON KERNEL PANIC")
	i.WaitForLogMessage("{{{bt:0:")
}

// See that reading a userspace page from the kernel is fatal.
func TestReadUserMemoryViolation(t *testing.T) {
	exDir := execDir(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(exDir, "test_data"), emulator.DistributionParams{
		Emulator: emulator.Qemu,
	})
	arch := distro.TargetCPU()
	if arch != emulator.X64 {
		// TODO(https://fxbug.dev/42137335): Enable this test once we have PAN support.
		t.Skip("Skipping test. This test only supports x64 targets.")
	}

	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, cmdline...)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()

	// Wait for the system to finish booting.
	i.WaitForLogMessage("usage: k <command>")

	// Crash the kernel by causing a userspace data read.
	i.RunCommand("k crash user_read")

	// See that an SMAP failure was identified and that the kernel panicked.
	i.WaitForLogMessageAssertNotSeen("SMAP failure", "cpu does not support smap; will not crash")
	i.WaitForLogMessage("ZIRCON KERNEL PANIC")
	i.WaitForLogMessage("{{{bt:0:")
}

// See that executing a userspace page from the kernel is fatal.
func TestExecuteUserMemoryViolation(t *testing.T) {
	exDir := execDir(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(exDir, "test_data"), emulator.DistributionParams{
		Emulator: emulator.Qemu,
	})
	arch := distro.TargetCPU()

	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, cmdline...)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()

	// Wait for the system to finish booting.
	i.WaitForLogMessage("usage: k <command>")

	// Crash the kernel by causing a userspace code execution.
	i.RunCommand("k crash user_execute")

	i.WaitForLogMessage("ZIRCON KERNEL PANIC")
	if arch == emulator.X64 {
		i.WaitForLogMessage("page fault in kernel mode")
	} else {
		i.WaitForLogMessage("instruction abort in kernel mode")
	}
	i.WaitForLogMessage("{{{bt:0:")
}

// Common helper for verifying that the pmm checker can detect pmm free list corruption.
func pmmCheckerTestCommon(t *testing.T, ctx context.Context, check_action string) *emulatortest.Instance {
	exDir := execDir(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(exDir, "test_data"), emulator.DistributionParams{
		Emulator: emulator.Qemu,
	})
	arch := distro.TargetCPU()
	if arch != emulator.X64 {
		t.Skip("Skipping test. This test only supports x64 targets.")
	}

	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, cmdline...)
	i := distro.NewInstance(ctx, device)
	i.Start()

	// Wait for the system to finish booting.
	i.WaitForLogMessage("usage: k <command>")

	// This test is incompatible with Address Sanitizer.
	i.RunCommand("k build_instrumentation")
	const kasan = "build_instrumentation: address_sanitizer"
	if match := i.WaitForAnyLogMessage(kasan, "build_instrumentation: done"); match == kasan {
		t.Skipf("Skipping test. This test is incompatible with Address Sanitizer")
	}

	// Enable the pmm checker with requested action.
	i.RunCommand("k pmm checker enable 4096 " + check_action)
	i.WaitForLogMessage("pmm checker enabled")

	// Corrupt the free list.
	i.RunCommand("k crash pmm_use_after_free")
	i.WaitForLogMessage("crash_pmm_use_after_free done")

	// Force a check.
	i.RunCommand("k pmm checker check")

	return i
}

// Verify the oops action.
func TestPmmCheckerOops(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := pmmCheckerTestCommon(t, ctx, "oops")
	// See that the corruption is detected and triggered an oops.
	i.WaitForLogMessage("ZIRCON KERNEL OOPS")
	i.WaitForLogMessage("pmm checker found unexpected pattern in page at")
	i.WaitForLogMessage("dump of page follows")
}

// Verify the panic action.
func TestPmmCheckerPanic(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := pmmCheckerTestCommon(t, ctx, "panic")
	// See that the corruption is detected and triggers a panic.
	i.WaitForLogMessage("ZIRCON KERNEL PANIC")
	i.WaitForLogMessage("pmm checker found unexpected pattern in page at")
	i.WaitForLogMessage("dump of page follows")
}

// See that `k crash assert` crashes the kernel.
func TestCrashAssert(t *testing.T) {
	exDir := execDir(t)
	distro := emulatortest.UnpackFrom(t, filepath.Join(exDir, "test_data"), emulator.DistributionParams{
		Emulator: emulator.Qemu,
	})
	arch := distro.TargetCPU()
	device := emulator.DefaultVirtualDevice(string(arch))
	device.KernelArgs = append(device.KernelArgs, cmdline...)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	i := distro.NewInstance(ctx, device)
	i.Start()

	// Wait for the system to finish booting.
	i.WaitForLogMessage("usage: k <command>")

	// Crash the kernel.
	i.RunCommand("k crash assert")

	// See that it panicked.
	i.WaitForLogMessage("ZIRCON KERNEL PANIC")

	// See that it was an assert failure and that the assert message was printed.
	i.WaitForLogMessage("ASSERT FAILED")
	i.WaitForLogMessage("value 42")
}

func execDir(t *testing.T) string {
	ex, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	return filepath.Dir(ex)
}
