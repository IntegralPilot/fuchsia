// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "reader.h"

void Reader(zx_instant_mono_ticks_t* shared_value, zx_instant_mono_ticks_t* read_value) {
  *read_value = *shared_value;
  // Issue a __builtin_debugtrap() to kick this thread out of restricted mode if the preceding read
  // succeeds.
  __builtin_debugtrap();
}
