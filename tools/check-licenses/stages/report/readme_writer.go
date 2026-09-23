// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"fmt"
	"os"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/readme"
)

// ReadmeWriter updates each project's README.fuchsia in-place with its found licenses
// and writes the updated contents to disk (or stdout).
type ReadmeWriter struct {
	FuchsiaDir       string
	PrintStdout      bool
	PreserveExisting bool
}

// NewReadmeWriter creates a new ReadmeWriter.
func NewReadmeWriter(fuchsiaDir string, printStdout bool) *ReadmeWriter {
	return &ReadmeWriter{
		FuchsiaDir:  fuchsiaDir,
		PrintStdout: printStdout,
	}
}

func (w *ReadmeWriter) Run(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
	for _, proj := range projects {
		if proj.Readme == nil || len(proj.Readme.Segments) == 0 {
			continue
		}

		if proj.IsFirstParty() {
			if !w.PrintStdout {
				fmt.Printf("ℹ️  Skipping 1st-party project %s (governed by virtual %s)\n", proj.RootPath, proj.Readme.Path)
			}
			continue
		}

		readme.UpdateWithClassifiedFiles(w.FuchsiaDir, proj.RootPath, proj.Readme.UpdatedSegments(), proj.FoundLicenses(), w.PreserveExisting)
		formatted := readme.Format(proj.Readme.UpdatedSegments())

		if w.PrintStdout {
			fmt.Print(formatted)
		} else {
			if err := os.WriteFile(proj.Readme.Path, []byte(formatted), 0644); err != nil {
				return fmt.Errorf("failed to write %s: %w", proj.Readme.Path, err)
			}
			fmt.Printf("✏️  Successfully updated %s\n", proj.Readme.Path)
		}
	}
	return nil
}
