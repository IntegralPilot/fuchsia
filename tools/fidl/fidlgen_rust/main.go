// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"flag"
	"fmt"
	"log"
	"os"
	"path"

	"go.fuchsia.dev/fuchsia/tools/fidl/fidlgen_rust/codegen"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type flagsDef struct {
	jsonPath           *string
	outputFilenamePath *string
	rustfmtPath        *string
	rustfmtConfigPath  *string
	fdomain            *bool
	common             *bool
	use_common         *string
	includeDrivers     *bool
	apiCoverage        *bool
}

var flags = flagsDef{
	jsonPath: flag.String("json", "",
		"relative path to the FIDL intermediate representation."),
	outputFilenamePath: flag.String("output-filename", "",
		"the output path for the generated Rust implementation."),
	rustfmtPath: flag.String("rustfmt", "",
		"path to the rustfmt tool."),
	rustfmtConfigPath: flag.String("rustfmt-config", "",
		"path to rustfmt.toml."),
	fdomain:        flag.Bool("fdomain", false, "if given, generate FDomain bindings."),
	common:         flag.Bool("common", false, "if given, generate only the common (non-resource) data structures."),
	use_common:     flag.String("use_common", "", "if given, depend on the given crate name for the common (non-resource) data structures."),
	includeDrivers: flag.Bool("include-drivers", false, "whether to include driver transport protocols or not"),
	apiCoverage:    flag.Bool("api-coverage", false, "whether to generate API coverage annotations"),
}

// valid returns true if the parsed flags are valid.
func (f flagsDef) valid() bool {
	return *f.jsonPath != "" && *f.outputFilenamePath != "" && (!*f.common || (!*f.fdomain && *f.use_common == ""))
}

func printUsage() {
	program := path.Base(os.Args[0])
	message := `Usage: ` + program + ` [flags]

Rust FIDL backend, used to generate Rust bindings from JSON IR input (the
intermediate representation of a FIDL library).

Flags:
`
	fmt.Fprint(flag.CommandLine.Output(), message)
	flag.PrintDefaults()
}

func main() {
	flag.Usage = printUsage
	flag.Parse()
	if !flags.valid() {
		printUsage()
		os.Exit(1)
	}

	root, err := fidlgen.ReadJSONIr(*flags.jsonPath)
	if err != nil {
		log.Fatal(err)
	}

	generator := codegen.NewGenerator(*flags.rustfmtPath, *flags.rustfmtConfigPath, *flags.fdomain, *flags.common, *flags.use_common, *flags.apiCoverage)
	err = generator.GenerateFidl(
		root, *flags.outputFilenamePath, *flags.includeDrivers)
	if err != nil {
		log.Fatalf("Error running generator: %v", err)
	}
}
