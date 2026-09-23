// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"
)

type stringsFlag []string

func (s *stringsFlag) String() string {
	return strings.Join(*s, ", ")
}

func (s *stringsFlag) Set(value string) error {
	*s = append(*s, value)
	return nil
}

var (
	keyArgs     stringsFlag
	inputFiles  stringsFlag
	outFile     string
	sourcesFile string
)

func init() {
	flag.Var(&keyArgs, "key", "a label for output")
	flag.Var(&inputFiles, "in", "path to an input file")
	flag.StringVar(&outFile, "out", "", "path to output file (default stdout)")
}

func main() {
	flag.Parse()
	if flag.NArg() != 0 {
		flag.PrintDefaults()
	}

	ctx := context.Background()
	argMap := NewArgMap()

	args, errs := ParseGNArgs(ctx, inputFiles, keyArgs)
	argMap.AddArgs(args)
	if err := <-errs; err != nil {
		log.Fatalf("Error: %s\n", err)
	}

	var out io.Writer
	if outFile != "" {
		var err error
		if dirErr := os.MkdirAll(filepath.Dir(outFile), os.ModePerm); dirErr != nil {
			log.Fatalf("Error creating directories: %s", dirErr)
		}
		out, err = os.Create(outFile)
		if err != nil {
			log.Fatalf("Error opening file: %s", err)
		}
	} else {
		out = os.Stdout
	}

	argMap.EmitMarkdown(out)
}
