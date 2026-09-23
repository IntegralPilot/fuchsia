// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//go:build linux

package serial

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"testing"

	"github.com/creack/pty"
	"golang.org/x/sys/unix"
)

func openPTY(t *testing.T) (*os.File, *os.File) {
	t.Helper()
	primary, secondary, err := pty.Open()
	if err != nil {
		if errors.Is(err, os.ErrNotExist) || errors.Is(err, unix.ENOENT) || errors.Is(err, unix.EACCES) || errors.Is(err, unix.EPERM) || errors.Is(err, unix.ENXIO) {
			t.Skipf("skipping test: pty unavailable in environment: %v", err)
		}
		t.Fatalf("pty.Open() failed: %v", err)
	}
	return primary, secondary
}

func TestOpenBaudRate(t *testing.T) {
	testCases := []struct {
		baudRate int
		wantRate uint32
	}{
		{baudRate: 9600, wantRate: unix.B9600},
		{baudRate: 19200, wantRate: unix.B19200},
		{baudRate: 38400, wantRate: unix.B38400},
		{baudRate: 57600, wantRate: unix.B57600},
		{baudRate: 115200, wantRate: unix.B115200},
		{baudRate: 230400, wantRate: unix.B230400},
		{baudRate: 460800, wantRate: unix.B460800},
		{baudRate: 921600, wantRate: unix.B921600},
		{baudRate: 1000000, wantRate: unix.B1000000},
		{baudRate: 1500000, wantRate: unix.B1500000},
		{baudRate: 2000000, wantRate: unix.B2000000},
	}

	for _, tc := range testCases {
		t.Run(fmt.Sprintf("%d", tc.baudRate), func(t *testing.T) {
			primary, secondary := openPTY(t)
			defer primary.Close()
			defer secondary.Close()

			port, err := OpenWithOptions(secondary.Name(), tc.baudRate)
			if err != nil {
				t.Fatalf("OpenWithOptions(%q, %d) failed: %v", secondary.Name(), tc.baudRate, err)
			}
			defer port.Close()

			termios, err := unix.IoctlGetTermios(int(secondary.Fd()), unix.TCGETS)
			if err != nil {
				t.Fatalf("unix.IoctlGetTermios failed: %v", err)
			}

			if got := termios.Cflag & unix.CBAUD; got != tc.wantRate {
				t.Errorf("Cflag CBAUD mismatch: got %#x, want %#x", got, tc.wantRate)
			}
			if termios.Cflag&unix.CS8 == 0 {
				t.Errorf("expected CS8 to be set in Cflag: %#x", termios.Cflag)
			}
			if termios.Cflag&unix.CREAD == 0 {
				t.Errorf("expected CREAD to be set in Cflag: %#x", termios.Cflag)
			}
			if termios.Cflag&unix.CLOCAL == 0 {
				t.Errorf("expected CLOCAL to be set in Cflag: %#x", termios.Cflag)
			}
			if termios.Cflag&unix.PARENB != 0 {
				t.Errorf("expected PARENB not to be set in Cflag: %#x", termios.Cflag)
			}
			if termios.Cflag&unix.CSTOPB != 0 {
				t.Errorf("expected CSTOPB not to be set in Cflag: %#x", termios.Cflag)
			}
		})
	}
}

func TestOpenUnsupportedBaudRate(t *testing.T) {
	if _, err := OpenWithOptions("/dev/null", 12345); err == nil {
		t.Errorf("expected error for unsupported baud rate 12345, got nil")
	}
}

func TestOpenReadWrite(t *testing.T) {
	primary, secondary := openPTY(t)
	defer primary.Close()
	defer secondary.Close()

	port, err := Open(secondary.Name())
	if err != nil {
		t.Fatalf("Open(%q) failed: %v", secondary.Name(), err)
	}
	defer port.Close()

	// Write to primary, read from port
	msg := []byte("hello serial\n")
	if _, err := primary.Write(msg); err != nil {
		t.Fatalf("primary.Write() failed: %v", err)
	}

	buf := make([]byte, len(msg))
	if _, err := port.Read(buf); err != nil {
		t.Fatalf("port.Read() failed: %v", err)
	}
	if !bytes.Equal(buf, msg) {
		t.Errorf("read mismatch: got %q, want %q", string(buf), string(msg))
	}
}
