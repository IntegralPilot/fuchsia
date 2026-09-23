// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"flag"
	"fmt"
	"log"
	"os"
	"path"

	"go.fuchsia.dev/fuchsia/tools/fidl/fidlgen_python/codegen"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

type flagsDef struct {
	jsonPath           *string
	outputFilenamePath *string
	includeDrivers     *bool
	blackPath          *string
}

var (
	fidlIrJson = flag.String("fidl-ir-json", "REQUIRED",
		"the FIDL intermediate representation file")
	outputFile = flag.String("output", "REQUIRED",
		"the output file for the generated Python implementation")
	blackPath = flag.String("black", "REQUIRED",
		"path to black formatter")
	pyprojectTomlPath = flag.String("pyproject-toml", "REQUIRED",
		"path to pyproject.toml for black configuration")
)

func printUsage() {
	program := path.Base(os.Args[0])
	message := `Usage: ` + program + ` [flags]

Python FIDL backend, used to generate Python bindings from JSON IR input (the
intermediate representation of a FIDL library).

Flags:
`
	fmt.Fprint(flag.CommandLine.Output(), message)
	flag.PrintDefaults()
}

func main() {
	log.SetFlags(log.LstdFlags | log.Llongfile)

	flag.Usage = printUsage
	flag.Parse()
	if *fidlIrJson == "REQUIRED" {
		log.Fatal("Error: --fidl-ir is required")
	}
	fidlIrJson := *fidlIrJson
	if *outputFile == "REQUIRED" {
		log.Fatal("Error: --output is required")
	}
	outputFile := *outputFile
	if *blackPath == "REQUIRED" {
		log.Fatal("Error: --black is required")
	}
	blackPath := *blackPath
	if *pyprojectTomlPath == "REQUIRED" {
		log.Fatal("Error: --pyprojectTomlPath is required")
	}
	pyprojectTomlPath := *pyprojectTomlPath

	root, err := fidlgen.ReadJSONIr(fidlIrJson)
	if err != nil {
		log.Fatal(err)
	}

	generator := codegen.NewGenerator(blackPath, pyprojectTomlPath)
	err = generator.GenerateFidl(root, outputFile)
	if err != nil {
		log.Fatalf("Error running generator: %v", err)
	}
}
