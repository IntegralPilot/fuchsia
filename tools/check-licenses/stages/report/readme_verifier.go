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
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
)

// ReadmeVerifier verifies that on-disk README.fuchsia files accurately reflect the classified licenses.
type ReadmeVerifier struct {
	FuchsiaDir string
}

// NewReadmeVerifier creates a new ReadmeVerifier.
func NewReadmeVerifier(fuchsiaDir string) *ReadmeVerifier {
	return &ReadmeVerifier{
		FuchsiaDir: fuchsiaDir,
	}
}

func (v *ReadmeVerifier) Run(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
	var diffErrors []pipeline.ComplianceError

	for _, proj := range projects {
		if proj.Readme == nil || len(proj.Readme.Segments) == 0 {
			continue
		}

		readme.UpdateWithClassifiedFiles(v.FuchsiaDir, proj.RootPath, proj.Readme.UpdatedSegments(), proj.FoundLicenses(), false)
		newFormatted := readme.Format(proj.Readme.UpdatedSegments())

		rawBytes, err := os.ReadFile(proj.Readme.Path)
		if err != nil {
			continue
		}

		if string(rawBytes) != newFormatted {
			diffErrors = append(diffErrors, pipeline.ComplianceError{
				CheckName: validate.CheckReadmeNeedsUpdate,
				Project:   proj.RootPath,
				FilePath:  proj.Readme.Path,
				Issue:     "README.fuchsia is out of date. Run 'fx check-licenses fix' or 'fx check-licenses update' to update it.",
			})
		}
	}

	if len(diffErrors) > 0 {
		return fmt.Errorf("found %d out of date README.fuchsia file(s)", len(diffErrors))
	}

	return nil
}
