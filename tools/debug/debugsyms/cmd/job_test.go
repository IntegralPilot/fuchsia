// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"fmt"
	"os"
	"path"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/debug/elflib"
)

func TestJob(t *testing.T) {
	buildID := "foo"
	filename := fmt.Sprintf("%s.debug", buildID)
	tmpFile, err := os.CreateTemp("", filename)
	if err != nil {
		t.Fatalf("Failed to create tempfile: %v", err)
	}
	defer os.Remove(tmpFile.Name())
	binaryFileRef := elflib.NewBinaryFileRef(tmpFile.Name(), buildID)
	ctx := context.Background()

	t.Run("debuginfod job", func(t *testing.T) {
		job := newDebuginfodJob(binaryFileRef)
		expectedPath := path.Join(debuginfodNamespace, buildID, "debuginfo")
		expectedName := fmt.Sprintf("ensure %q in %s", buildID, expectedPath)
		if job.name != expectedName {
			t.Fatalf("incorrect job name; expected %q, got %q", job.name, expectedName)
		}
		if job.path != expectedPath {
			t.Fatalf("incorrect GCS path; expected %q, got %q", expectedPath, job.path)
		}
	})
	t.Run("zxdb job", func(t *testing.T) {
		job := newZxdbJob(binaryFileRef)
		expectedPath := path.Join(zxdbNamespace, filename)
		expectedName := fmt.Sprintf("ensure %q in %s", buildID, expectedPath)
		if job.name != expectedName {
			t.Fatalf("incorrect job name; expected %q, got %q", job.name, expectedName)
		}
		if job.path != expectedPath {
			t.Fatalf("incorrect GCS path; expected %q, got %q", expectedPath, job.path)
		}
	})

	job := newZxdbJob(binaryFileRef)

	t.Run("ensure on nonexistent object", func(t *testing.T) {
		bkt := &mockBucket{contents: map[string]bool{"other.debug": true}}
		if err := job.ensure(ctx, bkt); err != nil {
			t.Fatalf("expected nil error, got %v", err)
		}
	})
	t.Run("ensure on existing object", func(t *testing.T) {
		bkt := &mockBucket{contents: map[string]bool{filename: true}}
		if err := job.ensure(ctx, bkt); err != nil {
			t.Fatalf("expected nil error, got %v", err)
		}
	})
	t.Run("ensure with upload error", func(t *testing.T) {
		bkt := &mockBucket{
			contents:  map[string]bool{"other.debug": true},
			uploadErr: fmt.Errorf("error during upload"),
		}
		if err := job.ensure(ctx, bkt); err == nil {
			t.Fatalf("expected error, got nil")
		}
	})
	t.Run("ensure with unknown object state error", func(t *testing.T) {
		bkt := &mockBucket{
			contents:        map[string]bool{"other.debug": true},
			objectExistsErr: fmt.Errorf("unknown object state"),
		}
		if err := job.ensure(ctx, bkt); err == nil {
			t.Fatalf("expected error, got %v", err)
		}
	})
}
