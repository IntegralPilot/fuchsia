// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_DEFINES_H_
#define ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_DEFINES_H_

#define SHIFT_4K (12)
#define SHIFT_16K (14)
#define SHIFT_64K (16)

// TODO(https://fxbug.dev/42146863): Use constants/#defines from libpage
// instead of PAGE_SIZE_SHIFT.
#ifdef ARM64_LARGE_PAGESIZE_64K
#define PAGE_SIZE_SHIFT (SHIFT_64K)
#elif ARM64_LARGE_PAGESIZE_16K
#define PAGE_SIZE_SHIFT (SHIFT_16K)
#else
#define PAGE_SIZE_SHIFT (SHIFT_4K)
#endif
#define USER_PAGE_SIZE_SHIFT SHIFT_4K

#define USER_PAGE_SIZE (1UL << USER_PAGE_SIZE_SHIFT)
#define USER_PAGE_MASK (USER_PAGE_SIZE - 1)

// The maximum cache line seen on any known ARM hardware.
#define MAX_CACHE_LINE 64

#endif  // ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_DEFINES_H_
