// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/metrics"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/util"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/config"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/classify"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/discover"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/prune"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/report"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
)

type GenerateCommand struct {
	configFile           string
	fuchsiaDir           string
	buildDir             string
	outDir               string
	licensesOutDir       string
	gnPath               string
	genProjectFile       string
	genIntermediateFile  string
	checkURLs            bool
	overwriteReadmeFiles bool
	outputLicenseFile    bool
	runAnalysis          bool
	logLevel             int
	runV2                bool
	verifyReadmes        bool
}

func (*GenerateCommand) Name() string { return "generate" }
func (*GenerateCommand) Synopsis() string {
	return "Run the full compliance pipeline and generate reports."
}
func (*GenerateCommand) Usage() string {
	return `generate [options] [<gn_target>]:
	Traverses the repository, executes the Google License Classifier, and generates SPDX/NOTICE files.
`
}

func (p *GenerateCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&p.configFile, "config_file", "", "Root config file path (unused in v2).")
	f.StringVar(&p.fuchsiaDir, "fuchsia_dir", os.Getenv("FUCHSIA_DIR"), "Location of the fuchsia root directory (//).")
	f.StringVar(&p.buildDir, "build_dir", os.Getenv("FUCHSIA_BUILD_DIR"), "Location of GN build directory.")
	f.StringVar(&p.outDir, "out_dir", "/tmp/check-licenses", "Directory to write outputs to.")
	f.StringVar(&p.licensesOutDir, "licenses_out_dir", "", "Directory to write license text segments.")

	f.StringVar(&p.gnPath, "gn_path", "{FUCHSIA_DIR}/prebuilt/third_party/gn/{PLATFORM}/gn", "Path to GN executable. Required when gen_filter_target is specified.")
	f.StringVar(&p.genProjectFile, "gen_project_file", "{BUILD_DIR}/project.json", "Path to 'project.json' output file.")
	f.StringVar(&p.genIntermediateFile, "gen_intermediate_file", "", "Path to intermediate serialized gen struct.")

	f.BoolVar(&p.checkURLs, "check_urls", false, "Flag for enabling checks for license URLs.")
	f.BoolVar(&p.overwriteReadmeFiles, "overwrite_readme_files", false, "Flag for enabling README.fuchsia file overwrites.")

	f.BoolVar(&p.outputLicenseFile, "output_license_file", true, "Flag for enabling template expansions.")
	f.BoolVar(&p.runAnalysis, "run_analysis", true, "Flag for enabling license analysis and 'result' package tests.")

	f.IntVar(&p.logLevel, "log_level", 1, "Log level. Set to 0 for no logs, 1 to log to stdout, 2 to log to stdout+file.")

	f.BoolVar(&p.runV2, "v2", true, "Run the experimental v2 pipeline architecture.")
	f.BoolVar(&p.verifyReadmes, "verify_readmes", false, "Flag for verifying if README.fuchsia files accurately reflect project licenses in v2 pipeline.")
}

func (p *GenerateCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	if err := p.executeImpl(ctx, f); err != nil {
		fmt.Fprintf(os.Stderr, "check-licenses generate: %s\nSee go/fuchsia-licenses-playbook for information on resolving common errors.\n", err)
		return subcommands.ExitFailure
	}
	return subcommands.ExitSuccess
}

func (p *GenerateCommand) executeImpl(ctx context.Context, f *flag.FlagSet) error {
	if err := p.setupLogging(); err != nil {
		return fmt.Errorf("failed to setup logging: %w", err)
	}

	defer metrics.PhaseDuration.Track()()

	fuchsiaDir, _, err := ResolveAndValidatePath(p.fuchsiaDir, ".")
	if err != nil {
		return err
	}
	p.fuchsiaDir = fuchsiaDir

	resolvePath := func(path string, mkdir bool) (string, error) {
		if path == "" {
			return "", nil
		}
		absPath := path
		if !filepath.IsAbs(path) {
			absPath = filepath.Join(fuchsiaDir, path)
		}
		absPath, err = filepath.Abs(absPath)
		if err != nil {
			return "", err
		}
		if mkdir {
			if _, err := os.Stat(absPath); os.IsNotExist(err) {
				if err := os.MkdirAll(absPath, 0755); err != nil {
					return "", err
				}
			}
		}
		return absPath, nil
	}

	p.buildDir, err = resolvePath(p.buildDir, false)
	if err != nil {
		return fmt.Errorf("failed to resolve buildDir: %w", err)
	}

	p.outDir, err = resolvePath(p.outDir, true)
	if err != nil {
		return fmt.Errorf("failed to resolve outDir: %w", err)
	}

	target := "//:default"
	if f.NArg() > 1 {
		return fmt.Errorf("check-licenses takes a maximum of 1 positional argument (filepath or gn target), got %v", f.NArg())
	}
	if f.NArg() == 1 {
		target = f.Arg(0)
	}

	if err := os.Chdir(p.fuchsiaDir); err != nil {
		return err
	}

	if p.outputLicenseFile {
		platform := "linux-x64"
		if runtime.GOOS == "darwin" {
			platform = "mac-x64"
		}
		if len(p.gnPath) > 0 {
			p.gnPath = strings.ReplaceAll(p.gnPath, "{FUCHSIA_DIR}", p.fuchsiaDir)
			p.gnPath = strings.ReplaceAll(p.gnPath, "{PLATFORM}", platform)
			p.gnPath, err = resolvePath(p.gnPath, false)
			if err != nil {
				return fmt.Errorf("failed to resolve gnPath: %w", err)
			}
		}
		p.genProjectFile = strings.ReplaceAll(p.genProjectFile, "{BUILD_DIR}", p.buildDir)
		p.genProjectFile, err = resolvePath(p.genProjectFile, false)
		if err != nil {
			return fmt.Errorf("failed to resolve genProjectFile: %w", err)
		}
	}

	return p.executeV2Pipeline(ctx, target)
}

func (p *GenerateCommand) setupLogging() error {
	logTargets := []io.Writer{}

	if p.logLevel == 1 || p.logLevel == 2 {
		if p.outDir != "" {
			if _, err := os.Stat(p.outDir); os.IsNotExist(err) {
				if err := os.MkdirAll(p.outDir, 0755); err != nil {
					return fmt.Errorf("failed to create out directory [%v]: %w", p.outDir, err)
				}
			}
			logfilePath := filepath.Join(p.outDir, "logs")
			f, err := os.OpenFile(logfilePath, os.O_RDWR|os.O_CREATE|os.O_APPEND, 0666)
			if err != nil {
				return fmt.Errorf("failed to create log file [%v]: %w", logfilePath, err)
			}
			logTargets = append(logTargets, f)
		}
	}

	switch p.logLevel {
	case 0:
		logTargets = append(logTargets, io.Discard)
	case 2:
		logTargets = append(logTargets, os.Stdout)
	}

	w := io.MultiWriter(logTargets...)
	log.SetOutput(w)
	log.SetFlags(0)
	return nil
}

// executeV2Pipeline runs the experimental v2 compliance engine.
func (p *GenerateCommand) executeV2Pipeline(ctx context.Context, target string) error {
	log.Println("Starting v2 fast compliance pipeline...")
	startTime := time.Now()

	endTrack := metrics.TotalRuntime.Track()

	// 1. Assembly Phase
	builder := config.NewBuilder(p.fuchsiaDir)
	if err := builder.Assemble(); err != nil {
		return fmt.Errorf("failed to assemble configuration: %w", err)
	}

	config := builder.Config
	log.Printf("Assembled configuration in %v", time.Since(startTime))

	// We still use the GN parsing logic for now to establish the build graph map
	var validFiles map[string]bool
	if p.outputLicenseFile {
		gnStart := time.Now()
		log.Printf("Generating GN project file to extract build graph... (This may take a while)")

		gn, err := util.NewGn(p.gnPath, p.buildDir)
		if err != nil {
			return err
		}
		if err := gn.GenerateProjectFile(ctx); err != nil {
			return err
		}

		log.Printf("Loading and parsing GN project.json file...")
		gen, err := util.LoadGen(p.genProjectFile)
		if err != nil {
			return err
		}

		log.Printf("Extracting transitive files from build graph...")
		validFiles, err = gen.GetTransitiveFiles(target, p.fuchsiaDir)
		if err != nil {
			return err
		}
		log.Printf("Build graph resolution complete in %v (Found %d valid files)", time.Since(gnStart), len(validFiles))
	}

	// 3. Instantiate Stages
	discoverer := discover.NewCrawler(p.fuchsiaDir, config.Discover)

	// Pass true for filesInReadmeOnly to match current behavior!
	boundaryCfg := config.Boundary
	boundaryCfg.FilesInReadmeOnly = true
	grouper := boundary.NewGrouper(p.fuchsiaDir, boundaryCfg)

	pruner := prune.NewPruner(validFiles)

	classifier, err := classify.NewClassifier(config.Classify)
	if err != nil {
		return fmt.Errorf("failed to initialize classifier: %w", err)
	}

	validator := validate.NewValidator(p.fuchsiaDir, config.Validate)

	var renderers pipeline.MultiRenderer
	if p.overwriteReadmeFiles {
		renderers = append(renderers, report.NewReadmeWriter(p.fuchsiaDir, false))
	} else if p.verifyReadmes {
		renderers = append(renderers, report.NewReadmeVerifier(p.fuchsiaDir))
	}

	if p.outputLicenseFile {
		renderers = append(renderers,
			report.NewNoticeRenderer(p.outDir),
			report.NewSpdxRenderer(p.outDir),
		)
	}
	metricsOutDir := ""
	if p.logLevel >= 2 {
		metricsOutDir = p.outDir
	}
	renderers = append(renderers,
		report.NewMetricsRenderer(metricsOutDir),
		report.NewConsoleErrorReporter(p.fuchsiaDir),
	)

	orchestrator := pipeline.NewOrchestrator(discoverer, grouper, pruner, classifier, validator, renderers)

	if err := orchestrator.Run(ctx, []string{p.fuchsiaDir}); err != nil {
		return fmt.Errorf("pipeline execution failed: %w", err)
	}

	endTrack()

	log.Printf("v2 pipeline completed successfully in %v\n", time.Since(startTime))
	return nil
}
