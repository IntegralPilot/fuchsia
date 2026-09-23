// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/boot-options/boot-options.h>
#include <lib/standalone-test/standalone.h>

#include "needs-next.h"

bool NeedsNextShouldSkip() {
  static bool next_vdso = standalone::GetBootOptions().always_use_next_vdso;
  return !next_vdso;
}
