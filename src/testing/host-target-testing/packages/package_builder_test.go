// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package packages

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"

	versionHistory "go.fuchsia.dev/fuchsia/src/lib/versioning/version-history/go"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
)

var hostDir = map[string]string{"arm64": "host_arm64", "amd64": "host_x64"}[runtime.GOARCH]

const testPrivateKey = `-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtz
c2gtZWQyNTUxOQAAACD1kR/9hRovUqG4rOa+v+vG3yMhR8P1fVdFkX43d1EWRQ
AAAJD6P+w0+j/sNAAAAAtzc2gtZWQyNTUxOQAAACD1kR/9hRovUqG4rOa+v+vG
3yMhR8P1fVdFkX43d1EWRQAAAEA9HkX9hRovUqG4rOa+v+vG3yMhR8P1fVdFkX
43d1EWRQAAACD1kR/9hRovUqG4rOa+v+vG3yMhR8P1fVdFkX43d1EWRQAAAAEA
AAAEc2xncgECAwQFBgcI
-----END OPENSSH PRIVATE KEY-----`

const testPublicKey = `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPWRH/2FGi9Sobis5r6/68bfIyFHw/V9V0WRfjd3URZF test@example.com`

func generatePlaceholderSshKey(t *testing.T, path string) {
	if err := os.WriteFile(path, []byte(testPrivateKey), 0600); err != nil {
		t.Fatalf("failed to write placeholder ssh key: %s", err)
	}
	if err := os.WriteFile(path+".pub", []byte(testPublicKey), 0644); err != nil {
		t.Fatalf("failed to write placeholder ssh public key: %s", err)
	}
}

// createTestPackage fills the given directory with a new repository.
func createTestPackage(t *testing.T, dir string) (*Repository, build.MerkleRoot) {
	ctx := context.Background()

	// Initialize a repo.
	t.Logf("Creating repo at %s", dir)
	blobsDir := filepath.Join(dir, "blobs")

	// Create a config.
	config := build.TestConfig()
	t.Logf("Creating meta.far in %s", config.OutputDir)

	config.PkgABIRevision = versionHistory.History().ExampleSupportedAbiRevisionForTests()

	build.BuildTestPackage(config)
	defer os.RemoveAll(filepath.Dir(config.OutputDir))

	// Grab the merkle of the config's package manifest.
	manifestPath := filepath.Join(config.OutputDir, "package_manifest.json")
	manifest, err := os.ReadFile(manifestPath)
	if err != nil {
		t.Fatal(err)
	}
	var packageManifest build.PackageManifest
	if err := json.Unmarshal(manifest, &packageManifest); err != nil {
		t.Fatal(err)
	}

	foundMerkle := false
	var metaMerkle build.MerkleRoot
	for _, blob := range packageManifest.Blobs {
		if blob.Path == "meta/" {
			foundMerkle = true
			metaMerkle = blob.Merkle
			break
		}
	}
	if !foundMerkle {
		t.Fatal("did not find meta.far in manifest")
	}

	// Publish the config to the repo.
	keyPath := filepath.Join(t.TempDir(), "placeholder_ssh_key")
	generatePlaceholderSshKey(t, keyPath)
	t.Setenv("FUCHSIA_SSH_KEY", keyPath)

	runDir := ffx.NewRunDir(filepath.Join(t.TempDir(), "ffx-run-dir"))
	ffxTool, err := ffx.NewFFXTool("host-tools/ffx", runDir)
	if err != nil {
		t.Fatalf("failed to create FFXTool: %s", err)
	}
	keysDir := filepath.Join(hostDir, "test_data/ffx_lib_pkg/empty-repo/keys")
	err = ffxTool.RepositoryCreate(ctx, dir, keysDir)
	if err != nil {
		t.Fatalf("failed to create repository: %s", err)
	}
	err = exec.Command("cp", "-r", keysDir, dir).Run()
	if err != nil {
		t.Fatalf("failed to copy keys: %s", err)
	}
	blobsDir = filepath.Join(dir, "blobs")
	if err := os.MkdirAll(blobsDir, os.ModePerm); err != nil {
		t.Fatalf("failed to create blobs directory: %s", err)
	}
	blobType := 1

	pkgRepo, err := NewRepository(ctx, dir, NewDirBlobStore(blobsDir), ffxTool, &blobType)
	if err != nil {
		t.Fatalf("failed to read repo: %s", err)
	}
	err = pkgRepo.Publish(ctx, manifestPath)
	if err != nil {
		t.Fatalf("failed to publish package: %s", err)
	}
	return pkgRepo, metaMerkle
}

// expandPackage expands the given merkle from the given repository into the given directory.
func expandPackage(t *testing.T, pkgRepo *Repository, merkle build.MerkleRoot, dir string) {
	ctx := context.Background()

	// Parse the package we want.
	pkg, err := newPackage(ctx, pkgRepo, "", merkle)
	if err != nil {
		t.Fatalf("failed to read package: %s", err)
	}

	// Expand to the given directory.
	if err = pkg.Expand(ctx, dir); err != nil {
		t.Fatalf("failed to expand to dir: %s", err)
	}
}

// createAndExpandPackage creates temporary directories and expands a test package, returning the expand directory.
func createAndExpandPackage(t *testing.T, parentDir string) (*Repository, string) {
	dir := filepath.Join(parentDir, "package")
	if err := os.Mkdir(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	pkgRepo, metaMerkle := createTestPackage(t, dir)
	expand := filepath.Join(parentDir, "expand")
	if err := os.Mkdir(expand, 0o700); err != nil {
		t.Fatal(err)
	}
	expandPackage(t, pkgRepo, metaMerkle, expand)
	return pkgRepo, expand
}

func TestAddResource(t *testing.T) {
	parentDir := t.TempDir()
	_, expandDir := createAndExpandPackage(t, parentDir)
	pkgBuilder, err := NewPackageBuilderFromDir(expandDir, "testpackage", "0", "testrepository.com")
	if err != nil {
		t.Fatalf("Failed to parse package from %s. %s", expandDir, err)
	}
	defer pkgBuilder.Close()

	newResource := "blah/z"

	// Confirm the new resource doesn't exist yet.
	if _, ok := pkgBuilder.Contents[newResource]; ok {
		t.Fatalf("Test resource %s should not exist yet in the package.", newResource)
	}

	pkgBuilder.AddResource(newResource, bytes.NewReader([]byte(newResource)))

	// Confirm the file and contents were added.
	path, ok := pkgBuilder.Contents[newResource]
	if !ok {
		t.Fatalf("Test resource %s failed to be added.", newResource)
	}
	newData, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("Failed to read contents of %s. %s", newResource, err)
	}
	if string(newData) != newResource {
		t.Fatalf("%s expects to have %s, but has %s", newResource, newResource, string(newData))
	}

	expectedFiles := map[string]struct{}{
		"blah/z":                        {},
		"meta/contents":                 {},
		"meta/foo/one":                  {},
		"meta/fuchsia.abi/abi-revision": {},
		"meta/package":                  {},
	}
	for _, item := range build.TestFiles {
		expectedFiles[item] = struct{}{}
	}

	for key := range pkgBuilder.Contents {
		if _, ok := expectedFiles[key]; !ok {
			t.Errorf("File %s is not expected", key)
		}
	}

	if len(expectedFiles) != len(pkgBuilder.Contents) {
		t.Errorf("Package contents has %d files, should have %d", len(pkgBuilder.Contents), len(expectedFiles))
	}

	if err := pkgBuilder.AddResource(newResource, bytes.NewReader([]byte(newResource))); err == nil {
		t.Fatalf("Resource %s should have failed to be added twice.", newResource)
	}
}

func TestPublish(t *testing.T) {
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

	// Confirm package in repo is as expected.
	pkg, err := pkgRepo.OpenPackage(ctx, fullPkgName)
	if err != nil {
		t.Fatalf("Repo does not contain '%s'. %s", fullPkgName, err)
	}
	if _, err := pkg.ReadFile(ctx, newResource); err == nil {
		t.Fatalf("%s should not be found in package", newResource)
	}

	// Add resource to package.
	pkgBuilder.AddResource(newResource, bytes.NewReader([]byte(newResource)))

	// Update repo with updated package. We don't check the merkle since the package includes randomly generated files.
	actualPkg, err := pkgBuilder.Publish(ctx, pkgRepo)
	if err != nil {
		t.Fatalf("Publishing package failed. %s", err)
	}

	if actualPkg.Path() != fullPkgName {
		t.Fatalf("package path should be %q, not %q", fullPkgName, actualPkg.Path())
	}

	deliveryBlobType := 1
	_, err = pkgRepo.blobStore.OpenBlob(ctx, &deliveryBlobType, actualPkg.Merkle())
	if err != nil {
		t.Fatalf("Delivery blob does not exist '%s'. %s", actualPkg.Merkle(), err)
	}

	keyPath2 := filepath.Join(t.TempDir(), "placeholder_ssh_key2")
	generatePlaceholderSshKey(t, keyPath2)
	t.Setenv("FUCHSIA_SSH_KEY", keyPath2)

	runDir := ffx.NewRunDir(filepath.Join(t.TempDir(), "ffx-run-dir"))
	ffxTool, err := ffx.NewFFXTool("host-tools/ffx", runDir)
	if err != nil {
		t.Fatalf("failed to create FFXTool: %s", err)
	}
	pkgRepo, err = NewRepository(ctx, pkgRepo.rootDir, pkgRepo.blobStore, ffxTool, &deliveryBlobType)

	// Confirm that the package is published and updated.
	pkg, err = pkgRepo.OpenPackage(ctx, fullPkgName)
	if err != nil {
		t.Fatalf("Repo does not contain '%s'. %s", fullPkgName, err)
	}
	if data, err := pkg.ReadFile(ctx, newResource); err != nil {
		t.Fatalf("%s should be in package.", newResource)
	} else {
		if string(data) != newResource {
			t.Fatalf("%s should have value %s but is %s", newResource, newResource, data)
		}
	}
}
