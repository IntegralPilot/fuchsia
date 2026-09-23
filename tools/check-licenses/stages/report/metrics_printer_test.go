// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

func TestSummaryMetricsPrinter_Run(t *testing.T) {
	tempDir := t.TempDir()
	printer := NewSummaryMetricsPrinter(tempDir, time.Now())

	projects := []*pipeline.Project{
		{
			RootPath: tempDir,
			Files: []pipeline.FileInfo{
				{Path: "foo.cc"},
			},
			ClassifiedFiles: []pipeline.ClassifiedFile{
				{Path: "foo.cc", IsLicenseFile: true},
			},
		},
	}

	if err := printer.Run(context.Background(), projects, nil); err != nil {
		t.Errorf("Expected nil error from SummaryMetricsPrinter.Run, got %v", err)
	}
}
