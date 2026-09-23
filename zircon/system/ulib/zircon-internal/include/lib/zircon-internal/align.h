// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZIRCON_INTERNAL_ALIGN_H_
#define LIB_ZIRCON_INTERNAL_ALIGN_H_

#include <stdint.h>

#define ZX_ROUNDUP(a, b)        \
  ({                            \
    const __typeof(a) _a = (a); \
    const __typeof(b) _b = (b); \
    ((_a + _b - 1) / _b * _b);  \
  })
#define ZX_ROUNDDOWN(a, b)      \
  ({                            \
    const __typeof(a) _a = (a); \
    const __typeof(b) _b = (b); \
    _a - (_a % _b);             \
  })
#define ZX_ALIGN(a, b) ZX_ROUNDUP(a, b)
#define ZX_IS_ALIGNED(a, b) (!(((uintptr_t)(a)) & (((uintptr_t)(b)) - 1)))

#endif  // LIB_ZIRCON_INTERNAL_ALIGN_H_
