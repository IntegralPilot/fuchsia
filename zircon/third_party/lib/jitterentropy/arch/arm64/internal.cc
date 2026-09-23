// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "internal.h"

#include <platform/timer.h>

extern "C" {

bool jent_have_clock(void) { return true; }

void jent_get_nstime(uint64_t* out) { *out = current_mono_ticks(); }

}  // extern C
