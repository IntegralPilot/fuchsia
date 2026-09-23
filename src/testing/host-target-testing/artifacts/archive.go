// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifacts

import (
	"bufio"
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
)

// Archive allows interacting with the build artifact repository.
type Archive struct {
	// lkg (typically found in $FUCHSIA_DIR/prebuilt/tools/lkg/lkg) is
	// used to look up the latest build id for a given builder.
	lkgPath string

	// artifacts (typically found in $FUCHSIA_DIR/prebuilt/tools/artifacts/artifacts)
	// is used to download artifacts for a given build id.
	artifactsPath string
}

// NewArchive creates a new Archive.
func NewArchive(lkgPath string, artifactsPath string) *Archive {
	return &Archive{
		lkgPath:       lkgPath,
		artifactsPath: artifactsPath,
	}
}

// GetBuilder returns a Builder with the given name and Archive.
func (a *Archive) GetBuilder(name string) *Builder {
	return &Builder{archive: a, name: name}
}

// GetBuildByID returns an ArtifactsBuild for fetching artifacts for the build
// with the given id.
func (a *Archive) GetBuildByID(
	ctx context.Context,
	id string,
	dir string,
) (*ArtifactsBuild, error) {
	// Make sure the build exists.
	srcs, err := a.list(ctx, id)
	if err != nil {
		return nil, err
	}

	srcsMap := make(map[string]struct{})
	for _, src := range srcs {
		srcsMap[src] = struct{}{}
	}

	return &ArtifactsBuild{
		id:       id,
		archive:  a,
		buildDir: filepath.Join(dir, id),
		blobsDir: filepath.Join(dir, "blobs"),
		srcs:     srcsMap,
	}, nil
}

// list artifacts that make up a build id `buildID`.
func (a *Archive) list(ctx context.Context, buildID string) ([]string, error) {
	args := []string{"ls", "-build", buildID}
	stdout, stderr, err := util.RunCommand(ctx, a.artifactsPath, args...)
	if err != nil {
		if len(stderr) != 0 {
			fmt.Printf("artifacts output: \n%s", stdout)
			fmt.Printf("artifacts stderr: \n%s", stderr)
			return nil, fmt.Errorf("artifacts failed: %w: %s", err, string(stderr))
		}
		return nil, fmt.Errorf("artifacts failed: %w", err)
	}

	var lines []string
	sc := bufio.NewScanner(bytes.NewReader(stdout))
	for sc.Scan() {
		lines = append(lines, sc.Text())
	}

	return lines, nil
}

// Download artifacts from the build id `buildID` and write the `srcs` artifacts
// to the `dstDir` directory.
func (a *Archive) download(
	ctx context.Context,
	buildID string,
	fromRoot bool,
	dstDir string,
	srcs []string,
) error {
	tmpDir, err := os.MkdirTemp("", "download")
	if err != nil {
		return err
	}
	defer os.RemoveAll(tmpDir)

	// Filter out any duplicate sources.
	srcs = removeDuplicates(srcs)

	var filesToDownload []string
	var filesToSkip []string
	for _, src := range srcs {
		path := filepath.Join(dstDir, src)

		// We only need to download the file if it doesn't
		// exist locally, or the local path is a directory
		// (since we don't know what files exist in a directory
		// ahead of time).
		if st, err := os.Stat(path); err != nil || st.IsDir() {
			filesToDownload = append(filesToDownload, src)
		} else {
			filesToSkip = append(filesToSkip, src)
		}
	}
	srcs = filesToDownload

	logger.Infof(ctx, "skipping %d files to download", len(filesToSkip))
	if len(filesToDownload) == 0 {
		// Skip downloading if the files are already present in the build dir.
		logger.Infof(ctx, "no files left to download")
		return nil
	}

	tmpFile, err := os.CreateTemp(tmpDir, "srcs-file")
	if err != nil {
		return err
	}
	defer os.Remove(tmpFile.Name())

	// We don't write to the in this program, so we can close it.
	tmpFile.Close()

	if err := os.WriteFile(tmpFile.Name(), []byte(strings.Join(filesToDownload, "\n")), 0755); err != nil {
		return fmt.Errorf("failed to write srcs-file: %w", err)
	}

	if len(srcs) == 1 {
		logger.Infof(ctx, "downloading %d artifact to %s", len(srcs), dstDir)
	} else {
		logger.Infof(ctx, "downloading %d artifacts to %s", len(srcs), dstDir)
	}

	for _, srcFile := range srcs {
		logger.Infof(ctx, "  %s", srcFile)
	}

	args := []string{
		"cp",
		"-build", buildID,
		"-dst", dstDir,
		"-srcs-file", tmpFile.Name(),
	}

	if fromRoot {
		args = append(args, "-root")
	}

	// The `artifacts` utility can occasionally run into transient issues. This implements a retry policy
	// that attempts to avoid such issues causing flakes.
	eb := retry.NewExponentialBackoff(100*time.Millisecond, 10*time.Second, 2)
	// ~12 seconds to hit backoff ceiling; 2.5 minutes of slack (given the above EB)
	retryCap := uint64(22)
	return retry.Retry(ctx, retry.WithMaxAttempts(eb, retryCap), func() error {
		stdout, stderr, err := util.RunCommand(ctx, a.artifactsPath, args...)
		if len(stdout) != 0 {
			logger.Infof(ctx, "artifacts stdout:\n%s", stdout)
		}
		if len(stderr) != 0 {
			logger.Infof(ctx, "artifacts stderr:\n%s", stderr)
		}
		if err != nil {
			if len(stderr) != 0 {
				// Don't retry if the artifact we want to download does not exist.
				if bytes.Contains(stderr, []byte("nothing matched prefix")) || bytes.Contains(stderr, []byte("object doesn't exist")) {
					return retry.Fatal(os.ErrNotExist)
				}
				return fmt.Errorf("artifacts failed: %w: %s", err, string(stderr))
			}
			return fmt.Errorf("artifacts failed: %w", err)
		}

		return nil
	}, nil)
}

// removeDuplicates removes any duplicated items in the list.
func removeDuplicates(srcs []string) []string {
	var srcList []string
	seen := make(map[string]struct{})
	for _, src := range srcs {
		if _, ok := seen[src]; !ok {
			srcList = append(srcList, src)
			seen[src] = struct{}{}
		}
	}
	return srcList
}
