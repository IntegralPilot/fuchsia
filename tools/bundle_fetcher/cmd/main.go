// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"os"

	"go.fuchsia.dev/fuchsia/tools/lib/color"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"

	"github.com/google/subcommands"
)

var (
	colors         color.EnableColor
	level          logger.LogLevel
	subcommandList []subcommands.Command
)

func init() {
	colors = color.ColorAuto
	level = logger.DebugLevel

	flag.Var(&colors, "color", "use color in output, can be never, auto, always")
	flag.Var(&level, "level", "output verbosity, can be fatal, error, warning, info, debug or trace")

	subcommandList = append(subcommandList,
		subcommands.HelpCommand(),
		subcommands.FlagsCommand(),
		&productListCmd{},
	)
}

func main() {
	for _, cmd := range subcommandList {
		subcommands.Register(cmd, "")
	}

	flag.Parse()
	log := logger.NewLogger(level, color.NewColor(colors), os.Stdout, os.Stderr, "bundles ")
	ctx := logger.WithLogger(context.Background(), log)
	os.Exit(int(subcommands.Execute(ctx)))
}
