// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

extern int fx_cc_library_for_test(void) {
#if defined(IN_FX_CC_LIBRARY)
  return 42;
#else   // !IN_FX_CC_LIBRARY
  return -1;
#endif  // !IN_FX_CC_LIBRARY
}
