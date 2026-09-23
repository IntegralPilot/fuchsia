// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"

	"github.com/google/subcommands"
	"go.fuchsia.dev/fuchsia/tools/orchestrate"
	utils "go.fuchsia.dev/fuchsia/tools/orchestrate/utils"
)

type runCmd struct {
	input        string
	deviceConfig string
	help         bool
	overrides    string
}

func (*runCmd) Name() string {
	return "run"
}

func (*runCmd) Synopsis() string {
	return "Runs fuchsia test in out-of-tree environment"
}

func (*runCmd) Usage() string {
	return "Usage: ./orchestrate run [flags...]"
}

func (r *runCmd) SetFlags(f *flag.FlagSet) {
	f.StringVar(&r.deviceConfig, "device-config", "/etc/botanist/config.json", "File path for device config JSON file.")
	f.StringVar(&r.input, "input", "", "File path for input JSON file.")
	f.BoolVar(&r.help, "help", false, "Print usage for this command.")
	f.StringVar(&r.overrides, "overrides", "", "JSON string configuration overrides (e.g. '{\"emulator\":{\"ffx_path\":\"foo\"}}'). Since these are merged with the base input configuration, changing run targets (eg: emulator to hardware) requires unsetting the previous run target. See config.go for more details.")

}

func (r *runCmd) Execute(ctx context.Context, f *flag.FlagSet, _ ...any) subcommands.ExitStatus {
	if r.help {
		fmt.Println(r.Usage())
		return subcommands.ExitSuccess
	}
	if err := initTestArtifactsDir(); err != nil {
		fmt.Printf("Failed to initialize test artifacts directory: %v\n", err)
		return subcommands.ExitFailure
	}
	oc := orchestrate.NewOrchestrateConfig()
	runInput, err := oc.ReadRunInput(r.input)
	if err != nil {
		fmt.Printf("Reading run input failed: %v\n", err)
		return subcommands.ExitFailure
	}
	if r.overrides != "" {
		if err := json.Unmarshal([]byte(r.overrides), runInput); err != nil {
			fmt.Printf("Applying overrides failed (invalid JSON): %v\n", err)
			return subcommands.ExitFailure
		}
		if err := runInput.Validate(); err != nil {
			fmt.Printf("Applying overrides failed (invalid configuration): %v\n", err)
			return subcommands.ExitFailure
		}
	}
	fmt.Printf("Run input: %+v\n", runInput)
	// Botanist's config.json and serial logging will only be present when
	// testing with hardware.
	var deviceConfig *orchestrate.DeviceConfig
	var serialLog *orchestrate.SerialLogging
	if runInput.IsHardware() {
		deviceConfig, err = oc.ReadDeviceConfig(r.deviceConfig)
		if err != nil {
			fmt.Printf("Failed to read Device Config: %v\n", err)
			return subcommands.ExitFailure
		}
		serialLog, err := orchestrate.StartSerialLogging(deviceConfig)
		if err != nil {
			fmt.Printf("Starting serial logging failed: %v\n", err)
			return subcommands.ExitFailure
		}
		defer serialLog.Stop()
	}
	if err = dumpEnv(); err != nil {
		fmt.Printf("Dumping env failed: %v\n", err)
	}
	runner := orchestrate.NewTestOrchestrator(deviceConfig)
	if serialLog != nil {
		defer serialLog.Symbolize(runner)
	}
	if err := runner.Run(ctx, runInput, f.Args()); err != nil {
		fmt.Printf("Runner failed: %v\n", err)
		return subcommands.ExitFailure
	}
	return subcommands.ExitSuccess
}

func initTestArtifactsDir() error {
	dir := utils.GetOutputsDir()
	if dir == "" {
		fmt.Printf("Neither TEST_UNDECLARED_OUTPUTS_DIR nor ISOLATED_OUTDIR is set, test artifacts will be written to the current working directory.\n")
		return nil
	}
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("os.Mkdir: %w", err)
	}
	return nil
}

func dumpEnv() error {
	dir := utils.GetOutputsDir()
	logFile, err := os.Create(filepath.Join(dir, "env.dump.txt"))
	if err != nil {
		return fmt.Errorf("os.Create: %w", err)
	}
	for _, e := range os.Environ() {
		fmt.Fprintf(logFile, "%v\n", e)
	}
	if err = logFile.Close(); err != nil {
		return fmt.Errorf("logFile.Close: %w", err)
	}
	return nil
}
