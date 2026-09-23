// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

// Program to watch for a specific string to appear from a socket's output and
// then exits successfully.

import (
	"context"
	"flag"
	"fmt"
	"io"
	"math"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/lib/color"
	"go.fuchsia.dev/fuchsia/tools/lib/iomisc"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
	"go.fuchsia.dev/fuchsia/tools/testing/testrunner"
	testrunnerconstants "go.fuchsia.dev/fuchsia/tools/testing/testrunner/constants"
)

var (
	timeout        time.Duration
	successString  string
	failureString  string
	redirectStdout bool
)

func init() {
	flag.DurationVar(&timeout, "timeout", 10*time.Minute, "amount of time to wait for success string")
	flag.BoolVar(&redirectStdout, "stdout", false, "whether to redirect serial output to stdout")
	flag.StringVar(&successString, "success-str", "", "string that - if read - indicates success")
	flag.StringVar(&failureString, "failure-str", "", "string that - if read - indicates failure")
}

func execute(ctx context.Context, serialLogPath string, stdout io.Writer) error {
	ctx, cancel := context.WithTimeout(ctx, testrunner.ScaleTestTimeout(timeout))
	defer cancel()

	if successString == "" {
		flag.Usage()
		return fmt.Errorf("-success is a required argument")
	}
	if serialLogPath == "" {
		return fmt.Errorf("could not find serial log path in environment")
	}

	serialReader, err := os.Open(serialLogPath)
	if err != nil {
		return fmt.Errorf("failed to open serial log: %w", err)
	}
	logger.Debugf(ctx, "serial log: %s", serialLogPath)
	defer serialReader.Close()
	success := iomisc.NewMatcher([]byte(successString))
	failure := iomisc.NewMatcher([]byte(failureString))

	serialTee := io.TeeReader(serialReader, stdout)

	// Print out a log periodically to give an estimate of the timestamp at which
	// logs are getting read from the socket.
	tickerSecs := math.Min(30, timeout.Seconds()/2)
	ticker := time.NewTicker(time.Duration(tickerSecs) * time.Second)
	defer ticker.Stop()
	go func() {
		for range ticker.C {
			logger.Debugf(ctx, "still running test...")
		}
	}()

	b := make([]byte, 1024)
	n, err := 0, nil

ReadLoop:
	// Check that context hasn't been canceled first of all and then first thing after sleeping on EOF.
	for ctx.Err() == nil {
		n, err = serialTee.Read(b)

		switch {
		// Check that context hasn't been canceled first thing after Read() to
		// ensure that match doesn't succeed when the context was already canceled.
		case ctx.Err() != nil:
			break ReadLoop
		case success.Match(b[:n]):
			logger.Debugf(ctx, "success string found: %q", successString)
			return nil
		case failure.Match(b[:n]):
			return fmt.Errorf("failure string found: %q", failureString)
		case err == io.EOF:
			// The serial log is continuously being written to, so the io.Reader may
			// return an EOF if it has read everything that's been written so far. Keep
			// reading until the success/failure string matches or the timeout is hit.
			time.Sleep(100 * time.Millisecond)
		case err != nil:
			return fmt.Errorf("error trying to read from serial log: %w", err)
		}
	}
	// If we time out, it is helpful to see the last bytes processed.
	logger.Debugf(ctx, "Matcher(%q): last %d bytes read before cancellation: %q", successString, n, b[:n])
	return fmt.Errorf("timed out before success string %q was read from serial", successString)
}

func main() {
	flag.Parse()

	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer cancel()
	log := logger.NewLogger(logger.DebugLevel, color.NewColor(color.ColorAuto),
		os.Stdout, os.Stderr, "seriallistener ")
	ctx = logger.WithLogger(ctx, log)

	// Emulator serial is already wired up to stdout
	// TODO(https://fxbug.dev/42067738): Temporarily write serial output
	// to a file for debugging purposes.
	stdout := io.Discard
	if outDir := os.Getenv(testrunnerconstants.TestOutDirEnvKey); outDir != "" {
		if serialOutput, err := osmisc.CreateFile(filepath.Join(outDir, "serial_output")); err != nil {
			logger.Errorf(ctx, "%s", err)
		} else {
			stdout = serialOutput
			// Have the logger write to the file as well to get a
			// better sense of how much is read from the socket before
			// the socket io or ticker timeouts are reached.
			log := logger.NewLogger(logger.DebugLevel, color.NewColor(color.ColorAuto),
				io.MultiWriter(os.Stdout, serialOutput), io.MultiWriter(os.Stderr, serialOutput), "seriallistener ")
			ctx = logger.WithLogger(ctx, log)
			defer serialOutput.Close()
		}
	}
	deviceType := os.Getenv(constants.DeviceTypeEnvKey)
	if deviceType != "QEMU" && deviceType != "AEMU" && deviceType != "crosvm" {
		stdout = os.Stdout
	}

	serialLogPath := os.Getenv(constants.SerialLogEnvKey)
	if err := execute(ctx, serialLogPath, stdout); err != nil {
		logger.Fatalf(ctx, "%s", err)
	}
}
