// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"path/filepath"

	"github.com/google/subcommands"
)

type AllowlistCommand struct {
	fuchsiaDir string
}

func (*AllowlistCommand) Name() string     { return "allowlist" }
func (*AllowlistCommand) Synopsis() string { return "Manage allowed licenses." }
func (*AllowlistCommand) Usage() string {
	return `allowlist SUBCOMMAND [options]:
  Manage allowed licenses.

  Subcommands:
    add   Add a new allowed license entry.
`
}

func (c *AllowlistCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&c.fuchsiaDir, "fuchsia_dir", os.Getenv("FUCHSIA_DIR"), "Location of the fuchsia root directory.")
}

func (c *AllowlistCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	subFlags := flag.NewFlagSet("allowlist", flag.ContinueOnError)
	if err := subFlags.Parse(f.Args()); err != nil {
		return subcommands.ExitUsageError
	}
	subCommander := subcommands.NewCommander(subFlags, "allowlist")
	subCommander.Register(&AllowlistAddCommand{fuchsiaDir: c.fuchsiaDir}, "")
	return subCommander.Execute(ctx)
}

type AllowlistAddCommand struct {
	fuchsiaDir  string
	bug         string
	description string
}

func (*AllowlistAddCommand) Name() string     { return "add" }
func (*AllowlistAddCommand) Synopsis() string { return "Add an allowed license entry." }
func (*AllowlistAddCommand) Usage() string {
	return `add -bug BUG_ID [-desc DESCRIPTION] LICENSE_NAME PROJECT_PATH:
  Adds an allowed license exception for the given project path.

  Flags:
    -bug  Bug ID tracking this exception (Mandatory).
    -desc Optional description for this exception.

  Examples:
    fx check-licenses allowlist add -bug b/123 GPL-2.0 vendor/foo
`
}

func (c *AllowlistAddCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&c.bug, "bug", "", "Bug ID tracking this exception (Mandatory).")
	f.StringVar(&c.description, "desc", "Auto-generated allowlist entry", "Optional description for this exception.")
}

func (c *AllowlistAddCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	cmdStr, misplacedFlags := ReconstructCommand("allowlist add", f.Args(), []string{"LICENSE_NAME", "PROJECT_PATH"}, c.bug, c.description)
	if misplacedFlags || f.NArg() != 2 {
		if misplacedFlags {
			fmt.Fprintln(os.Stderr, "❌ Error: Flags (like -bug or -desc) must be placed BEFORE positional arguments.")
		} else {
			fmt.Fprintln(os.Stderr, "❌ Error: Invalid number of arguments.")
		}
		fmt.Fprintf(os.Stderr, "Try running this copy-pasteable command instead:\n    %s\n\n", cmdStr)
		return subcommands.ExitUsageError
	}

	if c.bug == "" {
		fmt.Fprintln(os.Stderr, "Error: the -bug flag is mandatory.")
		return subcommands.ExitUsageError
	}

	licenseName := f.Arg(0)
	projectPath := filepath.Clean(f.Arg(1))

	if _, err := AddAllowlistEntry(c.fuchsiaDir, licenseName, projectPath, c.bug, c.description); err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	return subcommands.ExitSuccess
}

// AddAllowlistEntry adds an allowed license exception for the governing enclosing project root of a path.
func AddAllowlistEntry(fuchsiaDir, licenseName, projectPath, bug, description string) (string, error) {
	ic, err := LoadInputContext(fuchsiaDir, projectPath)
	if err != nil {
		return "", fmt.Errorf("failed to load input context: %w", err)
	}

	projectRoot, err := ic.ResolveProjectRoot(projectPath)
	if err != nil {
		projectRoot = filepath.Join(ic.FuchsiaDir, filepath.Clean(projectPath))
	}

	relProjectRoot, err := filepath.Rel(ic.FuchsiaDir, projectRoot)
	if err != nil {
		relProjectRoot = projectRoot
	}

	// Check if this project already has an exception for this license
	if list, ok := ic.Config.Validate.AllowedLicenses[licenseName]; ok {
		if _, exists := list[relProjectRoot]; exists {
			fmt.Printf("Project '%s' already has an allowlist entry for '%s'. Nothing to do.\n", relProjectRoot, licenseName)
			return "", nil
		}
	}

	destFile, err := ic.Config.AddAllowlistEntry(relProjectRoot, licenseName, bug, description)
	if err != nil {
		return "", err
	}

	fmt.Printf("✅ Added Allowlist Entry:\n")
	fmt.Printf("  - License: %s\n", licenseName)
	fmt.Printf("  - Project: %s\n", relProjectRoot)
	fmt.Printf("  - Bug:     %s\n", bug)
	fmt.Printf("  - File:    %s\n\n", destFile)

	return destFile, nil
}
