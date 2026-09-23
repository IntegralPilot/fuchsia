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

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/config"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/readme"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
)

type ProjectInfoCommand struct {
	fuchsiaDir string
}

func (*ProjectInfoCommand) Name() string { return "info" }
func (*ProjectInfoCommand) Synopsis() string {
	return "Shows the project metadata and compliance state for a given file or directory."
}
func (*ProjectInfoCommand) Usage() string {
	return `info <path>:
  Shows the project metadata and compliance state for a given file or directory.
`
}

func (c *ProjectInfoCommand) SetFlags(f *flag.FlagSet) {}

func (c *ProjectInfoCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	// Step 1: Validate argument count and load repository input context.
	if f.NArg() != 1 {
		fmt.Fprintln(os.Stderr, "Error: exactly one path must be provided.")
		return subcommands.ExitUsageError
	}

	inputPath := f.Arg(0)
	inputCtx, err := LoadInputContext(c.fuchsiaDir, inputPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	// Step 2: Locate the governing README.fuchsia for the input path.
	grouper := boundary.NewGrouper(inputCtx.FuchsiaDir, inputCtx.Config.Boundary)
	r, readmePath, err := grouper.FindProjectReadme(inputCtx.AbsPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to find project README: %v\n", err)
		return subcommands.ExitFailure
	}
	if r == nil {
		fmt.Fprintf(os.Stderr, "No project README found for %s\n", inputPath)
		return subcommands.ExitFailure
	}

	// Step 3: Determine the project root and print project header information.
	logicalRoot := readme.ResolveProjectRoot(r, readmePath, inputCtx.FuchsiaDir, inputCtx.Config.Boundary.OutOfTreeReadmes)
	isVirtual := false
	for _, physPath := range inputCtx.Config.Boundary.OutOfTreeReadmes {
		if physPath == readmePath {
			isVirtual = true
			break
		}
	}

	relRoot, _ := filepath.Rel(inputCtx.FuchsiaDir, logicalRoot)
	fmt.Printf("Project Root: //%s\n", relRoot)

	virtualStr := ""
	if isVirtual {
		virtualStr = " (Virtual)"
	}
	relReadme, _ := filepath.Rel(inputCtx.FuchsiaDir, readmePath)
	fmt.Printf("Readme Path:  %s%s\n", relReadme, virtualStr)

	// Step 4: Collect active policy exceptions that apply to this project.
	activePolicies := make(map[string]config.RuleMetadata)
	var activePolicyNames []string
	for policyName, paths := range inputCtx.Config.Validate.PolicyExceptions {
		for p, meta := range paths {
			cleanP := strings.TrimPrefix(p, "//")
			if grouper.BelongsToProject(cleanP, relRoot) {
				activePolicies[policyName] = meta
				activePolicyNames = append(activePolicyNames, policyName)
				break
			}
		}
	}
	sort.Strings(activePolicyNames)

	// Step 5: Collect allowed license exceptions that apply to this project.
	allowedLicenses := make(map[string]config.RuleMetadata)
	var allowedLicenseNames []string
	for licenseID, paths := range inputCtx.Config.Validate.AllowedLicenses {
		for p, meta := range paths {
			cleanP := strings.TrimPrefix(p, "//")
			if grouper.BelongsToProject(cleanP, relRoot) {
				allowedLicenses[licenseID] = meta
				allowedLicenseNames = append(allowedLicenseNames, licenseID)
				break
			}
		}
	}
	sort.Strings(allowedLicenseNames)

	// Step 6: Print policy overrides and allowed license details.
	if len(activePolicyNames) > 0 {
		fmt.Println("\nPolicy Overrides:")
		for _, name := range activePolicyNames {
			meta := activePolicies[name]
			fmt.Printf("  - %s\n", name)
			if meta.ConfigPath != "" {
				relConfig, _ := filepath.Rel(inputCtx.FuchsiaDir, meta.ConfigPath)
				fmt.Printf("      Config: %s\n", relConfig)
			}
			if meta.Bug != "" {
				fmt.Printf("      Bug: %s\n", meta.Bug)
			}
			if meta.Description != "" {
				fmt.Printf("      Description: %s\n", meta.Description)
			}
		}
	}

	if len(allowedLicenseNames) > 0 {
		fmt.Println("\nAllowed Licenses:")
		for _, name := range allowedLicenseNames {
			meta := allowedLicenses[name]
			fmt.Printf("  - %s\n", name)
			if meta.ConfigPath != "" {
				relConfig, _ := filepath.Rel(inputCtx.FuchsiaDir, meta.ConfigPath)
				fmt.Printf("      Config: %s\n", relConfig)
			}
			if meta.Bug != "" {
				fmt.Printf("      Bug: %s\n", meta.Bug)
			}
			if meta.Description != "" {
				fmt.Printf("      Description: %s\n", meta.Description)
			}
		}
	}
	fmt.Println()

	// Step 7: Print the parsed README.fuchsia content.
	fmt.Println("--- Parsed README.fuchsia Content ---")
	fmt.Println(readme.Format([]*readme.Readme{r}))

	return subcommands.ExitSuccess
}
