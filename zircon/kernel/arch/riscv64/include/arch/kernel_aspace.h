// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_KERNEL_ASPACE_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_KERNEL_ASPACE_H_

// NOTE: This is an independent header so it can be #include'd by a userland
// test program as well as the kernel source (both C++ and assembly).

// Virtual address where the kernel address space begins.
// Below this is the user address space.
// riscv64 with sv39 means a page-based 39-bit virtual memory space.  The
// base kernel address is chosen so that kernel addresses have a 1 in the
// most significant bit whereas user addresses have a 0.
#define KERNEL_ASPACE_BASE 0xffffffc000000000UL
#define KERNEL_ASPACE_SIZE (1UL << 38)

// Virtual address where the user-accessible address space begins.
// Below this is wholly inaccessible.
#define USER_ASPACE_BASE 0x0000000000200000UL
#define USER_ASPACE_SIZE ((1UL << 38) - USER_ASPACE_BASE)

// Size of the restricted mode address space in unified address spaces.
// We set the top of the restricted aspace to exactly halfway through the top
// level page table.
#define USER_RESTRICTED_ASPACE_SIZE ((1UL << 37) - USER_ASPACE_BASE)

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_KERNEL_ASPACE_H_
