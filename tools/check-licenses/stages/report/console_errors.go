// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

// ConsoleErrorReporter formats and prints compliance errors to stderr.
type ConsoleErrorReporter struct {
	FuchsiaDir string
}

// NewConsoleErrorReporter creates a new ConsoleErrorReporter.
func NewConsoleErrorReporter(fuchsiaDir string) *ConsoleErrorReporter {
	return &ConsoleErrorReporter{
		FuchsiaDir: fuchsiaDir,
	}
}

func (r *ConsoleErrorReporter) Run(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
	if len(errors) == 0 {
		return nil
	}

	sort.Slice(errors, func(i, j int) bool {
		if errors[i].CheckName != errors[j].CheckName {
			return errors[i].CheckName < errors[j].CheckName
		}
		if errors[i].Issue != errors[j].Issue {
			return errors[i].Issue < errors[j].Issue
		}
		if errors[i].Project != errors[j].Project {
			return errors[i].Project < errors[j].Project
		}
		return errors[i].FilePath < errors[j].FilePath
	})

	var b strings.Builder
	b.WriteString(fmt.Sprintf("Pipeline failed with %d compliance error(s):\n", len(errors)))

	lastCheckAndIssue := ""
	for _, e := range errors {
		checkAndIssue := e.Issue
		if e.CheckName != "" {
			checkAndIssue = fmt.Sprintf("[%s] %s", e.CheckName, e.Issue)
		}

		if checkAndIssue != lastCheckAndIssue {
			b.WriteString("\n" + checkAndIssue + "\n")
			lastCheckAndIssue = checkAndIssue
		}

		relProj, err := filepath.Rel(r.FuchsiaDir, e.Project)
		if err != nil || relProj == "." {
			relProj = e.Project
		}

		relFile := ""
		if e.FilePath != "" {
			relFile, err = filepath.Rel(r.FuchsiaDir, e.FilePath)
			if err != nil {
				relFile = e.FilePath
			}
			b.WriteString(fmt.Sprintf("- %s (%s)\n", relProj, relFile))
		} else {
			b.WriteString(fmt.Sprintf("- %s\n", relProj))
		}
	}

	fmt.Fprintln(os.Stderr, b.String())
	return fmt.Errorf("compliance validation failed with %d error(s)", len(errors))
}
