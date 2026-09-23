// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"fmt"
	"os"

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/classify"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/discover"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/prune"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/report"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
)

// FixCommand implements the `fx check-licenses fix` subcommand, which automatically
// resolves license policy violations (e.g. copyright headers, README attributions,
// missing license exceptions, and allowlists) on disk.
type FixCommand struct {
	fuchsiaDir string
}

func (*FixCommand) Name() string { return "fix" }

func (*FixCommand) Synopsis() string {
	return "Automatically fix compliance issues for a project or file."
}

func (*FixCommand) Usage() string {
	return `fix [<path>]:
  Runs the compliance pipeline on the given path (defaults to //) and attempts to automatically:
  - Add missing Fuchsia copyright headers.
  - Update README.fuchsia files with correct license attributions.
  - Add policy exceptions for projects missing licenses.
  - Add allowlist entries for restricted license patterns.

  Examples:
    fx check-licenses fix vendor/foo/bar
`
}

func (c *FixCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&c.fuchsiaDir, "fuchsia_dir", os.Getenv("FUCHSIA_DIR"), "Location of the fuchsia root directory.")
}

func (c *FixCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	// Parse positional arguments; default to the entire Fuchsia workspace root ("//") if omitted.
	if f.NArg() > 1 {
		fmt.Fprintln(os.Stderr, "Usage: fx check-licenses fix [<path>]")
		return subcommands.ExitUsageError
	}

	targetPath := "//"
	if f.NArg() == 1 {
		targetPath = f.Arg(0)
	}

	// Resolve target workspace context and configuration assembly.
	ic, err := LoadInputContext(c.fuchsiaDir, targetPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	// Always resolve to the enclosing logical project root directory, matching project commands behavior.
	projectRoot, err := ic.ResolveProjectRoot(targetPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error resolving project root for %s: %v\n", targetPath, err)
		return subcommands.ExitFailure
	}

	fmt.Printf("🔍 Starting auto-fix for project %s...\n", projectRoot)

	// Step 1: Initialize analysis stages (Discover, Group, Prune, Classify, Validate).
	discoverer := discover.NewCrawler(ic.FuchsiaDir, ic.Config.Discover)

	boundaryCfg := ic.Config.Boundary
	boundaryCfg.FilesInReadmeOnly = false
	grouper := boundary.NewGrouper(ic.FuchsiaDir, boundaryCfg)

	pruner := prune.NewPruner(nil) // Disable build-graph pruning so all target files are analyzed.

	classifier, err := classify.NewClassifier(ic.Config.Classify)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to initialize classifier: %v\n", err)
		return subcommands.ExitFailure
	}

	validator := validate.NewValidator(ic.FuchsiaDir, ic.Config.Validate)

	fixer := report.NewFixerRenderer(ic.FuchsiaDir, ic.Config)
	renderers := pipeline.MultiRenderer{
		report.NewReadmeWriter(ic.FuchsiaDir, false),
		fixer,
	}

	// Step 3: Run the compliance pipeline scoped to the enclosing project root directory.
	orchestrator := pipeline.NewOrchestrator(discoverer, grouper, pruner, classifier, validator, renderers)
	if err := orchestrator.Run(ctx, []string{projectRoot}); err != nil {
		fmt.Fprintf(os.Stderr, "Pipeline failed: %v\n", err)
		return subcommands.ExitFailure
	}

	// Step 4: Display a summary of applied fixes and required follow-up actions (e.g. OSRB bug assignments).
	fixer.PrintSummary()

	return subcommands.ExitSuccess
}
