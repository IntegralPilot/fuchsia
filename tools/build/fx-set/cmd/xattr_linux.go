// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//go:build linux

package main

import (
	"os"
	"syscall"
)

// probeXattrSupport checks if extended attributes are supported in the given directory.
//
// Returns (false, nil) iff extended attributes are verifiably unsupported and (true, nil) if they
// are verifiably supported. Returns (false, err) if it cannot be determined whether extended
// attributes are supported.
//
// Note: This test is sensitive to where 'dir' is mounted, as not all filesystems or mount options
// support extended attributes.
func probeXattrSupport(dir string) (bool, error) {
	if err := os.MkdirAll(dir, 0755); err != nil {
		return false, err
	}
	f, err := os.CreateTemp(dir, ".xattr_probe")
	if err != nil {
		return false, err
	}
	defer os.Remove(f.Name())
	defer f.Close()

	err = syscall.Setxattr(f.Name(), "user.ping", []byte("pong"), 0)
	if err == syscall.ENOTSUP || err == syscall.EOPNOTSUPP {
		return false, nil
	}
	return err == nil, err
}
