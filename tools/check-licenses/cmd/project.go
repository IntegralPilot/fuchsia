// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"os"

	"github.com/google/subcommands"
)

type ProjectCommand struct {
	fuchsiaDir string
}

func (*ProjectCommand) Name() string     { return "project" }
func (*ProjectCommand) Synopsis() string { return "Check or update project metadata and compliance." }
func (*ProjectCommand) Usage() string {
	return `project <subcommand> [options] <args...>:
  Manage project metadata and compliance.

  Subcommands:
    check <files...>   Analyzes specific files and validates them against their parent README.fuchsia.
    info <path>        Shows the project metadata and compliance state for a given file or directory.
    list <dir>         Lists all project boundaries discovered under the directory.
    update <dir>       Automatically updates the License File declarations in a README.fuchsia.
`
}

func (c *ProjectCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&c.fuchsiaDir, "fuchsia_dir", os.Getenv("FUCHSIA_DIR"), "Location of the fuchsia root directory (//).")
}

func (c *ProjectCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	subFlags := flag.NewFlagSet("project", flag.ContinueOnError)
	if err := subFlags.Parse(f.Args()); err != nil {
		return subcommands.ExitUsageError
	}
	subCommander := subcommands.NewCommander(subFlags, "project")
	subCommander.Register(&ProjectCheckCommand{fuchsiaDir: c.fuchsiaDir}, "")
	subCommander.Register(&ProjectInfoCommand{fuchsiaDir: c.fuchsiaDir}, "")
	subCommander.Register(&ProjectListCommand{fuchsiaDir: c.fuchsiaDir}, "")
	subCommander.Register(&ProjectUpdateCommand{fuchsiaDir: c.fuchsiaDir}, "")
	return subCommander.Execute(ctx)
}
