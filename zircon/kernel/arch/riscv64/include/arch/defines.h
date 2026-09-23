// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_DEFINES_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_DEFINES_H_

// TODO(https://fxbug.dev/42146863): Use constants/#defines from libpage
// instead of PAGE_SIZE_SHIFT.
#define PAGE_SIZE_SHIFT (12)
#define USER_PAGE_SIZE_SHIFT PAGE_SIZE_SHIFT

#define USER_PAGE_SIZE (1UL << USER_PAGE_SIZE_SHIFT)
#define USER_PAGE_MASK (USER_PAGE_SIZE - 1)

// Zic64b guarantees.
#define MAX_CACHE_LINE 64

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_DEFINES_H_
