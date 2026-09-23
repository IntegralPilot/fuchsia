// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"fmt"
	"os"
	"path"

	"go.fuchsia.dev/fuchsia/tools/debug/elflib"
)

const (
	// GCS top-level namespace to which symbols are uploaded for zxdb.
	zxdbNamespace = "debug"
	// GCS top-level namespace to which symbols are uploaded for debuginfod.
	debuginfodNamespace = "buildid"
)

// Job is a description of a BinaryFileRef to ensure existence in GCS.
type job struct {
	// The BinaryFileRef to ensure existence in GCS.
	bfr elflib.BinaryFileRef

	// The destination GCS path.
	path string

	// Human-readable display name.
	name string
}

// newZxdbJob returns a job to upload debug symbols to a location compatible
// with the fuchsia-specific zxdb format.
func newZxdbJob(bfr elflib.BinaryFileRef) job {
	path := path.Join(zxdbNamespace, bfr.BuildID+elflib.DebugFileSuffix)
	name := fmt.Sprintf("ensure %q in %s", bfr.BuildID, path)
	return job{bfr: bfr, path: path, name: name}
}

// newDebuginfodJob returns a job to upload debug symbols to a location
// compatible with debuginfod.
func newDebuginfodJob(bfr elflib.BinaryFileRef) job {
	path := path.Join(debuginfodNamespace, bfr.BuildID, "debuginfo")
	name := fmt.Sprintf("ensure %q in %s", bfr.BuildID, path)
	return job{bfr: bfr, path: path, name: name}
}

func (j *job) ensure(ctx context.Context, bkt bucket) error {
	exists, err := bkt.objectExists(ctx, j.path)
	if err != nil {
		return fmt.Errorf("failed to determine object %s existence: %w", j.path, err)
	}
	if exists {
		return nil
	}
	reader, err := os.Open(j.bfr.Filepath)
	if err != nil {
		return fmt.Errorf("failed to open %q: %w", j.bfr.Filepath, err)
	}
	defer reader.Close()
	return bkt.upload(ctx, j.path, reader)
}
