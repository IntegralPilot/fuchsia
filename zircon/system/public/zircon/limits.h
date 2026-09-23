// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_LIMITS_H_
#define ZIRCON_LIMITS_H_

#if defined(__x86_64__) || defined(__i386__)

#define ZX_MIN_PAGE_SHIFT (12u)
#define ZX_MAX_PAGE_SHIFT (21u)

#elif defined(__aarch64__) || defined(__arm__)

#define ZX_MIN_PAGE_SHIFT (12u)
#define ZX_MAX_PAGE_SHIFT (16u)

#elif defined(__riscv)

#define ZX_MIN_PAGE_SHIFT (12u)
#define ZX_MAX_PAGE_SHIFT (21u)

#else

#error what architecture?

#endif

#define ZX_MIN_PAGE_SIZE (1u << ZX_MIN_PAGE_SHIFT)
#define ZX_MAX_PAGE_SIZE (1u << ZX_MAX_PAGE_SHIFT)

#endif  // ZIRCON_LIMITS_H_
