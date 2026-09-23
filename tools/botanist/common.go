// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package botanist

import (
	"bytes"
	"context"
	"errors"
	"io"
	"math"
	"time"

	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/streams"
)

// Experiments is a map containing a set of experiments to check for.
type Experiments map[string]struct{}

func GetExperiments(experiments []string) Experiments {
	expMap := make(map[string]struct{})
	for _, exp := range experiments {
		expMap[exp] = struct{}{}
	}
	return expMap
}

func (e Experiments) Contains(experiment Experiment) bool {
	_, ok := e[string(experiment)]
	return ok
}

// Experiment represents a supported botanist experiment.
type Experiment string

const (
	UseFFXTestParallel Experiment = "use_ffx_test_parallel"
	UseFFXMonitor      Experiment = "use_ffx_monitor"
	ForceFFXUSB        Experiment = "force_ffx_usb"
	UseFFXRepository   Experiment = "use_ffx_repository"
)

var SupportedExperiments = []Experiment{UseFFXTestParallel, UseFFXMonitor, ForceFFXUSB, UseFFXRepository}

// GetLoggerCtx returns a new context with the logger of the provided ctx.
func GetLoggerCtx(ctx context.Context) context.Context {
	return logger.WithLogger(context.Background(), logger.LoggerFromContext(ctx))
}

// WaitForProcess launches a long-running process in the background and returns a cleanup
// function to cancel the context and wait for the process to finish.
func WaitForProcess(ctx context.Context, process func(context.Context) error, processName string) func() {
	// Use a new context so that the subprocess can only be terminated by
	// a direct call to the cancel function.
	processCtx, cancel := context.WithCancel(GetLoggerCtx(ctx))
	cmdWait := make(chan error)
	go func() {
		err := process(processCtx)
		if err != nil && !errors.Is(err, context.Canceled) {
			logger.Errorf(ctx, "%s process finished with err: %s", processName, err)
		} else {
			logger.Debugf(ctx, "%s process finished", processName)
		}
		close(cmdWait)
	}()
	cleanup := func() {
		cancel()
		<-cmdWait
	}
	return cleanup
}

// LockedWriter wraps an [io.Writer] so that only one Write happens at a time.
type LockedWriter struct {
	c chan struct{}
	w io.Writer
}

// NewLockedWriter creates a LockedWriter.
func NewLockedWriter(w io.Writer) *LockedWriter {
	lw := &LockedWriter{
		c: make(chan struct{}, 1),
		w: w,
	}
	lw.c <- struct{}{}
	return lw
}

func (lw *LockedWriter) Write(data []byte) (int, error) {
	<-lw.c
	defer func() { lw.c <- struct{}{} }()
	return lw.w.Write(data)
}

// LineWriter is a wrapper around a writer that writes line by line so
// that multiple writers to the same underlying writer won't interleave
// their writes midline.
type LineWriter struct {
	writer io.Writer
	line   []byte
	prefix string
}

// NewLineWriter returns a new LineWriter.
func NewLineWriter(writer io.Writer, prefix string) *LineWriter {
	return &LineWriter{
		writer: writer,
		prefix: prefix,
	}
}

// Write stores bytes until it gets a newline and then writes to the underlying
// writer line by line. If the underlying Write() returns an err, this writer
// will return the number of bytes of the current data that were written.
// Otherwise, it returns the full length of the data to notify callers that it
// has received the whole data.
func (w *LineWriter) Write(data []byte) (int, error) {
	lines := bytes.SplitAfter(data, []byte("\n"))
	written := 0
	for _, line := range lines {
		if !bytes.HasSuffix(line, []byte("\n")) {
			w.line = append(w.line, line...)
			written += len(line)
			continue
		}

		toWrite := []byte{}
		if w.prefix != "" {
			toWrite = append(toWrite, []byte(w.prefix+": ")...)
		}
		toWrite = append(toWrite, w.line...)
		fullLine := append(toWrite, line...)

		// b/532619063: Filter out Goldfish out-of-bound warning spam from AEMU to prevent overflowing
		// and truncating the 100MB Swarming artifact log buffer. Once the upstream QEMU patch from
		// b/407710119 is included in prebuilt AEMU/QEMU binaries rolled into Fuchsia, we can get rid of
		// this override
		if bytes.Contains(fullLine, []byte("goldfish_address_space_area is out-of-bound")) {
			written += len(line)
			w.line = []byte{}
			continue
		}

		n, err := w.writer.Write(fullLine)
		written += int(math.Max(0, float64(n-len(toWrite))))
		if err != nil {
			return written, err
		}

		w.line = []byte{}
	}
	return len(data), nil
}

// TimestampWriter is a wrapper around a writer that prepends its writes
// with the current host timestamp. This will allow all botanist logs
// (kernel/serial/syslog/test) to be reliably lined up when reading them.
type TimestampWriter struct {
	writer io.Writer
	format string
}

func NewTimestampWriter(writer io.Writer) *TimestampWriter {
	return &TimestampWriter{
		writer: writer,
		format: "2006-01-02 15:04:05.000000 ",
	}
}

func (w *TimestampWriter) Write(data []byte) (int, error) {
	if n, err := w.writer.Write([]byte(time.Now().Format(w.format))); err != nil {
		return n, err
	}
	return w.writer.Write(data)
}

// NewStiodWriters returns a new LineWriter for the stdout and stderr associated
// with the provided context. It also returns a function to flush out any
// remaining data not written by Write because it didn't end with a newline.
func NewStdioWriters(ctx context.Context, id string) (io.Writer, io.Writer, func()) {
	stdoutWriter := NewLineWriter(streams.Stdout(ctx), id)
	stderrWriter := NewLineWriter(streams.Stderr(ctx), id)
	flush := func() {
		// Flush out the rest of the data stored by the writers.
		if len(stdoutWriter.line) > 0 {
			if _, err := stdoutWriter.Write([]byte("\n")); err != nil {
				logger.Debugf(ctx, "failed to flush out data to stdout %q: %s", string(stdoutWriter.line), err)
			}
		}
		if len(stderrWriter.line) > 0 {
			if _, err := stderrWriter.Write([]byte("\n")); err != nil {
				logger.Debugf(ctx, "failed to flush out data to stderr %q: %s", string(stderrWriter.line), err)
			}
		}
	}
	return stdoutWriter, stderrWriter, flush
}
