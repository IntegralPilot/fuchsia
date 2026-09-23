// Copyright 2026 The Fuchsia Authors. All rights reserved.
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
	"sort"
	"strings"

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/classify"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/discover"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/prune"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/report"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
)

const (
	CheckResolveProjectRootError = "ResolveProjectRootError"
	CheckPipelineExecutionError  = "PipelineExecutionError"
)

type ProjectCheckCommand struct {
	fuchsiaDir   string
	fileList     string
	fast         bool
	format       string
	findingsFile string
}

func (*ProjectCheckCommand) Name() string { return "check" }
func (*ProjectCheckCommand) Synopsis() string {
	return "Analyzes specific files and validates them against their parent README.fuchsia."
}
func (*ProjectCheckCommand) Usage() string {
	return `check [--fast] [--format=text|json] [-findings_file <path>] [-file-list <path>] <files...>:
  Checks if the specified files are declared in their parent README.fuchsia.
  Use -file-list to specify a file containing paths to check, one per line.
  Use --fast to only evaluate declared license files and explicit targets.
  Use --format=json to output structured machine-readable findings with replacements.
  Use -findings_file to write structured findings to a file.
`
}

func (c *ProjectCheckCommand) SetFlags(f *flag.FlagSet) {
	defaultFuchsiaDir := c.fuchsiaDir
	if defaultFuchsiaDir == "" {
		defaultFuchsiaDir = os.Getenv("FUCHSIA_DIR")
	}
	f.StringVar(&c.fuchsiaDir, "fuchsia_dir", defaultFuchsiaDir, "Location of the fuchsia root directory (//).")
	f.StringVar(&c.fileList, "file-list", "", "Path to a file containing a list of file paths to check, one per line.")
	f.BoolVar(&c.fast, "fast", false, "Fast mode: only check files declared in README.fuchsia and target paths, avoiding full directory recursion.")
	f.StringVar(&c.format, "format", "text", "Output format: text or json.")
	f.StringVar(&c.findingsFile, "findings_file", "", "Path to write structured JSON findings for shac / static analysis.")
}

func (c *ProjectCheckCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	allFindings := []report.Finding{}
	hasErrors := false

	outputFindings := func() {
		allFindings = report.DeduplicateFindings(allFindings)
		sort.SliceStable(allFindings, func(i, j int) bool {
			if allFindings[i].FilePath != allFindings[j].FilePath {
				return allFindings[i].FilePath < allFindings[j].FilePath
			}
			if allFindings[i].Line != allFindings[j].Line {
				return allFindings[i].Line < allFindings[j].Line
			}
			if allFindings[i].EndLine != allFindings[j].EndLine {
				return allFindings[i].EndLine < allFindings[j].EndLine
			}
			return allFindings[i].CheckName < allFindings[j].CheckName
		})

		var data []byte
		if c.findingsFile != "" || c.format == "json" {
			var err error
			data, err = json.MarshalIndent(allFindings, "", "  ")
			if err != nil {
				fmt.Fprintf(os.Stderr, "Error marshaling JSON findings: %v\n", err)
			}
		}

		if c.findingsFile != "" && data != nil {
			if dir := filepath.Dir(c.findingsFile); dir != "" && dir != "." {
				if err := os.MkdirAll(dir, 0755); err != nil {
					fmt.Fprintf(os.Stderr, "Error creating findings directory: %v\n", err)
				}
			}
			if err := os.WriteFile(c.findingsFile, append(data, '\n'), 0644); err != nil {
				fmt.Fprintf(os.Stderr, "Error writing findings file: %v\n", err)
			}
		}

		if c.format == "json" && data != nil {
			fmt.Println(string(data))
		}
	}

	// Step 1: Load target paths and repository input context.
	inputPaths, err := LoadTargets(c.fileList, c.fuchsiaDir, f.Args())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		outputFindings()
		return subcommands.ExitUsageError
	}

	inputCtx, err := LoadInputContext(c.fuchsiaDir, inputPaths[0])
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		outputFindings()
		return subcommands.ExitFailure
	}

	// Step 2: Initialize pipeline stages for target evaluation.
	classifier, err := classify.NewClassifier(inputCtx.Config.Classify)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to initialize classifier: %v\n", err)
		outputFindings()
		return subcommands.ExitFailure
	}

	boundaryCfg := inputCtx.Config.Boundary
	boundaryCfg.FilesInReadmeOnly = false
	validator := validate.NewValidator(inputCtx.FuchsiaDir, inputCtx.Config.Validate)

	// Step 3: Group input targets by project root.
	// We map each input target path to its enclosing project root so that multiple files belonging to
	// the same project are verified together against their governing README in a single pipeline run.
	projectTargets := make(map[string][]string)

	for _, inputPath := range inputPaths {
		absInput := inputPath
		if strings.HasPrefix(absInput, "//") {
			absInput = filepath.Join(inputCtx.FuchsiaDir, strings.TrimPrefix(absInput, "//"))
		} else if !filepath.IsAbs(absInput) {
			absInput = filepath.Join(inputCtx.FuchsiaDir, absInput)
		}
		if inputCtx.Config.IsSkipped(absInput) {
			continue
		}

		projectRoot, err := inputCtx.ResolveProjectRoot(absInput)
		if err != nil {
			if c.format != "json" {
				fmt.Fprintf(os.Stderr, "❌ Error: %v\n", err)
			}
			relInput := absInput
			if r, err := filepath.Rel(inputCtx.FuchsiaDir, absInput); err == nil && !strings.HasPrefix(r, "..") {
				relInput = filepath.ToSlash(r)
			}
			allFindings = append(allFindings, report.Finding{
				FilePath:  relInput,
				Level:     "error",
				CheckName: CheckResolveProjectRootError,
				Message:   err.Error(),
			})
			hasErrors = true
			continue
		}
		projectTargets[projectRoot] = append(projectTargets[projectRoot], absInput)
	}

	// Sort project roots to ensure deterministic execution ordering.
	var projectRoots []string
	for root := range projectTargets {
		projectRoots = append(projectRoots, root)
	}
	sort.Strings(projectRoots)

	if len(projectRoots) == 0 {
		outputFindings()
		if hasErrors {
			return subcommands.ExitFailure
		}
		return subcommands.ExitSuccess
	}

	var allTargets []string
	// For first-party code, the project root is the entire Fuchsia repository (FuchsiaDir).
	// Scope the crawler's roots to the specific directories containing the targeted files
	// (or project roots for third-party code) to avoid walking the entire repository.
	var crawlRoots []string
	seenRoots := make(map[string]bool)
	for _, projectRoot := range projectRoots {
		targets := projectTargets[projectRoot]
		allTargets = append(allTargets, targets...)
		if projectRoot == inputCtx.FuchsiaDir {
			for _, t := range targets {
				dir := t
				if info, err := os.Stat(t); err != nil || !info.IsDir() {
					dir = filepath.Dir(t)
				}
				if !seenRoots[dir] {
					seenRoots[dir] = true
					crawlRoots = append(crawlRoots, dir)
				}
			}
		} else {
			if !seenRoots[projectRoot] {
				seenRoots[projectRoot] = true
				crawlRoots = append(crawlRoots, projectRoot)
			}
		}
	}
	sort.Strings(crawlRoots)

	disc := discover.NewCrawler(inputCtx.FuchsiaDir, inputCtx.Config.Discover)
	grouper := boundary.NewGrouper(inputCtx.FuchsiaDir, boundaryCfg)
	pruner := prune.NewPruner(nil)
	pruner.FilesInReadmeOnly = c.fast
	pruner.TargetFiles = allTargets
	pruner.FuchsiaDir = inputCtx.FuchsiaDir

	verifier := report.NewTargetComplianceVerifier(inputCtx.FuchsiaDir, inputCtx.Config, allTargets...)
	verifier.PreserveExisting = c.fast

	targetProjects := make(map[string]*pipeline.Project)
	projectCollector := pipeline.RenderFunc(func(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
		for _, p := range projects {
			targetProjects[p.RootPath] = p
		}
		return nil
	})

	renderers := pipeline.MultiRenderer{verifier, projectCollector}
	orchestrator := pipeline.NewOrchestrator(disc, grouper, pruner, classifier, validator, renderers)
	runErr := orchestrator.Run(ctx, crawlRoots)

	if len(verifier.Findings) > 0 {
		allFindings = append(allFindings, verifier.Findings...)
		hasErrors = true
		if c.format != "json" {
			for _, finding := range verifier.Findings {
				fmt.Fprintf(os.Stderr, "❌ Error: %s\n", finding.Message)
			}
		}
	}
	if runErr != nil && (len(verifier.Findings) == 0 || runErr.Error() != verifier.Findings[0].Message) {
		hasErrors = true
		if c.format != "json" {
			fmt.Fprintf(os.Stderr, "❌ Pipeline execution error: %v\n", runErr)
		}
		allFindings = append(allFindings, report.Finding{
			Level:     "error",
			CheckName: CheckPipelineExecutionError,
			Message:   runErr.Error(),
		})
	}

	// Step 4: Output pass confirmation for compliant targets in text mode.
	if c.format != "json" && (runErr == nil || len(verifier.Findings) > 0) {
		for _, projectRoot := range projectRoots {
			targets := projectTargets[projectRoot]
			relProjectRoot := projectRoot
			if r, err := filepath.Rel(inputCtx.FuchsiaDir, projectRoot); err == nil && !strings.HasPrefix(r, "..") {
				relProjectRoot = filepath.ToSlash(r)
			}

			// If any finding belongs to a third-party project, the entire project fails.
			if projectRoot != inputCtx.FuchsiaDir {
				hasProjectError := false
				for _, finding := range allFindings {
					if finding.FilePath == relProjectRoot || strings.HasPrefix(finding.FilePath, relProjectRoot+"/") {
						hasProjectError = true
						break
					}
				}
				if hasProjectError {
					continue
				}
			}

			// For first-party targets, check if any finding matches or is within the target.
			hasTargetError := false
			for _, t := range targets {
				relT := t
				if r, err := filepath.Rel(inputCtx.FuchsiaDir, t); err == nil && !strings.HasPrefix(r, "..") {
					relT = filepath.ToSlash(r)
				}
				for _, finding := range allFindings {
					if finding.FilePath == relT || strings.HasPrefix(finding.FilePath, relT+"/") {
						hasTargetError = true
						break
					}
				}
				if hasTargetError {
					break
				}
			}
			if hasTargetError {
				continue
			}

			targetProj := targetProjects[projectRoot]
			projectName := ""
			if targetProj != nil && targetProj.Readme != nil {
				origs := targetProj.Readme.OriginalSegments()
				if len(origs) > 0 && origs[0].Name != "" {
					projectName = origs[0].Name
				}
			}
			if projectName == "" {
				projectName = findProjectBasename(inputCtx.FuchsiaDir, projectRoot, inputCtx.Config)
			}

			if len(targets) == 1 {
				target := targets[0]
				relTarget := target
				if r, err := filepath.Rel(inputCtx.FuchsiaDir, target); err == nil && !strings.HasPrefix(r, "..") {
					relTarget = filepath.ToSlash(r)
				}
				if relTarget == "" || relTarget == "." {
					fmt.Printf("✅ Passed: %s\n", projectName)
				} else {
					fmt.Printf("✅ Passed: %s (%s)\n", projectName, relTarget)
				}
			} else {
				fmt.Printf("✅ Passed: %s (%d files checked)\n", projectName, len(targets))
			}
		}
	}

	outputFindings()

	// Step 5: Return overall success or failure status.
	if hasErrors {
		return subcommands.ExitFailure
	}
	return subcommands.ExitSuccess
}
