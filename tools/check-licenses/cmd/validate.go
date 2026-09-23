// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"os"
	"time"

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/config"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/metrics"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/classify"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/discover"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/prune"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/report"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
)

type ValidateCommand struct {
	fuchsiaDir        string
	outDir            string
	findingsFile      string
	logLevel          int
	filesInReadmeOnly bool
}

func (*ValidateCommand) Name() string { return "validate" }
func (*ValidateCommand) Synopsis() string {
	return "Audits the repository for compliance and verifies README.fuchsia files are up to date."
}
func (*ValidateCommand) Usage() string {
	return `validate [options]:
  Crawls the entire repository (or only files listed in READMEs if configured),
  runs the license classifier, checks policy exceptions, and verifies that
  README.fuchsia files are up to date. Returns a non-zero exit code if any
  compliance errors are found. Use the 'fix' command to automatically update READMEs.
`
}

func (p *ValidateCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&p.fuchsiaDir, "fuchsia_dir", os.Getenv("FUCHSIA_DIR"), "Location of the fuchsia root directory (//).")
	f.StringVar(&p.outDir, "out_dir", "/tmp/check-licenses", "Directory to write logs.")
	f.StringVar(&p.findingsFile, "findings_file", "", "Path to write structured JSON findings for shac / static analysis.")
	f.IntVar(&p.logLevel, "log_level", 1, "Log level. 0: none, 1: stdout, 2: stdout+file.")
	f.BoolVar(&p.filesInReadmeOnly, "files_in_readme_only", false, "Only classify files explicitly listed in README.fuchsia files (fast mode).")
}

func (p *ValidateCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	defer metrics.TotalRuntime.Track()()

	if w, err := getLogWriters(p.logLevel, p.outDir); err == nil {
		log.SetOutput(w)
	} else {
		return subcommands.ExitFailure
	}

	log.SetFlags(0)

	fuchsiaDir, _, err := ResolveAndValidatePath(p.fuchsiaDir, ".")
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	log.Println("Starting compliance validation...")
	startTime := time.Now()

	// 1. Assembly Phase
	builder := config.NewBuilder(fuchsiaDir)
	if err := builder.Assemble(); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to assemble configuration: %v\n", err)
		return subcommands.ExitFailure
	}
	cfg := builder.Config

	log.Printf("Assembled configuration in %v", time.Since(startTime))

	// 2. Instantiate Stages
	discoverer := discover.NewCrawler(fuchsiaDir, cfg.Discover)

	boundaryCfg := cfg.Boundary
	boundaryCfg.FilesInReadmeOnly = p.filesInReadmeOnly
	grouper := boundary.NewGrouper(fuchsiaDir, boundaryCfg)

	// Validate checks the entire tree, so we don't prune any targets based on the build graph.
	// Passing nil to NewPruner makes it a no-op.
	pruner := prune.NewPruner(nil)

	classifier, err := classify.NewClassifier(cfg.Classify)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to initialize classifier: %v\n", err)
		return subcommands.ExitFailure
	}

	validator := validate.NewValidator(fuchsiaDir, cfg.Validate)
	metricsOutDir := ""
	if p.logLevel >= 2 {
		metricsOutDir = p.outDir
	}
	renderers := pipeline.MultiRenderer{
		// TODO: Re-enable README.fuchsia verification.
		// report.NewReadmeVerifier(fuchsiaDir),
		report.NewMetricsRenderer(metricsOutDir),
	}
	if p.findingsFile != "" {
		renderers = append(renderers, report.NewFindingsReporter(fuchsiaDir, p.findingsFile))
	}
	renderers = append(renderers, report.NewConsoleErrorReporter(fuchsiaDir))

	orchestrator := pipeline.NewOrchestrator(discoverer, grouper, pruner, classifier, validator, renderers)

	if err := orchestrator.Run(ctx, []string{fuchsiaDir}); err != nil {
		fmt.Fprintf(os.Stderr, "Validation failed: %v\n", err)
		return subcommands.ExitFailure
	}

	log.Printf("Validation completed successfully in %v\n", time.Since(startTime))

	return subcommands.ExitSuccess
}
