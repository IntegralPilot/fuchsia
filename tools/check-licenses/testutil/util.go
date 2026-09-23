// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Package testutil provides helper functions for check-licenses tests.
package testutil

import (
	"io/fs"
	"os"
	"path/filepath"
	"testing"
)

// DumpTestData recursively writes the content of the embedded file system to
// the specified directory.
func DumpTestData(t *testing.T, testDataFS fs.FS, path string) {
	t.Helper()
	err := fs.WalkDir(testDataFS, ".", func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return os.MkdirAll(filepath.Join(path, p), 0755)
		}
		data, err := fs.ReadFile(testDataFS, p)
		if err != nil {
			return err
		}
		return os.WriteFile(filepath.Join(path, p), data, 0644)
	})
	if err != nil {
		t.Fatalf("Failed to dump test data: %v", err)
	}
}
