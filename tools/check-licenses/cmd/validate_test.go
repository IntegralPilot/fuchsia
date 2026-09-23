// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/google/subcommands"
)

func TestValidateCommand_Execute(t *testing.T) {
	origDir, _ := os.Getwd()
	defer os.Chdir(origDir)

	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	outDir := filepath.Join(tempDir, "out")
	cmd := &ValidateCommand{}

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"-fuchsia_dir", tempDir, "-out_dir", outDir, "-log_level", "2"})

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	if status := cmd.Execute(ctx, fs); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess, got %v", status)
	}

	metricsFile := filepath.Join(outDir, "metrics.json")
	if _, err := os.Stat(metricsFile); os.IsNotExist(err) {
		t.Errorf("Expected metrics.json to be generated at %s", metricsFile)
	}
}

func TestValidateCommand_DefaultLogLevel_NoMetricsFile(t *testing.T) {
	origDir, _ := os.Getwd()
	defer os.Chdir(origDir)

	tempDir := t.TempDir()
	scaffoldV2Config(t, tempDir)

	outDir := filepath.Join(tempDir, "out")
	cmd := &ValidateCommand{}

	fs := flag.NewFlagSet("test", flag.ContinueOnError)
	cmd.SetFlags(fs)
	fs.Parse([]string{"-fuchsia_dir", tempDir, "-out_dir", outDir})

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	if status := cmd.Execute(ctx, fs); status != subcommands.ExitSuccess {
		t.Errorf("Expected ExitSuccess, got %v", status)
	}

	metricsFile := filepath.Join(outDir, "metrics.json")
	if _, err := os.Stat(metricsFile); !os.IsNotExist(err) {
		t.Errorf("Expected metrics.json to NOT be generated at %s for default log_level 1", metricsFile)
	}
}
