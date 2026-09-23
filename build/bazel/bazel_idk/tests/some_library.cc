// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dependency.h>
#include <lib/test_lib/test_header.h>
#include <zircon/availability.h>

#if defined(__Fuchsia__) && FUCHSIA_API_LEVEL_AT_LEAST(NEXT)  // NEVER_REPLACE_NEXT
__attribute__((__visibility__("default")))
#endif
int kAvailableFromApiLevelNext = 42;

__attribute__((__visibility__("default"))) int SomeFunction() {
  return RequiredFunction(kAvailableFromApiLevelNext);
}
