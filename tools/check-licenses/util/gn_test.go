// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package util

import (
	"compress/gzip"
	"embed"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/testutil"
)

//go:embed testdata/*
var testDataFS embed.FS

func TestFilterTargetsEmpty(t *testing.T) {
	tempDir := t.TempDir()
	testutil.DumpTestData(t, testDataFS, tempDir)
	testDataDir := filepath.Join(tempDir, "testdata")
	root := filepath.Join(testDataDir, "empty")
	zippedProjectJson := filepath.Join(root, "project.json.gz")
	projectJson := unzipProjectJson(t, zippedProjectJson)
	gen, err := LoadGen(projectJson)
	if err != nil {
		t.Errorf("%s: expected no error, got: %v.", t.Name(), err)
	}

	if len(gen.Targets) > 0 {
		t.Errorf("%s: expected to find no targets, found %d: %v.", t.Name(), len(gen.Targets), gen.Targets)
	}
}

func loadWantJSON(wantFile string, t *testing.T) map[string]*Target {
	wantFileContent, err := os.ReadFile(wantFile)
	if err != nil {
		t.Fatalf("%s: failed to read in want.json file [%s]: %v\n", t.Name(), wantFile, err)
	}

	var want map[string]*Target
	err = json.Unmarshal(wantFileContent, &want)
	if err != nil {
		t.Fatalf("%s: failed to unmarshal want.json data [%s]: %v\n", t.Name(), wantFile, err)
	}
	return want
}

func unzipProjectJson(t *testing.T, path string) string {
	t.Helper()

	f, err := os.Open(path)
	if err != nil {
		t.Fatalf("%s: failed to open zipped project.json file [%s]: %v", t.Name(), path, err)
	}
	defer f.Close()

	stream, err := gzip.NewReader(f)
	if err != nil {
		t.Fatalf("%s: failed to create gzip reader: %v", t.Name(), err)
	}

	dir := t.TempDir()
	outputFilePath := filepath.Join(dir, "project.json")
	outputFile, err := os.Create(outputFilePath)
	if err != nil {
		t.Fatalf("%s: failed to create project.json file [%s]: %v", t.Name(), outputFilePath, err)
	}
	defer outputFile.Close()

	_, err = io.Copy(outputFile, stream)
	if err != nil {
		t.Fatalf("%s: failed to copy zipped contents into output file [%s]: %v", t.Name(), outputFilePath, err)
	}

	return outputFilePath
}
