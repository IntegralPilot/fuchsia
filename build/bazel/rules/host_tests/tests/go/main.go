// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// main is a example binary for demonstrating Bazel host Go tests in Fuchsia.
package main

import (
	"fmt"

	"go.fuchsia.dev/fuchsia/build/bazel/host_tests/go_tests/lib"
)

// wrapAdd is a wrapper around lib.Add for demonstration purposes.
func wrapAdd(a, b int) int {
	return lib.Add(a, b)
}

func main() {
	fmt.Println("1 + 2 = ", wrapAdd(1, 2))
}
