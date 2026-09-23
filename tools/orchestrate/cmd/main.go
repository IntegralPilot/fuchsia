// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"os"

	"github.com/google/subcommands"
)

func main() {
	subcommands.Register(&runCmd{}, "")
	subcommands.Register(subcommands.HelpCommand(), "")

	flag.Parse()

	os.Exit(int(subcommands.Execute(context.Background())))
}
