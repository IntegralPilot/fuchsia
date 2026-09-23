// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_VM_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_VM_H_

#include <lib/page/size.h>
#include <stdint.h>
#include <sys/types.h>
#include <zircon/compiler.h>

#include <arch/kernel_aspace.h>
#include <arch/riscv64/mmu.h>

constexpr bool is_kernel_address(vaddr_t va) {
  return (va >= (vaddr_t)KERNEL_ASPACE_BASE &&
          va - (vaddr_t)KERNEL_ASPACE_BASE < (vaddr_t)KERNEL_ASPACE_SIZE);
}

constexpr uint8_t kRiscv64VaddrBits = kVirtualAddressSize;
// Canonical addresses (to use an x86 term) are addresses where the top bits
// from 63 down to (kRiscv64VaddrBits-1) are all either 0 or 1.
// This means user area is [ 0 ... (1<<(kRiscv64VAddrBits-1)) )
constexpr uint64_t kRiscv64CanonicalAddressMask = ~((1UL << (kRiscv64VaddrBits - 1)) - 1UL);
static_assert(kRiscv64CanonicalAddressMask == 0xffffffc000000000);

// Assert that the kernel aspace #defines lines up with this notion, since it'll
// start at the first available address in the kernel half of the address space and
// extend to the highest 64bit value.
static_assert(kRiscv64CanonicalAddressMask == KERNEL_ASPACE_BASE);
static_assert(~kRiscv64CanonicalAddressMask == KERNEL_ASPACE_SIZE - 1);

constexpr bool is_user_accessible(vaddr_t va) {
  // This address refers to userspace if it is in the lower half of the
  // canonical addresses.  IOW - if all of the bits in the canonical address
  // mask are zero.
  return (va & kRiscv64CanonicalAddressMask) == 0;
}

// Check that the continuous range of addresses in [va, va+len) are all
// accessible to the user.
constexpr bool is_user_accessible_range(vaddr_t va, size_t len) {
  vaddr_t end = 0;

  // Check for normal overflow which implies the range is not continuous.
  if (add_overflow(va, len, &end)) {
    return false;
  }

  // Check that the start and end are accessible to userspace.
  if (!is_user_accessible(va) || (len != 0 && !is_user_accessible(end - 1))) {
    return false;
  }

  return true;
}

// Assert that user space also lines up with the canonical mask calculation.
static_assert(is_user_accessible_range(USER_ASPACE_BASE, USER_ASPACE_SIZE));
static_assert(!is_user_accessible_range(USER_ASPACE_BASE, USER_ASPACE_SIZE + 1));

// Userspace threads can only set an entry point to userspace addresses, or
// the null pointer (for testing a thread that will always fail).
constexpr bool arch_is_valid_user_pc(vaddr_t pc) {
  return (pc == 0) || (is_user_accessible(pc) && !is_kernel_address(pc));
}

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_VM_H_
