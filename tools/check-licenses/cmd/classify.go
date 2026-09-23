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

	"github.com/google/subcommands"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/classify"
)

type ClassifyCommand struct {
	fuchsiaDir string
}

func (*ClassifyCommand) Name() string { return "classify" }
func (*ClassifyCommand) Synopsis() string {
	return "Runs the Google License Classifier on a specific file."
}
func (*ClassifyCommand) Usage() string {
	return `classify <file_path>:
  Runs the underlying license classifier engine on the provided file and prints
  any detected license patterns, their SPDX IDs, and line numbers to stdout.
`
}

func (c *ClassifyCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&c.fuchsiaDir, "fuchsia_dir", os.Getenv("FUCHSIA_DIR"), "Location of the fuchsia root directory (//).")
}

func (c *ClassifyCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...interface{}) subcommands.ExitStatus {
	if f.NArg() != 1 {
		fmt.Fprintln(os.Stderr, "Error: exactly one file path must be provided.")
		return subcommands.ExitUsageError
	}

	targetFile := f.Arg(0)
	fuchsiaDir, targetFile, err := ResolveAndValidatePath(c.fuchsiaDir, targetFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		return subcommands.ExitFailure
	}
	absPath := filepath.Join(fuchsiaDir, targetFile)

	// Initialize classifier
	patternsDir := filepath.Join(fuchsiaDir, "tools", "check-licenses", "assets", "patterns")
	// We don't filter extensions here since the user explicitly asked to classify this file.
	classifier, err := classify.NewClassifier(classify.Config{
		Threshold:   0.8,
		PatternDirs: []string{patternsDir},
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to initialize classifier: %v\n", err)
		return subcommands.ExitFailure
	}

	// We instantiate a dummy filtered project just to run the classifier on this one file
	inChan := make(chan pipeline.FilteredProject, 1)
	inChan <- pipeline.FilteredProject{
		Project: pipeline.Project{
			RootPath: filepath.Dir(absPath),
			Files:    []pipeline.FileInfo{{Path: absPath, LicenseParser: "Single License"}},
		},
	}
	close(inChan)

	outChan, err := classifier.Run(ctx, inChan)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to run classifier: %v\n", err)
		return subcommands.ExitFailure
	}

	var cf pipeline.ClassifiedFile
	for result := range outChan {
		cf = result
	}

	if len(cf.Matches) == 0 {
		fmt.Println("No license patterns found.")
		return subcommands.ExitSuccess
	}

	fmt.Printf("Found %d match(es) in %s:\n\n", len(cf.Matches), targetFile)
	for i, match := range cf.Matches {
		fmt.Printf("--- Match %d ---\n", i+1)
		fmt.Printf("SPDX ID:      %s\n", match.SPDXID)
		fmt.Printf("Pattern Name: %s\n", match.PatternName)
		fmt.Printf("Match Type:   %s\n", match.MatchType)
		fmt.Printf("Lines:        %d-%d\n", match.StartLine, match.EndLine)
	}

	return subcommands.ExitSuccess
}
