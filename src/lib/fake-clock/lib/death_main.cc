// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/utc.h>

int main() {
  // Expected to crash when the binary is instrumented with fake-clock without
  // any special affordances.
  zx_utc_reference_get();
}
