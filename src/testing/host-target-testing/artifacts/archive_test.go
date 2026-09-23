// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifacts

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDownload(t *testing.T) {
	tmpDir := t.TempDir()

	// The expected command will be something like `artifacts cp -build <id>
	// -dst <dst> -srcs-file <srcs-file>`. Make a mock script to copy src to the
	// dst directory. If src doesn't exist, create it so that a retry of the
	// command will succeed.
	mockArtifactsWithSrcsFileScript := filepath.Join(tmpDir, "artifacts_retry_srcsfile")
	if err := os.WriteFile(mockArtifactsWithSrcsFileScript, []byte(`#!/bin/bash
	set -x
	dst="$5"
	srcs_file=$(cat "$7")
	mkdir -p $dst
	cp -r $srcs_file "$dst" || (for src in $srcs_file; do echo contents > $src; done && exit 1)
	`), os.ModePerm); err != nil {
		t.Fatal(err)
	}
	srcDir := filepath.Join(tmpDir, "src_dir")
	if err := os.Mkdir(srcDir, os.ModePerm); err != nil {
		t.Fatal(err)
	}
	srcFile := filepath.Join(srcDir, "src_file")
	if err := os.WriteFile(srcFile, []byte("src"), os.ModePerm); err != nil {
		t.Fatal(err)
	}

	tests := []struct {
		name          string
		srcs          []string
		dst           string
		expectedFiles map[string]string
		forceRetry    bool
	}{
		{
			name: "download dir",
			srcs: []string{srcDir},
			dst:  "download_dir",
			expectedFiles: map[string]string{
				filepath.Join("download_dir", "src_dir", "src_file"): "src",
			},
		},
		{
			name: "download file",
			srcs: []string{srcFile},
			dst:  "download_file",
			expectedFiles: map[string]string{
				filepath.Join("download_file", "src_file"): "src",
			},
		},
		{
			name: "retry download",
			srcs: []string{filepath.Join(tmpDir, "new_src")},
			dst:  "retry_download_file",
			expectedFiles: map[string]string{
				filepath.Join("retry_download_file", "new_src"): "contents",
			},
			forceRetry: true,
		},
		{
			name: "retry download with srcs-file",
			srcs: []string{filepath.Join(tmpDir, "new_src"), filepath.Join(tmpDir, "new_src2")},
			dst:  "retry_download_srcs_file",
			expectedFiles: map[string]string{
				filepath.Join("retry_download_srcs_file", "new_src"):  "contents",
				filepath.Join("retry_download_srcs_file", "new_src2"): "contents",
			},
			forceRetry: true,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			artifactsPath := mockArtifactsWithSrcsFileScript
			if test.forceRetry {
				// Make sure all srcs don't exist so that the script fails the first time.
				for _, src := range test.srcs {
					os.Remove(src)
				}
			}
			archive := NewArchive("", artifactsPath)
			tmpDstDir := filepath.Join(tmpDir, "test")
			if err := os.Mkdir(tmpDstDir, os.ModePerm); err != nil {
				t.Fatal(err)
			}
			defer os.RemoveAll(tmpDstDir)
			if err := archive.download(context.Background(), "id", false, filepath.Join(tmpDstDir, test.dst), test.srcs); err != nil {
				t.Fatal(err)
			}
			for expectedFile, expectedContents := range test.expectedFiles {
				data, err := os.ReadFile(filepath.Join(tmpDstDir, expectedFile))
				if err != nil {
					t.Error(err)
				}
				if strings.Trim(string(data), "\n") != expectedContents {
					t.Errorf("expected contents: %s, got: %s", expectedContents, data)
				}
			}
		})
	}
}

func TestDownloadNotExist(t *testing.T) {
	tmpDir := t.TempDir()
	// The expected command will be something like `artifacts cp -build <id> -src <src> -dst <dst> -srcs-file <srcs-file>`.
	// Make a mock script to always say the artifact doesn't exist.
	mockArtifactsScript := filepath.Join(tmpDir, "artifacts")
	if err := os.WriteFile(mockArtifactsScript, []byte("#!/bin/bash\necho \"object doesn't exist\" >&2\nexit 1"), os.ModePerm); err != nil {
		t.Fatal(err)
	}

	archive := NewArchive("", mockArtifactsScript)
	err := archive.download(context.Background(), "id", false, "dst", []string{"src"})
	if !os.IsNotExist(err) {
		t.Fatal("expected not exist error, got:", err)
	}
}
