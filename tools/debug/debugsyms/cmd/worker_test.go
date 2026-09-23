// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"fmt"
	"os"
	"reflect"
	"sync"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/tools/debug/elflib"
)

type mockContext struct {
	err error
}

func (ctx mockContext) Deadline() (time.Time, bool) {
	return time.Time{}, true
}

func (ctx mockContext) Done() <-chan struct{} {
	if ctx.err != nil {
		c := make(chan struct{}, 1)
		c <- struct{}{}
		var rc <-chan struct{} = c
		return rc
	}
	return nil
}

func (ctx mockContext) Err() error {
	return ctx.err
}

func (ctx mockContext) Value(key any) any {
	return ""
}

func mockWithReachableTimeout(parent context.Context, timeout time.Duration) (context.Context, context.CancelFunc) {
	return mockContext{err: context.DeadlineExceeded}, func() {}
}

func mockWithUnreachableTimeout(parent context.Context, timeout time.Duration) (context.Context, context.CancelFunc) {
	return mockContext{}, func() {}
}

func TestWorker(t *testing.T) {
	buildID := "foo"
	filename := fmt.Sprintf("%s.debug", buildID)
	tmpFile, err := os.CreateTemp("", filename)
	if err != nil {
		t.Fatalf("failed to create tempfile: %v", err)
	}
	timeout := 1 * time.Second
	defer os.Remove(tmpFile.Name())
	binaryFileRef := elflib.NewBinaryFileRef(tmpFile.Name(), buildID)
	ctx := context.Background()
	t.Run("run worker", func(t *testing.T) {
		jobs := make(chan job, 1)
		errs := make(chan error, 1)
		testJob := newZxdbJob(binaryFileRef)
		jobs <- testJob
		close(jobs)
		bkt := &mockBucket{
			contents: map[string]bool{"other.debug": true},
		}
		var wg sync.WaitGroup
		wg.Add(1)
		worker(ctx, bkt, &wg, mockWithUnreachableTimeout, timeout, jobs, errs)
		wg.Wait()
		close(errs)
		if err := <-errs; err != nil {
			t.Fatalf("expected nil error, got %v", err)
		}
	})
	t.Run("run worker with timeout", func(t *testing.T) {
		jobs := make(chan job, 1)
		errs := make(chan error, 1)
		testJob := newZxdbJob(binaryFileRef)
		jobs <- testJob
		close(jobs)
		bkt := &mockBucket{
			contents: map[string]bool{"other.debug": true},
		}
		var wg sync.WaitGroup
		wg.Add(1)
		worker(ctx, bkt, &wg, mockWithReachableTimeout, timeout, jobs, errs)
		wg.Wait()
		close(errs)
		err := <-errs
		if err == nil {
			t.Fatalf("expected error, got nil")
		}
		if _, ok := err.(jobTimeout); !ok {
			t.Fatalf("expected jobTimeout error, got %v", reflect.TypeOf(err))
		}
	})
}
