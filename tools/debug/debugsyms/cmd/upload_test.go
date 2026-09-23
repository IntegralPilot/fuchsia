// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"fmt"
	"os"
	"strings"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/debug/elflib"
)

func TestQueue(t *testing.T) {
	buildID := "foo"
	filename := fmt.Sprintf("%s.debug", buildID)
	tmpFile, err := os.CreateTemp("", filename)
	if err != nil {
		t.Fatalf("Failed to create tempfile: %v", err)
	}
	defer os.Remove(tmpFile.Name())
	binaryFileRef := elflib.NewBinaryFileRef(tmpFile.Name(), buildID)

	c, err := queue([]elflib.BinaryFileRef{binaryFileRef})
	if err != nil {
		t.Fatalf("Unexpected error from queue(): %s", err)
	}

	var jobs []job
	for j := range c {
		jobs = append(jobs, j)
	}
	if len(jobs) != 2 {
		t.Fatalf("Expected queue() to produce 2 jobs but got %d", len(jobs))
	}
	// Relative order of zxdb and debuginfod jobs doesn't actually matter, but
	// ignoring order would introduce too much complexity here.
	if !strings.HasPrefix(jobs[0].path, zxdbNamespace) {
		t.Errorf("Expected queue() to queue a zxdb upload job first but found: %+v", jobs[0])
	}
	if !strings.HasPrefix(jobs[1].path, debuginfodNamespace) {
		t.Errorf("Expected queue() to queue a debuginfod upload job last but found: %+v", jobs[1])
	}
}
