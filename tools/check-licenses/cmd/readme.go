// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"bytes"
	"context"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/config"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/readme"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/discover"
)

type ReadmeCommand struct {
	fuchsiaDir string
}

func (*ReadmeCommand) Name() string     { return "readme" }
func (*ReadmeCommand) Synopsis() string { return "Format or check a README.fuchsia file." }
func (*ReadmeCommand) Usage() string {
	return `readme <subcommand> [options]:
  Manage README.fuchsia files.

  Subcommands:
    format   Formats the README.fuchsia file in-place.
    check    Checks if the README.fuchsia file is valid and formatted.
`
}

func (c *ReadmeCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&c.fuchsiaDir, "fuchsia_dir", os.Getenv("FUCHSIA_DIR"), "Location of the fuchsia root directory (//).")
}

func (c *ReadmeCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	subFlags := flag.NewFlagSet("readme", flag.ContinueOnError)
	if err := subFlags.Parse(f.Args()); err != nil {
		return subcommands.ExitUsageError
	}
	subCommander := subcommands.NewCommander(subFlags, "readme")
	subCommander.Register(&ReadmeFormatCommand{fuchsiaDir: c.fuchsiaDir}, "")
	subCommander.Register(&ReadmeCheckCommand{fuchsiaDir: c.fuchsiaDir}, "")
	subCommander.Register(&ReadmeListCommand{fuchsiaDir: c.fuchsiaDir}, "")
	return subCommander.Execute(ctx)
}

type ReadmeFormatCommand struct {
	fuchsiaDir  string
	printStdout bool
}

func (*ReadmeFormatCommand) Name() string     { return "format" }
func (*ReadmeFormatCommand) Synopsis() string { return "Formats the README.fuchsia file in-place." }
func (*ReadmeFormatCommand) Usage() string {
	return `format [-stdout] <file_path>:
  Formats the README.fuchsia file to match the canonical schema.
  Use -stdout to print the formatted text to stdout without modifying the file.
`
}

func (c *ReadmeFormatCommand) SetFlags(f *flag.FlagSet) {
	f.BoolVar(&c.printStdout, "stdout", false, "Print the formatted text to stdout instead of overwriting the file.")
}

func (c *ReadmeFormatCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	if f.NArg() != 1 {
		fmt.Fprintln(os.Stderr, "Error: exactly one file path must be provided.")
		return subcommands.ExitUsageError
	}

	targetFile := f.Arg(0)
	fuchsiaDir, targetFile, err := ResolveAndValidatePath(c.fuchsiaDir, targetFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	absPath := filepath.Join(fuchsiaDir, targetFile)

	data, err := os.ReadFile(absPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to read file: %v\n", err)
		return subcommands.ExitFailure
	}

	readmes, err := readme.Parse(data)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to parse README: %v\n", err)
		return subcommands.ExitFailure
	}

	if len(readmes) == 0 {
		fmt.Fprintf(os.Stderr, "Warning: parsed 0 readmes from file\n")
		return subcommands.ExitSuccess
	}

	formattedText := readme.Format(readmes)
	formattedBytes := []byte(formattedText)

	builder := config.NewBuilder(fuchsiaDir)
	if err := builder.Assemble(); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to assemble config: %v\n", err)
		return subcommands.ExitFailure
	}

	errs := readme.Validate(fuchsiaDir, absPath, readmes, builder.Config)
	hasValidationErrors := len(errs) > 0
	for _, err := range errs {
		fmt.Fprintf(os.Stderr, "%v\n", err)
	}

	if c.printStdout {
		fmt.Print(formattedText)
		if hasValidationErrors {
			return subcommands.ExitFailure
		}
		return subcommands.ExitSuccess
	}

	if bytes.Equal(data, formattedBytes) {
		if hasValidationErrors {
			return subcommands.ExitFailure
		}
		fmt.Printf("✅ README is already formatted: %s\n", targetFile)
		return subcommands.ExitSuccess
	}

	if err := os.WriteFile(absPath, formattedBytes, 0644); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to write formatted README: %v\n", err)
		return subcommands.ExitFailure
	}

	if hasValidationErrors {
		return subcommands.ExitFailure
	}
	fmt.Printf("✏️  Successfully formatted README: %s\n", targetFile)
	return subcommands.ExitSuccess
}

type ReadmeCheckCommand struct {
	fuchsiaDir string
}

func (*ReadmeCheckCommand) Name() string { return "check" }
func (*ReadmeCheckCommand) Synopsis() string {
	return "Checks if the README.fuchsia file is valid and formatted."
}
func (*ReadmeCheckCommand) Usage() string {
	return `check <file_path>:
  Checks if the README.fuchsia file is perfectly formatted and contains no unknown fields.
`
}

func (c *ReadmeCheckCommand) SetFlags(f *flag.FlagSet) {}

func (c *ReadmeCheckCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	if f.NArg() != 1 {
		fmt.Fprintln(os.Stderr, "Error: exactly one file path must be provided.")
		return subcommands.ExitUsageError
	}

	targetFile := f.Arg(0)
	fuchsiaDir, targetFile, err := ResolveAndValidatePath(c.fuchsiaDir, targetFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	absPath := filepath.Join(fuchsiaDir, targetFile)

	data, err := os.ReadFile(absPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to read file: %v\n", err)
		return subcommands.ExitFailure
	}

	readmes, err := readme.Parse(data)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to parse README: %v\n", err)
		return subcommands.ExitFailure
	}

	if len(readmes) == 0 {
		fmt.Fprintf(os.Stderr, "Warning: parsed 0 readmes from file\n")
		return subcommands.ExitSuccess
	}

	builder := config.NewBuilder(fuchsiaDir)
	if err := builder.Assemble(); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to assemble config: %v\n", err)
		return subcommands.ExitFailure
	}

	errs := readme.Validate(fuchsiaDir, absPath, readmes, builder.Config)
	hasValidationErrors := len(errs) > 0
	for _, err := range errs {
		fmt.Fprintf(os.Stderr, "%v\n", err)
	}

	if hasValidationErrors {
		return subcommands.ExitFailure
	}

	fmt.Printf("✅ README is perfectly formatted and contains no unknown fields: %s\n", targetFile)
	return subcommands.ExitSuccess
}

type ReadmeListCommand struct {
	fuchsiaDir string
}

func (*ReadmeListCommand) Name() string { return "list" }
func (*ReadmeListCommand) Synopsis() string {
	return "Lists all README.fuchsia files discovered in the repository."
}
func (*ReadmeListCommand) Usage() string {
	return `list:
  Lists the relative paths (from the fuchsia root) to all physical and virtual README.fuchsia files.
`
}

func (c *ReadmeListCommand) SetFlags(f *flag.FlagSet) {}

func (c *ReadmeListCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	fuchsiaDir, _, err := ResolveAndValidatePath(c.fuchsiaDir, ".")
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	builder := config.NewBuilder(fuchsiaDir)
	if err := builder.Assemble(); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to assemble configuration: %v\n", err)
		return subcommands.ExitFailure
	}
	cfg := builder.Config

	var allReadmes []string

	for _, physPath := range cfg.Boundary.OutOfTreeReadmes {
		rel, err := filepath.Rel(fuchsiaDir, physPath)
		if err == nil {
			allReadmes = append(allReadmes, rel)
		}
	}

	discoverer := discover.NewCrawler(fuchsiaDir, cfg.Discover)
	rawPaths, err := discoverer.Run(ctx, []string{fuchsiaDir})
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to crawl repository: %v\n", err)
		return subcommands.ExitFailure
	}

	for rp := range rawPaths {
		if rp.IsDir {
			continue
		}
		if filepath.Base(rp.Path) == "README.fuchsia" {
			rel, err := filepath.Rel(fuchsiaDir, rp.Path)
			if err == nil {
				allReadmes = append(allReadmes, rel)
			}
		}
	}

	sort.Strings(allReadmes)
	last := ""
	for _, r := range allReadmes {
		if r != last {
			fmt.Println(r)
			last = r
		}
	}

	return subcommands.ExitSuccess
}
