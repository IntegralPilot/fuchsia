// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import "testing"

func TestWrapAdd(t *testing.T) {
	if got := wrapAdd(1, 2); got != 3 {
		t.Errorf("wrapAdd(1, 2) = %d; want 3", got)
	}
}
