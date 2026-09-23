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
	"sort"
	"strings"
	"time"

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/classify"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/discover"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/prune"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/report"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
)

type ProjectUpdateCommand struct {
	fuchsiaDir  string
	printStdout bool
	fileList    string
	fast        bool
}

func (*ProjectUpdateCommand) Name() string { return "update" }
func (*ProjectUpdateCommand) Synopsis() string {
	return "Automatically updates the License File declarations in a README.fuchsia."
}
func (*ProjectUpdateCommand) Usage() string {
	return `update [--fast] [-stdout] [-file-list <path>] <paths...>:
  Automatically updates the License File declarations in a README.fuchsia.
  Use -stdout to print result to stdout instead of overwriting file.
  Use -file-list to specify a file containing paths to update, one per line.
  Use --fast to only evaluate declared license files and explicit targets.
`
}

func (c *ProjectUpdateCommand) SetFlags(f *flag.FlagSet) {
	f.BoolVar(&c.printStdout, "stdout", false, "Print the updated README to stdout instead of overwriting the file.")
	f.StringVar(&c.fileList, "file-list", "", "Path to a file containing a list of file paths to update, one per line.")
	f.BoolVar(&c.fast, "fast", false, "Fast mode: only update files declared in README.fuchsia and target paths, avoiding full directory recursion.")
}

func (c *ProjectUpdateCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	// Step 1: Load target paths and repository input context.
	inputPaths, err := LoadTargets(c.fileList, c.fuchsiaDir, f.Args())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitUsageError
	}

	inputCtx, err := LoadInputContext(c.fuchsiaDir, inputPaths[0])
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	// Step 2: Initialize pipeline stages for project updating.
	classifier, err := classify.NewClassifier(inputCtx.Config.Classify)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to initialize classifier: %v\n", err)
		return subcommands.ExitFailure
	}

	boundaryCfg := inputCtx.Config.Boundary
	boundaryCfg.FilesInReadmeOnly = false
	grouper := boundary.NewGrouper(inputCtx.FuchsiaDir, boundaryCfg)
	validator := validate.NewValidator(inputCtx.FuchsiaDir, inputCtx.Config.Validate)

	// Step 3: Group input targets by project root and execute Orchestrator with Stage 6 ReadmeWriter.
	// We map each input target path to its enclosing project root so that multiple files belonging to
	// the same project are processed together in a single pipeline run.
	projectTargets := make(map[string][]string)
	hasErrors := false

	for _, inputPath := range inputPaths {
		projectRoot, err := inputCtx.ResolveProjectRoot(inputPath)
		if err != nil {
			fmt.Fprintf(os.Stderr, "❌ Error: %v\n", err)
			hasErrors = true
			continue
		}
		projectTargets[projectRoot] = append(projectTargets[projectRoot], inputPath)
	}

	// Sort project roots to ensure deterministic execution ordering.
	var projectRoots []string
	for root := range projectTargets {
		projectRoots = append(projectRoots, root)
	}
	sort.Strings(projectRoots)

	for _, projectRoot := range projectRoots {
		targets := projectTargets[projectRoot]

		// First-party projects are governed by virtual READMEs and do not require in-tree manifests.
		r, bestReadmePath, err := grouper.FindProjectReadme(projectRoot)
		if err == nil && r != nil && r.FirstParty == "yes" {
			if !c.printStdout {
				fmt.Printf("ℹ️  Skipping 1st-party project %s (governed by virtual README)\n", projectRoot)
			}
			continue
		}
		startTime := time.Now()

		var renderers pipeline.MultiRenderer
		writer := report.NewReadmeWriter(inputCtx.FuchsiaDir, c.printStdout)
		// When fast mode is active, tell ReadmeWriter to preserve existing license file entries
		// and NOTICE.fuchsia files rather than resetting them, preventing unclassified files from being lost.
		writer.PreserveExisting = c.fast
		renderers = append(renderers, writer)
		if !c.printStdout {
			renderers = append(renderers, report.NewSummaryMetricsPrinter(inputCtx.FuchsiaDir, startTime))
		}

		disc := discover.NewCrawler(inputCtx.FuchsiaDir, inputCtx.Config.Discover)
		// When fast mode is requested, configure the pruner to keep only the files explicitly
		// declared in the governing README or explicitly targeted, skipping unnecessary files.
		iterPruner := prune.NewPruner(nil)
		iterPruner.FilesInReadmeOnly = c.fast
		iterPruner.TargetFiles = targets
		iterPruner.FuchsiaDir = inputCtx.FuchsiaDir

		// If the target path points to a subdirectory of the project, widen crawlRoot to the directory
		// containing the governing README so that the README and its notices can be found and updated.
		crawlRoot := projectRoot
		if err == nil && bestReadmePath != "" && strings.HasPrefix(bestReadmePath, inputCtx.FuchsiaDir) {
			readmeDir := filepath.Dir(bestReadmePath)
			if strings.HasPrefix(projectRoot, readmeDir) {
				crawlRoot = readmeDir
			}
		}

		// Execute the pipeline on the crawlRoot to discover, group, prune, classify, and write out updates.
		orchestrator := pipeline.NewOrchestrator(disc, grouper, iterPruner, classifier, validator, renderers)
		if err := orchestrator.Run(ctx, []string{crawlRoot}); err != nil {
			fmt.Fprintf(os.Stderr, "Failed to update project %s: %v\n", projectRoot, err)
			hasErrors = true
			continue
		}
	}

	// Step 4: Return overall success or failure status.
	if hasErrors {
		return subcommands.ExitFailure
	}
	return subcommands.ExitSuccess
}
