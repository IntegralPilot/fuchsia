// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffxutil

import (
	"context"
	"fmt"
	"os/exec"
	"path/filepath"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

// EmuTools represent tools used by `ffx emu`. If using tools not included in the SDK,
// their paths should be provided in this struct to EmuStart().
type EmuTools struct {
	Emulator              string
	FVM                   string
	ZBI                   string
	UEFI_arm64            string
	UEFI_x64              string
	SecureBootloaderArm64 string
}

type EmuStartArgs struct {
	// The emulation engine to run with. The currently supported values are
	// `aemu`, `qemu`, and `crosvm`. If empty, it will default to `aemu`.
	Engine string

	// If using a custom config, all other fields below should be empty.
	Config string

	// Extra args to `ffx emu` to be passed during emulator startup
	ExtraEmuArgs []string

	ProductBundle string
	KernelArgs    []string
	Accel         build.AccelMode
	Device        string
}

// EmuStartConsole returns a command to launch the emulator.
func (f *FFXInstance) EmuStartConsole(ctx context.Context, sdkRoot, name string, tools EmuTools, startArgs EmuStartArgs) (*exec.Cmd, error) {
	// If using different tools from the ones in the sdk, they need to be
	// provided in the sdk.overrides in the ffx config.
	toolsToOverride := make(map[string]string)
	if tools.Emulator != "" {
		expectedName := "aemu_internal"
		if startArgs.Engine != "" {
			expectedName = fmt.Sprintf("%s_internal", startArgs.Engine)
		}
		toolsToOverride[expectedName] = tools.Emulator
	}
	if tools.FVM != "" {
		toolsToOverride["fvm"] = tools.FVM
	}
	if tools.ZBI != "" {
		toolsToOverride["zbi"] = tools.ZBI
	}
	if tools.UEFI_arm64 != "" {
		toolsToOverride["uefi_internal_arm64"] = tools.UEFI_arm64
	}
	if tools.UEFI_x64 != "" {
		toolsToOverride["uefi_internal_x64"] = tools.UEFI_x64
	}
	if tools.SecureBootloaderArm64 != "" {
		toolsToOverride["secure_bootloader_arm64"] = tools.SecureBootloaderArm64
	}
	for toolName, toolPath := range toolsToOverride {
		var err error
		toolPath, err = filepath.Abs(toolPath)
		if err != nil {
			return nil, err
		}
		if err := f.ConfigSet(ctx, fmt.Sprintf("sdk.overrides.%s", toolName), toolPath); err != nil {
			return nil, err
		}
	}

	args := []string{"--console", "--net", "tap", "--name", name, "-H", "-s", "0"}
	args = append(args, startArgs.ExtraEmuArgs...)
	if startArgs.Config != "" {
		args = append(args, "--config", startArgs.Config)
	} else {
		args = append(args, startArgs.ProductBundle)
		for _, k := range startArgs.KernelArgs {
			args = append(args, "--kernel-args", k)
		}
		if startArgs.Accel != "" {
			args = append(args, "--accel", string(startArgs.Accel))
		}
		if startArgs.Device != "" {
			args = append(args, "--device", startArgs.Device)
		}
	}
	if startArgs.Engine != "" {
		// For `ffx emu start`, the `aemu` engine is specified as `femu`.
		engine := startArgs.Engine
		if engine == "aemu" {
			engine = "femu"
		}
		args = append(args, "--engine", engine)
	}
	dryRunCommand := append(args, "--dry-run")
	if err := f.EmuStart(dryRunCommand...).run(ctx); err != nil {
		logger.Debugf(ctx, "failed to run dry-run command: %s", err)
	}
	return f.EmuStart(args...).cmd(), nil
}

// EmuStop terminates all emulator instances launched by ffx.
func (f *FFXInstance) EmuStopAll(ctx context.Context) error {
	return f.EmuStop(ctx, "--all")
}
