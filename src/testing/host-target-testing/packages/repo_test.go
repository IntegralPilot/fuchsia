// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package packages

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"testing"
)

func TestCreateAndEditPackage(t *testing.T) {
	ctx := context.Background()

	parentDir := t.TempDir()
	pkgRepo, expandDir := createAndExpandPackage(t, parentDir)
	pkgBuilder, err := NewPackageBuilderFromDir(expandDir, "testpackage", "0", "testrepository.com")
	if err != nil {
		t.Fatalf("Failed to parse package from %s. %s", expandDir, err)
	}
	defer pkgBuilder.Close()

	fullPkgName := pkgBuilder.Name + "/" + pkgBuilder.Version
	newResource := "blah/z"

	srcPkg, err := pkgRepo.OpenPackage(ctx, fullPkgName)
	if err != nil {
		t.Fatalf("Repo does not contain '%s'. %s", fullPkgName, err)
	}
	if _, err := srcPkg.ReadFile(ctx, newResource); err == nil {
		t.Fatalf("%s should not be found in package", newResource)
	}

	pkg, err := pkgRepo.EditPackage(ctx, srcPkg, "testpackage_prime/0", func(tempDir string) error {
		contents := bytes.NewReader([]byte(newResource))
		data, err := io.ReadAll(contents)
		if err != nil {
			return fmt.Errorf("failed to read file: %w", err)
		}

		tempPath := filepath.Join(tempDir, newResource)
		if err := os.MkdirAll(filepath.Dir(tempPath), os.ModePerm); err != nil {
			return fmt.Errorf("failed to create parent directories for %q: %w", tempPath, err)
		}

		if err := os.WriteFile(tempPath, data, 0644); err != nil {
			return fmt.Errorf("failed to write data to %q: %w", tempDir, err)
		}

		return nil
	})

	if err != nil {
		t.Fatalf("Failed to edit the package '%s'. %s", fullPkgName, err)
	}
	if data, err := pkg.ReadFile(ctx, newResource); err != nil {
		t.Fatalf("%s should be in package.", newResource)
	} else {
		if string(data) != newResource {
			t.Fatalf("%s should have value %s but is %s", newResource, newResource, data)
		}
	}
}
