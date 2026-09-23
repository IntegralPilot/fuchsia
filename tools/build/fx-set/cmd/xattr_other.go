// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//go:build !linux

package main

import "errors"

// probeXattrSupport checks if extended attributes are supported in the given directory.
//
// Returns (false, nil) iff extended attributes are verifiably unsupported and (true, nil) if they
// are verifiably supported. Returns (false, err) if it cannot be determined whether extended
// attributes are supported.
//
// Note: This test is sensitive to where 'dir' is mounted, as not all filesystems or mount options
// support extended attributes.
func probeXattrSupport(dir string) (bool, error) {
	return false, errors.New("probeXattrSupport not implemented for non-Linux platforms")
}
