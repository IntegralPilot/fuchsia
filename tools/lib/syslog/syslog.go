// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package syslog

import (
	"context"
	"fmt"
	"io"
	"time"

	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
	"go.fuchsia.dev/fuchsia/tools/lib/syslog/constants"
	"go.fuchsia.dev/fuchsia/tools/net/sshutil"
)

const (
	// The program on fuchsia used to stream system logs through a shell, not to
	// be confused with the zircon host tool "loglistener" (no underscore) used
	// to stream zircon-level logs to host.
	LogListener = "/bin/log_listener"

	// Time to wait between attempts to reconnect after losing the connection.
	defaultReconnectInterval = 5 * time.Second
)

// Syslogger streams systems logs from a Fuchsia instance.
type Syslogger struct {
	client  sshClient
	ffx     *ffxutil.FFXInstance
	running bool
}

type sshClient interface {
	Run(context.Context, []string, io.Writer, io.Writer) error
	ReconnectWithBackoff(context.Context, retry.Backoff) error
}

func LogListenerWithArgs(args ...string) []string {
	return append([]string{LogListener, "--no-color"}, args...)
}

// NewSyslogger creates a new Syslogger, given an SSH session with a Fuchsia instance.
func NewSyslogger(client *sshutil.Client) *Syslogger {
	return &Syslogger{
		client: client,
	}
}

// NewFFXSyslogger creates a new Syslogger which will use the provided ffx instance
// to capture syslogs.
func NewFFXSyslogger(ffx *ffxutil.FFXInstance) *Syslogger {
	return &Syslogger{
		ffx: ffx,
	}
}

// IsRunning returns whether the syslog is still streaming. Once the goroutine
// started in Stream() completes, this should return false.
func (s *Syslogger) IsRunning() bool {
	return s.running
}

// Stream writes system logs to a given writer, starting from the very beginning
// of the system's uptime. It keeps running until the context is canceled or until an
// unexpected (not SSH-related) error occurs; otherwise, it returns errors to
// the returned channel to signify the syslog was interrupted and restarted.
func (s *Syslogger) Stream(ctx, loggerCtx context.Context, output io.Writer) <-chan error {
	errs := make(chan error, 1)
	sendErr := func(errs chan error, err error) {
		// Wait a bit for any listeners to clear the channel first.
		time.Sleep(500 * time.Millisecond)
		select {
		case <-errs:
			// Clear the channel if nobody is listening.
		default:
			// Break out if channel is empty.
		}
		errs <- err
	}
	var cmd []string
	if s.ffx == nil {
		cmd = LogListenerWithArgs()
	}

	ffxLogAdditionalArgs := []string{}
	s.running = true
	go func() {
		for {
			var err error
			if s.ffx != nil {
				args := append([]string{"--no-color", "--symbolize", "off"}, ffxLogAdditionalArgs...)
				err = s.ffx.Log(ctx, &output, args...)
			} else if s.client != nil {
				// Note: Fuchsia's log_listener does not write to stderr.
				err = s.client.Run(ctx, cmd, output, nil)
			} else {
				err = fmt.Errorf("syslogger requires an SSH client or ffx instance to run")
				logger.Errorf(loggerCtx, "error streaming syslog: %s", err)
				s.running = false
				sendErr(errs, err)
				close(errs)
				return
			}
			if ctx.Err() == nil {
				if err != nil {
					logger.Debugf(loggerCtx, "error streaming syslog: %s", err)
				} else {
					logger.Debugf(loggerCtx, "log_listener exited successfully, will rerun")
					// Don't stream from the beginning of the system's uptime, since
					// that would include logs that we've already streamed.
					if s.ffx != nil {
						ffxLogAdditionalArgs = []string{"--since", "now"}
					} else {
						cmd = LogListenerWithArgs("--since_now", "yes")
					}
					continue
				}
			}

			// We need not attempt to reconnect if we hit an error unrelated to the
			// connection (which is probably unrecoverable) or if the context was
			// canceled (context cancellation is the only mechanism for stopping
			// this method, so it generally indicates that the caller is exiting
			// normally).
			if ctx.Err() != nil || (s.client != nil && !sshutil.IsConnectionError(err)) {
				logger.Debugf(loggerCtx, "syslog streaming complete: %s", err)
				s.running = false
				sendErr(errs, err)
				close(errs)
				return
			}

			logger.Errorf(loggerCtx, "syslog: SSH client unresponsive; will attempt to reconnect and continue streaming: %s", err)
			var reconnectErr error
			if s.ffx != nil {
				reconnectErr = s.ffx.TargetWait(ctx, "-t", "90")
			} else {
				reconnectErr = s.client.ReconnectWithBackoff(ctx, retry.NewConstantBackoff(defaultReconnectInterval))
			}
			if reconnectErr != nil {
				// The context probably got cancelled before we were able to
				// reconnect.
				if ctx.Err() != nil {
					logger.Errorf(loggerCtx, "syslog: %s: %s", constants.CtxReconnectError, ctx.Err())
				}
				s.running = false
				sendErr(errs, err)
				close(errs)
				return
			}

			// Start streaming from the beginning of the system's uptime again now that
			// we're rebooting.
			if s.ffx != nil {
				ffxLogAdditionalArgs = []string{}
			} else {
				cmd = LogListenerWithArgs()
			}
			logger.Infof(loggerCtx, "syslog: refreshed ssh connection")
			io.WriteString(output, "\n\n<< SYSLOG STREAM INTERRUPTED; RECONNECTING NOW >>\n\n")
			sendErr(errs, err)
		}
	}()
	return errs
}
