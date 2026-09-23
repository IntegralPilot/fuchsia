// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"

	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

type jobTimeout struct {
	job     job
	timeout time.Duration
}

func (t jobTimeout) Error() string {
	return fmt.Sprintf("job %s timed out after %.1fs", t.job.name, t.timeout.Seconds())
}

type withTimeoutFunc func(context.Context, time.Duration) (context.Context, context.CancelFunc)

// Worker processes all jobs on the input channel, emitting any errors on errs.
func worker(ctx context.Context, bkt bucket, wg *sync.WaitGroup, withTimeout withTimeoutFunc, timeout time.Duration, jobs <-chan job, errs chan<- error) {
	defer wg.Done()
	for job := range jobs {
		ctx, cancel := withTimeout(ctx, timeout)
		defer cancel()
		jobErrs := make(chan error)
		logger.Debugf(ctx, "executing %s", job.name)
		go func() {
			jobErrs <- job.ensure(ctx, bkt)
		}()
		select {
		case err := <-jobErrs:
			if err != nil {
				errs <- fmt.Errorf("job %s failed: %v", job.name, err)
			}
		case <-ctx.Done():
			if errors.Is(ctx.Err(), context.DeadlineExceeded) {
				errs <- jobTimeout{job: job, timeout: timeout}
			} else {
				errs <- fmt.Errorf("job %s was canceled", job.name)
			}
		}
	}
}
