// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <inttypes.h>
#include <lib/affine/ratio.h>

extern "C" __attribute__((visibility("default"))) uint64_t scale_ratio(uint32_t *a, uint32_t *b,
                                                                       uint64_t *scale) {
  affine::Ratio a_b_ratio(*a, *b);
  return a_b_ratio.Scale(*scale);
}
