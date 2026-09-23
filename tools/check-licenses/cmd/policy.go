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

	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
)

type PolicyCommand struct {
	fuchsiaDir string
}

func (*PolicyCommand) Name() string     { return "policy" }
func (*PolicyCommand) Synopsis() string { return "Manage policy exceptions." }
func (*PolicyCommand) Usage() string {
	return `policy SUBCOMMAND [options]:
  Manage policy exceptions.

  Subcommands:
    add   Add a new policy exception.
`
}

func (p *PolicyCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&p.fuchsiaDir, "fuchsia_dir", os.Getenv("FUCHSIA_DIR"), "Location of the fuchsia root directory.")
}

func (p *PolicyCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	subFlags := flag.NewFlagSet("policy", flag.ContinueOnError)
	if err := subFlags.Parse(f.Args()); err != nil {
		return subcommands.ExitUsageError
	}
	subCommander := subcommands.NewCommander(subFlags, "policy")
	subCommander.Register(&PolicyAddCommand{fuchsiaDir: p.fuchsiaDir}, "")
	return subCommander.Execute(ctx)
}

type PolicyAddCommand struct {
	fuchsiaDir  string
	bug         string
	description string
}

func (*PolicyAddCommand) Name() string     { return "add" }
func (*PolicyAddCommand) Synopsis() string { return "Add a policy exception." }
func (*PolicyAddCommand) Usage() string {
	return `add -bug BUG_ID [-desc DESCRIPTION] CHECK_NAME TARGET_PATH:
  Adds a policy exception for the given project or file path.

  Flags:
    -bug  Bug ID tracking this exception (Mandatory).
    -desc Optional description for this exception.

  Examples:
    fx check-licenses policy add -bug b/123 AllProjectsMustHaveALicense vendor/foo
    fx check-licenses policy add -bug b/456 -desc "Custom exception" AllLicenseTextsMustBeRecognized third_party/bar/LICENSE
`
}

func (p *PolicyAddCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&p.bug, "bug", "", "Bug ID tracking this exception (Mandatory).")
	f.StringVar(&p.description, "desc", "Auto-generated exception", "Optional description for this exception.")
}

func (p *PolicyAddCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	cmdStr, misplacedFlags := ReconstructCommand("policy add", f.Args(), []string{"CHECK_NAME", "TARGET_PATH"}, p.bug, p.description)
	if misplacedFlags || f.NArg() != 2 {
		if misplacedFlags {
			fmt.Fprintln(os.Stderr, "❌ Error: Flags (like -bug or -desc) must be placed BEFORE positional arguments.")
		} else {
			fmt.Fprintln(os.Stderr, "❌ Error: Invalid number of arguments.")
		}
		fmt.Fprintf(os.Stderr, "Try running this copy-pasteable command instead:\n    %s\n\n", cmdStr)
		return subcommands.ExitUsageError
	}

	if p.bug == "" {
		fmt.Fprintln(os.Stderr, "Error: the -bug flag is mandatory.")
		return subcommands.ExitUsageError
	}

	checkName := f.Arg(0)
	if !validate.IsValidPolicy(checkName) {
		validChecks := validate.ValidPolicies()
		sort.Strings(validChecks)
		fmt.Fprintf(os.Stderr, "Error: invalid check name %q. Must be one of: %s\n", checkName, strings.Join(validChecks, ", "))
		return subcommands.ExitUsageError
	}
	targetPath := filepath.Clean(f.Arg(1))

	if _, err := AddPolicyException(p.fuchsiaDir, checkName, targetPath, p.bug, p.description); err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	return subcommands.ExitSuccess
}

// AddPolicyException adds a policy exception for a given project or file path.
func AddPolicyException(fuchsiaDir, checkName, targetPath, bug, description string) (string, error) {
	ic, err := LoadInputContext(fuchsiaDir, targetPath)
	if err != nil {
		return "", fmt.Errorf("failed to load input context: %w", err)
	}

	resolvedTarget := targetPath
	if validate.IsProjectScopePolicy(checkName) {
		if projectRoot, err := ic.ResolveProjectRoot(targetPath); err == nil {
			resolvedTarget = projectRoot
		}
	}

	relTarget, err := filepath.Rel(ic.FuchsiaDir, resolvedTarget)
	if err != nil {
		relTarget = resolvedTarget
	}
	relTarget = filepath.ToSlash(filepath.Clean(relTarget))

	// Check if this target already has an exception
	if list, ok := ic.Config.Validate.PolicyExceptions[checkName]; ok {
		if _, exists := list[relTarget]; exists {
			fmt.Printf("Path '%s' already has a policy exception for '%s'. Nothing to do.\n", relTarget, checkName)
			return "", nil
		}
	}

	destFile, err := ic.Config.AddPolicyException(checkName, relTarget, bug, description)
	if err != nil {
		return "", err
	}

	fmt.Printf("✅ Added Policy Exception:\n")
	fmt.Printf("  - Check:  %s\n", checkName)
	fmt.Printf("  - Target: %s\n", relTarget)
	fmt.Printf("  - Bug:    %s\n", bug)
	fmt.Printf("  - File:   %s\n\n", destFile)

	return destFile, nil
}
