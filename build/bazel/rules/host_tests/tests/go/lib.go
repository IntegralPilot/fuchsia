// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// lib is a example library for demonstrating Bazel host Go tests in Fuchsia.
package lib

// Add adds two integers and returns the sum.
func Add(a, b int) int {
	return a + b
}
