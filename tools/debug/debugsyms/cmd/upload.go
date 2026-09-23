// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"fmt"
	"sync"
	"time"

	"go.fuchsia.dev/fuchsia/tools/debug/elflib"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"

	"github.com/google/subcommands"
)

type uploadCommand struct {
	// GCS bucket to upload symbols to.
	bucket string

	// Timeout duration per upload.
	timeout time.Duration

	// Number of concurrent uploading routines.
	j int
}

func (uploadCommand) Name() string {
	return "upload"
}

func (uploadCommand) Synopsis() string {
	return "upload debug symbols from one or more .build-id directories to GCS"
}

func (uploadCommand) Usage() string {
	return `
upload -bucket $GCS_BUCKET [-timeout $TIMEOUT_SECS] $BUILD_ID_DIR1 [$BUILD_ID_DIR2 ...]
`
}

func (cmd *uploadCommand) SetFlags(f *flag.FlagSet) {
	f.StringVar(&cmd.bucket, "bucket", "", "GCS bucket to upload symbols to")
	f.DurationVar(&cmd.timeout, "timeout", 20*time.Minute, "timeout duration per upload")
	f.IntVar(&cmd.j, "j", 500, "number of concurrent uploading routines")
}

func (cmd uploadCommand) Execute(ctx context.Context, f *flag.FlagSet, _ ...any) subcommands.ExitStatus {
	args := f.Args()
	if len(args) == 0 {
		logger.Errorf(ctx, "one or more positional args expected: paths to .build-id dirs")
		return subcommands.ExitUsageError
	}
	if cmd.bucket == "" {
		logger.Errorf(ctx, "-bucket is required")
		return subcommands.ExitUsageError
	}
	if err := cmd.execute(ctx, args); err != nil {
		logger.Errorf(ctx, "%v", err)
		return subcommands.ExitFailure
	}
	return subcommands.ExitSuccess
}

func (cmd uploadCommand) execute(ctx context.Context, dirs []string) error {
	bfrs, err := collect(dirs)
	if err != nil {
		return fmt.Errorf("failed to collect .debug files: %v", err)
	}
	bfrs, err = prune(ctx, bfrs)
	if err != nil {
		return fmt.Errorf("pruning .debug files failed: %v", err)
	}
	if !verify(ctx, bfrs) {
		return fmt.Errorf("verification failed for .debug files")
	}
	jobs, err := queue(bfrs)
	if err != nil {
		return fmt.Errorf("failed to queue jobs: %v", err)
	}
	bkt, err := newGCSBucket(ctx, cmd.bucket)
	if err != nil {
		return err
	}
	succeeded := upload(ctx, bkt, cmd.timeout, cmd.j, jobs)
	if !succeeded {
		return fmt.Errorf("completed with errors")
	}
	return nil
}

// Returns BinaryFileRefs for each unique .debug file in dirs.
func collect(dirs []string) ([]elflib.BinaryFileRef, error) {
	var out []elflib.BinaryFileRef
	buildIDSet := map[string]bool{}
	for _, dir := range dirs {
		refs, err := elflib.WalkBuildIDDir(dir)
		if err != nil {
			return nil, err
		}
		for _, ref := range refs {
			if _, ok := buildIDSet[ref.BuildID]; ok {
				continue
			}
			buildIDSet[ref.BuildID] = true
			out = append(out, ref)
		}
	}
	return out, nil
}

// Removes files from the collected set of BinaryFileRefs that but might not have a debug_info but
// should not cause errors in verify.
func prune(ctx context.Context, bfrs []elflib.BinaryFileRef) ([]elflib.BinaryFileRef, error) {
	var out []elflib.BinaryFileRef
	for _, bfr := range bfrs {
		hasDebugInfo, err := bfr.HasDebugInfo()
		if err != nil {
			logger.Errorf(ctx, "cannot read %s: %v", bfr.Filepath, err)
			return nil, err
		}
		if !hasDebugInfo {
			hasDex, err := bfr.HasDex()
			if err != nil {
				logger.Errorf(ctx, "could not determine whether %s has a dex section: %v", bfr.Filepath, err)
				return nil, err
			}
			if !hasDex {
				// Found a file that doesn't have a debug_info section or a dex
				// section, output a warning and continue. This was formerly a hard
				// error, but after https://fxbug.dev/342649089, it has been
				// downgraded to a warning.
				// TODO(https://fxbug.dev/342649089): Evaluate if this should return
				// to blocking status or not.
				logger.Warningf(ctx, "%s missing .debug_info and .dex sections", bfr.Filepath)
			}

			// Fallthrough if the file has a dex section but no debug_info section.
			// These files are technically valid, but will cause issues in downstream
			// pipelines that assume all of the files in a debug package have debug_info
			// sections, so do not upload these files but also don't fail in the verify
			// step when the expectation is that they have a debug_info section.
		} else {
			// This one has a debug_info section, add it.
			out = append(out, bfr)
		}
	}
	return out, nil
}

// Returns true iff all bfrs are valid debug binaries.
func verify(ctx context.Context, bfrs []elflib.BinaryFileRef) bool {
	succeeded := true
	for _, bfr := range bfrs {
		if err := bfr.Verify(); err != nil {
			logger.Errorf(ctx, "verification failed for %s: %v", bfr.Filepath, err)
			succeeded = false
		}
	}
	return succeeded
}

// Returns a read-only channel of jobs to upload each file referenced in bfrs.
func queue(bfrs []elflib.BinaryFileRef) (<-chan job, error) {
	var jobs []job
	for _, bfr := range bfrs {
		jobs = append(jobs, newZxdbJob(bfr), newDebuginfodJob(bfr))
	}

	c := make(chan job, len(jobs))
	for _, j := range jobs {
		c <- j
	}
	close(c)
	return c, nil
}

// Upload executes all of the jobs to upload files from the input channel. Returns true
// iff all uploads succeeded without error.
func upload(ctx context.Context, bkt *GCSBucket, timeout time.Duration, j int, jobs <-chan job) bool {
	errs := make(chan error, j)
	defer close(errs)

	// Spawn workers to execute the uploads.
	var wg sync.WaitGroup
	wg.Add(j)
	for i := 0; i < j; i++ {
		go worker(ctx, bkt, &wg, context.WithTimeout, timeout, jobs, errs)
	}

	// Let the caller know whether any errors were emitted.
	succeeded := true
	go func() {
		for e := range errs {
			if e != nil {
				succeeded = false
				logger.Errorf(ctx, "%v", e)
			}
		}
	}()
	wg.Wait()
	return succeeded
}
