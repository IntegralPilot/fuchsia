// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package orchestrate

import (
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"

	utils "go.fuchsia.dev/fuchsia/tools/orchestrate/utils"
)

var (
	serialLog    = filepath.Join(utils.GetOutputsDir(), "serial.log")
	serialSymLog = filepath.Join(utils.GetOutputsDir(), "serial.symbolized.log")
)

// SerialLogging holds information about logging the serial from the device.
type SerialLogging struct {
	socket net.Conn
}

// StartSerialLogging captures serial logging into a file in the background.
func StartSerialLogging(deviceConfig *DeviceConfig) (*SerialLogging, error) {
	socket, err := net.Dial("unix", deviceConfig.SerialMux)
	if err != nil {
		return nil, fmt.Errorf("net.Dial: %w", err)
	}
	dst, err := os.Create(serialLog)
	if err != nil {
		return nil, fmt.Errorf("os.Create: %w", err)
	}
	go func() {
		if _, err := io.Copy(dst, socket); err != nil {
			fmt.Printf("serial io.Copy failed: %v\n", err)
		}
		if err := dst.Close(); err != nil {
			fmt.Printf("serial conn.Close failed: %v\n", err)
		}
	}()
	return &SerialLogging{socket: socket}, nil
}

// Symbolize the log file.
func (s *SerialLogging) Symbolize(runner *TestOrchestrator) {
	if err := runner.Symbolize(serialLog, serialSymLog); err != nil {
		fmt.Printf("serial runner.Symbolize: %v\n", err)
	}
}

// Stop stops the goroutine listening for logs.
func (s *SerialLogging) Stop() {
	if err := s.socket.Close(); err != nil {
		fmt.Printf("serial conn.Close: %v\n", err)
	}
}
