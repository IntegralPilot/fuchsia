// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/test_lib/platform_dependent_header.h>
#include <lib/test_lib/test_header.h>

#ifdef __Fuchsia__
#include <lib/test_lib/fuchsia_only_header.h>
#endif

__attribute__((__visibility__("default"))) int SomeFunction() {
  int result =
#ifdef __Fuchsia__
      FuchsiaSpecificFunction() +
#endif
      PlatformSpecificFunction() + GetPlatformSpecificValue();

  return result;
}
