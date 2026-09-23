// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

func TestConsoleErrorReporter_NoErrors(t *testing.T) {
	r := NewConsoleErrorReporter(t.TempDir())
	if err := r.Run(context.Background(), nil, nil); err != nil {
		t.Errorf("Expected nil error when errors slice is empty, got %v", err)
	}
}

func TestConsoleErrorReporter_WithErrors(t *testing.T) {
	r := NewConsoleErrorReporter(t.TempDir())
	errors := []pipeline.ComplianceError{
		{
			CheckName: "UnrecognizedLicense",
			Project:   "third_party/foo",
			FilePath:  "third_party/foo/LICENSE",
			Issue:     "Unrecognized license header",
		},
	}

	if err := r.Run(context.Background(), nil, errors); err == nil {
		t.Errorf("Expected error returned when errors slice is non-empty, got nil")
	}
}
