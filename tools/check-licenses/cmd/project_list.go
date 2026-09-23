// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"sort"

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/readme"
)

type ProjectListCommand struct {
	fuchsiaDir string
}

func (*ProjectListCommand) Name() string { return "list" }
func (*ProjectListCommand) Synopsis() string {
	return "Lists all project boundaries discovered under the directory."
}
func (*ProjectListCommand) Usage() string {
	return `list <dir>:
  Lists all project boundaries discovered under the given directory.
`
}

func (c *ProjectListCommand) SetFlags(f *flag.FlagSet) {}

func (c *ProjectListCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	// Step 1: Resolve target directory and load repository input context.
	dir := c.fuchsiaDir
	if f.NArg() > 0 {
		dir = f.Arg(0)
	}
	inputCtx, err := LoadInputContext(c.fuchsiaDir, dir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}

	// Step 2: Discover all project boundaries under the target directory.
	discoveredProjects, err := readme.DiscoverProjects(inputCtx.AbsPath, inputCtx.FuchsiaDir, inputCtx.Config)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to discover projects: %v\n", err)
		return subcommands.ExitFailure
	}

	// Step 3: Deduplicate discovered projects by path
	seen := make(map[string]bool)
	var projects []readme.ProjectInfo
	for _, p := range discoveredProjects {
		if !seen[p.Path] {
			seen[p.Path] = true
			projects = append(projects, p)
		}
	}

	// Step 4: Sort all discovered projects by path for deterministic output.
	sort.Slice(projects, func(i, j int) bool {
		return projects[i].Path < projects[j].Path
	})

	// Step 5: Output formatted project boundaries to stdout.
	for _, p := range projects {
		fmt.Printf("//%s: %s\n", p.Path, p.Name)
	}

	return subcommands.ExitSuccess
}
