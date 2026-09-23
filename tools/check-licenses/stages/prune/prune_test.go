// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package prune

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

func TestPruner_Run(t *testing.T) {
	validFile := filepath.Join("/", "workspace", "src", "used.cc")
	invalidFile := filepath.Join("/", "workspace", "src", "unused.cc")

	pruner := NewPruner(map[string]bool{
		validFile: true,
	})

	inChan := make(chan pipeline.Project, 2)

	// Project 1 has a valid file (should be kept)
	inChan <- pipeline.Project{
		RootPath: filepath.Join("/", "workspace", "src", "proj1"),
		Files:    []pipeline.FileInfo{{Path: validFile}, {Path: invalidFile}},
	}

	// Project 2 has no valid files (should be dropped)
	inChan <- pipeline.Project{
		RootPath: filepath.Join("/", "workspace", "src", "proj2"),
		Files:    []pipeline.FileInfo{{Path: invalidFile}},
	}
	close(inChan)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	outChan, err := pruner.Run(ctx, inChan)
	if err != nil {
		t.Fatalf("Failed to run pruner: %v", err)
	}

	var results []pipeline.FilteredProject
	for p := range outChan {
		results = append(results, p)
	}

	if len(results) != 1 {
		t.Fatalf("Expected exactly 1 filtered project, got %d", len(results))
	}

	expectedPath := filepath.Join("/", "workspace", "src", "proj1")
	if results[0].RootPath != expectedPath {
		t.Errorf("Expected proj1 to survive pruning, got %v", results[0].RootPath)
	}
}

func TestPruner_EmptyValidFiles(t *testing.T) {
	// If no valid files are provided, it should pass everything through
	pruner := NewPruner(nil)

	inChan := make(chan pipeline.Project, 1)
	inChan <- pipeline.Project{
		RootPath: "/test",
		Files:    []pipeline.FileInfo{{Path: "/test/file.cc"}},
	}
	close(inChan)

	ctx := context.Background()
	outChan, _ := pruner.Run(ctx, inChan)

	count := 0
	for range outChan {
		count++
	}

	if count != 1 {
		t.Errorf("Expected 1 project when ValidFiles is empty, got %d", count)
	}
}

func TestPruner_FilesInReadmeOnly(t *testing.T) {
	projDir := filepath.Join("/", "workspace", "third_party", "foo")
	proj2Dir := filepath.Join("/", "workspace", "third_party", "bar")

	readmePath := filepath.Join(projDir, "README.fuchsia")
	licensePath := filepath.Join(projDir, "LICENSE")
	noticePath := filepath.Join(projDir, "NOTICE.fuchsia")
	ignoredPath := filepath.Join(projDir, "ignored.cc")
	cargoPath := filepath.Join(projDir, "Cargo.toml")
	undeclaredPath := filepath.Join(projDir, "undeclared.cc")
	targetFile := filepath.Join(projDir, "target.cc")
	proj2File := filepath.Join(proj2Dir, "bar.cc")

	p1 := pipeline.Project{
		RootPath: projDir,
		Files: []pipeline.FileInfo{
			{Path: readmePath},
			{Path: licensePath, IsLicenseFile: true},
			{Path: noticePath, IsLicenseFile: true},
			{Path: ignoredPath, IsNonLicense: true},
			{Path: cargoPath},
			{Path: undeclaredPath},
			{Path: targetFile},
		},
	}
	p2 := pipeline.Project{
		RootPath: proj2Dir,
		Files:    []pipeline.FileInfo{{Path: proj2File}},
	}

	// Test 1: FilesInReadmeOnly with explicit file target
	pruner := NewPruner(nil)
	pruner.FilesInReadmeOnly = true
	pruner.TargetFiles = []string{targetFile}

	inChan := make(chan pipeline.Project, 2)
	inChan <- p1
	inChan <- p2
	close(inChan)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	outChan, err := pruner.Run(ctx, inChan)
	if err != nil {
		t.Fatalf("Failed to run pruner: %v", err)
	}

	var results []pipeline.FilteredProject
	for p := range outChan {
		results = append(results, p)
	}

	if len(results) != 1 {
		t.Fatalf("Expected 1 filtered project, got %d", len(results))
	}
	if results[0].RootPath != projDir {
		t.Errorf("Expected root %q, got %q", projDir, results[0].RootPath)
	}

	found := make(map[string]bool)
	for _, f := range results[0].Files {
		found[f.Path] = true
	}

	if !found[targetFile] {
		t.Errorf("Expected targetFile %q to be kept", targetFile)
	}
	if !found[licensePath] {
		t.Errorf("Expected licensePath %q to be kept", licensePath)
	}
	if !found[noticePath] {
		t.Errorf("Expected noticePath %q to be kept", noticePath)
	}
	if !found[ignoredPath] {
		t.Errorf("Expected ignoredPath %q to be kept", ignoredPath)
	}
	if !found[cargoPath] {
		t.Errorf("Expected cargoPath %q to be kept", cargoPath)
	}
	if found[undeclaredPath] {
		t.Errorf("Undeclared file %q should have been pruned", undeclaredPath)
	}

	// Test 2: FilesInReadmeOnly with directory target
	pruner2 := NewPruner(nil)
	pruner2.FilesInReadmeOnly = true
	pruner2.TargetFiles = []string{projDir}

	inChan2 := make(chan pipeline.Project, 2)
	inChan2 <- p1
	inChan2 <- p2
	close(inChan2)

	outChan2, err := pruner2.Run(ctx, inChan2)
	if err != nil {
		t.Fatalf("Failed to run pruner2: %v", err)
	}

	var results2 []pipeline.FilteredProject
	for p := range outChan2 {
		results2 = append(results2, p)
	}

	if len(results2) != 1 {
		t.Fatalf("Expected 1 project for dir target, got %d", len(results2))
	}

	found2 := make(map[string]bool)
	for _, f := range results2[0].Files {
		found2[f.Path] = true
	}

	if !found2[undeclaredPath] {
		t.Errorf("Expected undeclaredPath %q to be kept when dir is targeted", undeclaredPath)
	}
	if !found2[targetFile] {
		t.Errorf("Expected targetFile %q to be kept when dir is targeted", targetFile)
	}
}
