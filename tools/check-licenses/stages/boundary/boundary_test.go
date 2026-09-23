// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package boundary

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

func TestGrouper_Run(t *testing.T) {
	fuchsiaDir := t.TempDir()

	grouper := NewGrouper(
		fuchsiaDir,
		Config{
			BarrierPaths: map[string]bool{"third_party": true, filepath.Join("prebuilt", "foo"): true},
			OutOfTreeReadmes: map[string]string{
				filepath.Join("prebuilt", "virtual"): "/fake/path/to/README.fuchsia",
			},
			FilesInReadmeOnly: false,
		},
	)

	inChan := make(chan pipeline.RawPath, 10)

	// 1. File with a physical README in same dir
	proj1DirRel := filepath.Join("src", "proj1")
	proj1Dir := filepath.Join(fuchsiaDir, proj1DirRel)
	if err := os.MkdirAll(proj1Dir, 0755); err != nil {
		t.Fatal(err)
	}
	readmeContent := []byte(fmt.Sprintf(`License: Android
License File: %s

-------------------- DEPENDENCY DIVIDER --------------------

Location: vendored_lib
License: Chromium
License File: %s
`, filepath.Join(proj1DirRel, "lib", "util.cc"), filepath.Join(proj1DirRel, "vendored_lib", "LICENSE")))

	if err := os.WriteFile(filepath.Join(proj1Dir, "README.fuchsia"), readmeContent, 0644); err != nil {
		t.Fatal(err)
	}

	inChan <- pipeline.RawPath{Path: filepath.Join(proj1Dir, "README.fuchsia"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(proj1Dir, "main.cc"), IsDir: false}

	// 2. File in a child dir of a physical README
	inChan <- pipeline.RawPath{Path: filepath.Join(proj1Dir, "lib", "util.cc"), IsDir: false}

	// 2.5 File in a sub-project (vendored_lib) defined by DEPENDENCY DIVIDER Location
	subProjDir := filepath.Join(proj1Dir, "vendored_lib")
	inChan <- pipeline.RawPath{Path: filepath.Join(subProjDir, "sub_main.cc"), IsDir: false}

	// 3. File behind a Barrier (third_party/foo should be project root)
	proj2Dir := filepath.Join(fuchsiaDir, "third_party", "foo")
	inChan <- pipeline.RawPath{Path: filepath.Join(proj2Dir, "src", "bar.cc"), IsDir: false}

	// 5. File behind a Virtual README
	proj4Dir := filepath.Join(fuchsiaDir, "prebuilt", "virtual")
	inChan <- pipeline.RawPath{Path: filepath.Join(proj4Dir, "bin", "tool"), IsDir: false}

	close(inChan)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	outChan, err := grouper.Run(ctx, inChan)
	if err != nil {
		t.Fatalf("Failed to run grouper: %v", err)
	}

	results := make(map[string][]pipeline.FileInfo)
	for p := range outChan {
		results[p.RootPath] = p.Files
	}

	expectedProj1 := []pipeline.FileInfo{
		{Path: filepath.Join(proj1Dir, "README.fuchsia")},
		{Path: filepath.Join(proj1Dir, "lib", "util.cc"), IsLicenseFile: true},
		{Path: filepath.Join(proj1Dir, "main.cc")},
	}
	if !reflect.DeepEqual(results[proj1Dir], expectedProj1) {
		t.Errorf("Expected proj1 files %v, got %v", expectedProj1, results[proj1Dir])
	}

	expectedSubProj := []pipeline.FileInfo{
		{Path: filepath.Join(subProjDir, "sub_main.cc")},
	}
	if !reflect.DeepEqual(results[subProjDir], expectedSubProj) {
		t.Errorf("Expected subProj files %v, got %v", expectedSubProj, results[subProjDir])
	}

	expectedProj2 := []pipeline.FileInfo{{Path: filepath.Join(proj2Dir, "src", "bar.cc")}}
	if !reflect.DeepEqual(results[proj2Dir], expectedProj2) {
		t.Errorf("Expected proj2 files %v, got %v", expectedProj2, results[proj2Dir])
	}

	expectedProj4 := []pipeline.FileInfo{{Path: filepath.Join(proj4Dir, "bin", "tool")}}
	if !reflect.DeepEqual(results[proj4Dir], expectedProj4) {
		t.Errorf("Expected proj4 files %v, got %v", expectedProj4, results[proj4Dir])
	}
}

func TestGrouper_PackageManifestSubprojects(t *testing.T) {
	fuchsiaDir := t.TempDir()

	golibsDir := filepath.Join(fuchsiaDir, "third_party", "golibs")
	if err := os.MkdirAll(golibsDir, 0755); err != nil {
		t.Fatal(err)
	}

	goModContent := `module go.fuchsia.dev/fuchsia/third_party/golibs

go 1.23

require (
	github.com/spdx/tools-golang v0.5.5
)
`
	goModPath := filepath.Join(golibsDir, "go.mod")
	if err := os.WriteFile(goModPath, []byte(goModContent), 0644); err != nil {
		t.Fatal(err)
	}

	spdxDir := filepath.Join(golibsDir, "vendor", "github.com/spdx/tools-golang")
	if err := os.MkdirAll(spdxDir, 0755); err != nil {
		t.Fatal(err)
	}

	grouper := NewGrouper(
		fuchsiaDir,
		Config{
			BarrierPaths: map[string]bool{"third_party": true},
		},
	)

	inChan := make(chan pipeline.RawPath, 10)
	inChan <- pipeline.RawPath{Path: goModPath, IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(spdxDir, "LICENSE.code"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(spdxDir, "main.go"), IsDir: false}
	close(inChan)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	outChan, err := grouper.Run(ctx, inChan)
	if err != nil {
		t.Fatalf("Failed to run grouper: %v", err)
	}

	results := make(map[string][]pipeline.FileInfo)
	for p := range outChan {
		results[p.RootPath] = p.Files
	}

	// Verify spdx files are grouped under the vendored package root, not squashed into third_party/golibs
	spdxFiles, ok := results[spdxDir]
	if !ok {
		t.Fatalf("Expected group for spdxDir %s, got groups: %v", spdxDir, results)
	}
	expectedPaths := []string{
		filepath.Join(spdxDir, "LICENSE.code"),
		filepath.Join(spdxDir, "main.go"),
	}
	var actualPaths []string
	for _, f := range spdxFiles {
		actualPaths = append(actualPaths, f.Path)
	}
	if !reflect.DeepEqual(actualPaths, expectedPaths) {
		t.Errorf("Expected files %v, got %v", expectedPaths, actualPaths)
	}
}

func TestGrouper_BarrierExceptions(t *testing.T) {
	fuchsiaDir := t.TempDir()

	golibsDir := filepath.Join(fuchsiaDir, "third_party", "golibs")
	if err := os.MkdirAll(golibsDir, 0755); err != nil {
		t.Fatal(err)
	}
	goModContent := "module foo\n\ngo 1.23\n\nrequire (\n\tgithub.com/pkg/errors v0.9.1\n)\n"
	if err := os.WriteFile(filepath.Join(golibsDir, "go.mod"), []byte(goModContent), 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(golibsDir, "imports.go"), []byte("package golibs\n"), 0644); err != nil {
		t.Fatal(err)
	}

	pkgDir := filepath.Join(golibsDir, "vendor", "github.com", "pkg", "errors")
	if err := os.MkdirAll(pkgDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(pkgDir, "LICENSE"), []byte("BSD"), 0644); err != nil {
		t.Fatal(err)
	}

	grouper := NewGrouper(
		fuchsiaDir,
		Config{
			BarrierPaths: map[string]bool{
				"third_party":               true,
				"third_party/golibs/vendor": true,
			},
			BarrierExceptions: map[string]bool{
				"third_party/golibs": true,
			},
		},
	)

	inChan := make(chan pipeline.RawPath, 20)
	inChan <- pipeline.RawPath{Path: filepath.Join(golibsDir, "go.mod"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(golibsDir, "go.sum"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(golibsDir, "imports.go"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(golibsDir, "BUILD.gn"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(golibsDir, "BUILD.bazel"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(golibsDir, "OWNERS"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(golibsDir, "update.sh"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(golibsDir, "vendor", "modules.txt"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(pkgDir, "LICENSE"), IsDir: false}
	close(inChan)

	outChan, err := grouper.Run(context.Background(), inChan)
	if err != nil {
		t.Fatal(err)
	}

	groups := make(map[string][]string)
	for p := range outChan {
		rel, _ := filepath.Rel(fuchsiaDir, p.RootPath)
		for _, f := range p.Files {
			relFile, _ := filepath.Rel(fuchsiaDir, f.Path)
			groups[rel] = append(groups[rel], filepath.ToSlash(relFile))
		}
	}

	// 1. Container files in third_party/golibs bubble up to repository root "."
	rootFiles, hasRoot := groups["."]
	if !hasRoot {
		t.Fatalf("Expected repository root '.' project for golibs container files, got groups: %v", groups)
	}
	for _, expected := range []string{
		"third_party/golibs/go.mod",
		"third_party/golibs/imports.go",
		"third_party/golibs/BUILD.gn",
		"third_party/golibs/vendor/modules.txt",
	} {
		found := false
		for _, rf := range rootFiles {
			if rf == expected {
				found = true
				break
			}
		}
		if !found {
			t.Errorf("Expected root file %s in root group, but not found", expected)
		}
	}

	// 2. Vendored package has its own project group
	relPkg, _ := filepath.Rel(fuchsiaDir, pkgDir)
	pkgFiles, hasPkg := groups[filepath.ToSlash(relPkg)]
	if !hasPkg {
		t.Fatalf("Expected project group for %s, got groups: %v", relPkg, groups)
	}
	if len(pkgFiles) != 1 || pkgFiles[0] != "third_party/golibs/vendor/github.com/pkg/errors/LICENSE" {
		t.Errorf("Unexpected files in pkg group: %v", pkgFiles)
	}

	// 3. ResolveProjectRoot on container file and dir resolves to repository root fuchsiaDir
	importsFile := filepath.Join(golibsDir, "imports.go")
	if gotRoot := grouper.ResolveProjectRoot(importsFile); gotRoot != fuchsiaDir {
		t.Errorf("ResolveProjectRoot(%q) = %q, want %q", importsFile, gotRoot, fuchsiaDir)
	}
	if gotRoot := grouper.ResolveProjectRoot(golibsDir); gotRoot != fuchsiaDir {
		t.Errorf("ResolveProjectRoot(%q) = %q, want %q", golibsDir, gotRoot, fuchsiaDir)
	}

	// 4. BelongsToProject on container file
	if !grouper.BelongsToProject(importsFile, "") {
		t.Errorf("Expected imports.go to belong to root project")
	}
	if grouper.BelongsToProject(importsFile, "third_party/golibs") {
		t.Errorf("Expected imports.go NOT to belong to third_party/golibs")
	}

	// 5. BelongsToProject on vendored file
	pkgLicense := filepath.Join(pkgDir, "LICENSE")
	if !grouper.BelongsToProject(pkgLicense, "third_party/golibs/vendor/github.com/pkg/errors") {
		t.Errorf("Expected pkg LICENSE to belong to vendored pkg project")
	}
	if grouper.BelongsToProject(pkgLicense, "third_party/golibs") {
		t.Errorf("Expected pkg LICENSE NOT to belong to container third_party/golibs")
	}
}
func TestGrouper_ResolveProjectRootAndFindReadme(t *testing.T) {
	fuchsiaDir := t.TempDir()

	// 1. Root virtual README
	virtualDir := filepath.Join(fuchsiaDir, "tools", "check-licenses", "assets", "readmes")
	if err := os.MkdirAll(virtualDir, 0755); err != nil {
		t.Fatal(err)
	}
	rootVirtualReadme := filepath.Join(virtualDir, "README.fuchsia")
	if err := os.WriteFile(rootVirtualReadme, []byte("Name: Fuchsia\nFirst Party: yes\n"), 0644); err != nil {
		t.Fatal(err)
	}

	// 2. third_party/foo with README.fuchsia
	fooDir := filepath.Join(fuchsiaDir, "third_party", "foo")
	if err := os.MkdirAll(filepath.Join(fooDir, "sub"), 0755); err != nil {
		t.Fatal(err)
	}
	fooReadme := filepath.Join(fooDir, "README.fuchsia")
	if err := os.WriteFile(fooReadme, []byte("Name: foo\nLicense: MIT\n"), 0644); err != nil {
		t.Fatal(err)
	}

	// 3. third_party/new_foo WITHOUT README.fuchsia
	newFooDir := filepath.Join(fuchsiaDir, "third_party", "new_foo", "nested", "deep")
	if err := os.MkdirAll(newFooDir, 0755); err != nil {
		t.Fatal(err)
	}
	newFooFile := filepath.Join(newFooDir, "foo.cc")
	if err := os.WriteFile(newFooFile, []byte("int foo() {}\n"), 0644); err != nil {
		t.Fatal(err)
	}

	// 4. src/first_party file
	srcDir := filepath.Join(fuchsiaDir, "src", "lib")
	if err := os.MkdirAll(srcDir, 0755); err != nil {
		t.Fatal(err)
	}
	srcFile := filepath.Join(srcDir, "lib.cc")
	if err := os.WriteFile(srcFile, []byte("int lib() {}\n"), 0644); err != nil {
		t.Fatal(err)
	}

	// 5. prebuilt/third_party/orphan WITHOUT README
	orphanDir := filepath.Join(fuchsiaDir, "prebuilt", "third_party", "orphan")
	if err := os.MkdirAll(orphanDir, 0755); err != nil {
		t.Fatal(err)
	}
	orphanFile := filepath.Join(orphanDir, "tool")
	if err := os.WriteFile(orphanFile, []byte("binary\n"), 0644); err != nil {
		t.Fatal(err)
	}

	grouper := NewGrouper(
		fuchsiaDir,
		Config{
			BarrierPaths: map[string]bool{
				"third_party":          true,
				"prebuilt":             true,
				"prebuilt/third_party": true,
			},
			OutOfTreeReadmes: map[string]string{},
		},
	)

	// Case 1: third_party/foo with README
	if root := grouper.ResolveProjectRoot(filepath.Join(fooDir, "sub", "file.cc")); root != fooDir {
		t.Errorf("Expected root %s, got %s", fooDir, root)
	}
	r, p, err := grouper.FindProjectReadme(filepath.Join(fooDir, "sub", "file.cc"))
	if err != nil || r == nil || p != fooReadme {
		t.Errorf("Expected FindProjectReadme to return foo README, got r=%v, p=%s, err=%v", r, p, err)
	}

	// Case 2: third_party/new_foo WITHOUT README (behind barrier)
	expectedNewFooRoot := filepath.Join(fuchsiaDir, "third_party", "new_foo")
	if root := grouper.ResolveProjectRoot(newFooFile); root != expectedNewFooRoot {
		t.Errorf("Expected root %s for file in barrier dir without README, got %s", expectedNewFooRoot, root)
	}
	r, p, err = grouper.FindProjectReadme(newFooFile)
	if err != nil || r != nil || p != "" {
		t.Errorf("Expected FindProjectReadme to return nil for file behind barrier without README, got r=%v, p=%s, err=%v", r, p, err)
	}

	// Case 3: Direct directory target on third_party/new_foo
	if root := grouper.ResolveProjectRoot(expectedNewFooRoot); root != expectedNewFooRoot {
		t.Errorf("Expected root %s for directory behind barrier without README, got %s", expectedNewFooRoot, root)
	}
	r, p, err = grouper.FindProjectReadme(expectedNewFooRoot)
	if err != nil || r != nil || p != "" {
		t.Errorf("Expected FindProjectReadme to return nil for directory behind barrier without README, got r=%v, p=%s, err=%v", r, p, err)
	}

	// Case 4: First-party file (no barrier) resolves to fuchsiaDir and root virtual README
	if root := grouper.ResolveProjectRoot(srcFile); root != fuchsiaDir {
		t.Errorf("Expected root %s for 1st-party file, got %s", fuchsiaDir, root)
	}
	r, p, err = grouper.FindProjectReadme(srcFile)
	if err != nil || r == nil || p != rootVirtualReadme {
		t.Errorf("Expected FindProjectReadme to return root virtual README for 1st-party file, got r=%v, p=%s, err=%v", r, p, err)
	}

	// Case 5: prebuilt/third_party/orphan WITHOUT README
	if root := grouper.ResolveProjectRoot(orphanFile); root != orphanDir {
		t.Errorf("Expected root %s for orphan prebuilt tool, got %s", orphanDir, root)
	}
	r, p, err = grouper.FindProjectReadme(orphanFile)
	if err != nil || r != nil || p != "" {
		t.Errorf("Expected FindProjectReadme to return nil for orphan prebuilt tool, got r=%v, p=%s, err=%v", r, p, err)
	}

	// Case 6: BelongsToProject checks with barrier-aware logic
	if !grouper.BelongsToProject(newFooFile, "third_party/new_foo") {
		t.Errorf("Expected newFooFile to belong to third_party/new_foo")
	}
	if !grouper.BelongsToProject(newFooFile, "//third_party/new_foo") {
		t.Errorf("Expected newFooFile to belong to //third_party/new_foo")
	}
	if !grouper.BelongsToProject(newFooFile, expectedNewFooRoot) {
		t.Errorf("Expected newFooFile to belong to absolute expectedNewFooRoot")
	}
	if grouper.BelongsToProject(newFooFile, "") {
		t.Errorf("Expected newFooFile NOT to belong to root workspace")
	}
	if !grouper.BelongsToProject(srcFile, "") {
		t.Errorf("Expected 1st-party srcFile to belong to root workspace")
	}
	if !grouper.BelongsToProject(srcFile, "//") {
		t.Errorf("Expected 1st-party srcFile to belong to //")
	}
	if !grouper.BelongsToProject(srcFile, fuchsiaDir) {
		t.Errorf("Expected 1st-party srcFile to belong to absolute fuchsiaDir")
	}
}

func TestGrouper_DartSubpackages(t *testing.T) {
	fuchsiaDir := t.TempDir()

	pkgDir := filepath.Join(fuchsiaDir, "third_party", "dart-pkg", "pub", "dwds")
	debugExtDir := filepath.Join(pkgDir, "debug_extension")
	debugExtMv3Dir := filepath.Join(pkgDir, "debug_extension_mv3")

	for _, d := range []string{pkgDir, debugExtDir, debugExtMv3Dir} {
		if err := os.MkdirAll(d, 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(d, "pubspec.yaml"), []byte("name: dwds\n"), 0644); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(pkgDir, "LICENSE"), []byte("BSD"), 0644); err != nil {
		t.Fatal(err)
	}

	grouper := NewGrouper(
		fuchsiaDir,
		Config{
			BarrierPaths: map[string]bool{
				"third_party":              true,
				"third_party/dart-pkg/pub": true,
			},
		},
	)

	inChan := make(chan pipeline.RawPath, 10)
	inChan <- pipeline.RawPath{Path: filepath.Join(pkgDir, "pubspec.yaml"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(pkgDir, "LICENSE"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(debugExtDir, "pubspec.yaml"), IsDir: false}
	inChan <- pipeline.RawPath{Path: filepath.Join(debugExtMv3Dir, "pubspec.yaml"), IsDir: false}
	close(inChan)

	outChan, err := grouper.Run(context.Background(), inChan)
	if err != nil {
		t.Fatal(err)
	}

	var projects []string
	for p := range outChan {
		rel, _ := filepath.Rel(fuchsiaDir, p.RootPath)
		projects = append(projects, filepath.ToSlash(rel))
	}

	expected := []string{"third_party/dart-pkg/pub/dwds"}
	if len(projects) != 1 || projects[0] != expected[0] {
		t.Fatalf("Expected projects %v, got %v", expected, projects)
	}

	// Verify ResolveProjectRoot and BelongsToProject for Dart subpackages
	debugExtFile := filepath.Join(debugExtDir, "pubspec.yaml")
	expectedDwdsRoot := filepath.Join(fuchsiaDir, "third_party", "dart-pkg", "pub", "dwds")
	if gotRoot := grouper.ResolveProjectRoot(debugExtFile); gotRoot != expectedDwdsRoot {
		t.Errorf("ResolveProjectRoot(%q) = %q, want %q", debugExtFile, gotRoot, expectedDwdsRoot)
	}
	if !grouper.BelongsToProject(debugExtFile, "third_party/dart-pkg/pub/dwds") {
		t.Errorf("Expected debug_extension/pubspec.yaml to belong to dwds project")
	}
}
