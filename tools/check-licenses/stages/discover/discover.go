// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package discover

import (
	"context"
	"io/fs"
	"log"
	"path"
	"path/filepath"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/metrics"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

// Crawler implements pipeline.Discoverer using standard Go filesystem traversal.
// It filters paths based on FuchsiaDir, SkipPaths, and SkipAnywhere maps.
type Crawler struct {
	FuchsiaDir   string
	SkipPaths    map[string]bool
	SkipAnywhere map[string]bool
}

// NewCrawler creates a new stateless crawler.
func NewCrawler(fuchsiaDir string, config Config) *Crawler {
	absFuchsiaDir, err := filepath.Abs(fuchsiaDir)
	if err == nil {
		fuchsiaDir = absFuchsiaDir
	}

	return &Crawler{
		FuchsiaDir:   fuchsiaDir,
		SkipPaths:    config.SkipPaths,
		SkipAnywhere: config.SkipAnywhere,
	}
}

// isSkipped checks if the given absolute path matches any skip rules.
func (c *Crawler) isSkipped(absPath string) bool {
	relPath, err := filepath.Rel(c.FuchsiaDir, absPath)
	if err != nil {
		base := filepath.Base(absPath)
		if base != "." && base != ".." && strings.HasPrefix(base, ".") {
			return true
		}
		return c.SkipAnywhere[base]
	}

	slashRel := filepath.ToSlash(relPath)
	for p := slashRel; p != "." && p != "" && p != "/"; p = path.Dir(p) {
		base := path.Base(p)
		// Automatically skip all hidden directories and files (e.g. .git, .mise, .vscode, .cipd, etc.).
		// We explicitly check that base is not "." or ".." so that the workspace root itself is never skipped.
		if base != "." && base != ".." && strings.HasPrefix(base, ".") {
			return true
		}
		if c.SkipAnywhere[base] || c.SkipPaths[p] {
			return true
		}
	}
	return false
}

// Run walks the given root directories and streams discovered paths into the returned channel.
func (c *Crawler) Run(ctx context.Context, rootDirs []string) (<-chan pipeline.RawPath, error) {
	out := make(chan pipeline.RawPath, 1000)

	go func() {
		defer close(out)
		defer metrics.DirectoryTraversalDuration.Track()()

		for _, root := range rootDirs {
			// Resolve absolute path to ensure consistent downstream processing
			absRoot, err := filepath.Abs(root)
			if err != nil {
				log.Printf("Failed to resolve absolute path for %q: %v\n", root, err)
				continue
			}

			err = filepath.WalkDir(absRoot, func(path string, d fs.DirEntry, err error) error {
				// Check for context cancellation
				if ctx.Err() != nil {
					return ctx.Err()
				}
				if err != nil {
					// Log and continue if a specific file/dir has permissions issues
					log.Printf("Error accessing path %q: %v\n", path, err)
					return nil
				}

				if c.isSkipped(path) {
					if d.IsDir() {
						return filepath.SkipDir
					}
					return nil
				}

				if !d.IsDir() {
					metrics.FilesProcessed.Inc("discovered")
				}

				// Emit the path
				select {
				case <-ctx.Done():
					return ctx.Err()
				case out <- pipeline.RawPath{
					Path:  path,
					IsDir: d.IsDir(),
				}:
				}
				return nil
			})

			if err != nil && err != context.Canceled && err != context.DeadlineExceeded {
				log.Printf("Error walking directory %q: %v\n", absRoot, err)
			}
		}
	}()

	return out, nil
}
