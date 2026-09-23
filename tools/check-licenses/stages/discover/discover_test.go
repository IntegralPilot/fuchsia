// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package discover

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestCrawler_Run(t *testing.T) {
	tempDir := t.TempDir()

	// Resolve the tempDir to absolute path since our Crawler forces absolute
	absTempDir, err := filepath.Abs(tempDir)
	if err != nil {
		t.Fatal(err)
	}

	// Create a nested file
	nestedDir := filepath.Join(absTempDir, "subdir")
	if err := os.Mkdir(nestedDir, 0755); err != nil {
		t.Fatal(err)
	}
	testFilePath := filepath.Join(nestedDir, "test.txt")
	if err := os.WriteFile(testFilePath, []byte("test content"), 0644); err != nil {
		t.Fatal(err)
	}

	crawler := NewCrawler(absTempDir, Config{
		SkipPaths:    map[string]bool{"subdir_skipped": true},
		SkipAnywhere: map[string]bool{"skip_me": true},
	})

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	pathChan, err := crawler.Run(ctx, []string{absTempDir})
	if err != nil {
		t.Fatalf("Failed to run crawler: %v", err)
	}

	foundPaths := make(map[string]bool)
	for p := range pathChan {
		foundPaths[p.Path] = true
	}

	// We expect to find the root dir, the subdir, and the test.txt file
	expectedPaths := []string{
		absTempDir,
		nestedDir,
		testFilePath,
	}

	for _, expected := range expectedPaths {
		if !foundPaths[expected] {
			t.Errorf("Expected crawler to emit path %q, but it was not found. Found: %v", expected, foundPaths)
		}
	}
}
