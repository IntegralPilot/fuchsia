// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"bytes"
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"syscall"

	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
	"go.fuchsia.dev/fuchsia/tools/mobly_test_parser"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
	"go.fuchsia.dev/fuchsia/tools/testing/testrunner/constants"
)

func usage() {
	fmt.Printf(`mobly_test_parser [mobly test command]

Reads stdout from the mobly test command, and writes a JSON formatted summary to stdout
of any error messages parsed from the logs.
`)
}

func mainImpl() (int, error) {
	flag.Usage = usage

	// Parse any global flags (e.g. those for glog)
	flag.Parse()

	args := flag.Args()

	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer cancel()

	var testErr error
	var retCode int
	stdoutForParsing := new(bytes.Buffer)
	testStdout := io.MultiWriter(os.Stdout, stdoutForParsing)
	r := &subprocess.Runner{Env: os.Environ()}
	fmt.Fprintf(os.Stdout, "Running %s\n", args[0])
	if err := r.Run(ctx, args, subprocess.RunOptions{Stdout: testStdout}); err != nil {
		var exitErr *exec.ExitError
		if errors.As(err, &exitErr) {
			retCode = exitErr.ExitCode()
		} else {
			retCode = 1
		}
		testErr = fmt.Errorf("Error running test: %w", err)
	}

	if outputSummaryPath := os.Getenv(constants.TestOutputSummaryPathEnvKey); outputSummaryPath != "" {
		cases := mobly_test_parser.Parse(stdoutForParsing.Bytes())
		result := runtests.TestResult{
			Cases: cases,
		}
		if err := jsonutil.WriteToFile(outputSummaryPath, result); err != nil {
			return retCode, fmt.Errorf("Error writing output: %w", err)
		}
	}
	return retCode, testErr
}

func main() {
	if retCode, err := mainImpl(); err != nil {
		fmt.Fprintf(os.Stderr, "%s\n", err)
		os.Exit(retCode)
	}
}
