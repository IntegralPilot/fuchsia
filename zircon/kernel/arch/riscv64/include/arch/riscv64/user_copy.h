// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_USER_COPY_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_USER_COPY_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <arch/arch_thread.h>

// Typically we would not use structs as function return values, but in this case it enables us to
// very efficiently use the 2 registers for return values to encode the optional flags and va
// page fault values.
struct Riscv64UserCopyRet {
  zx_status_t status;
  unsigned int pf_flags;
  vaddr_t pf_va;
};
static_assert(sizeof(Riscv64UserCopyRet) == 16, "Riscv64UserCopyRet has unexpected size");

// If this bit is set in the data_fault_resume field of the thread, run the routine first, before
// calling into the page fault handler.
#define RISCV_CAPTURE_USER_COPY_FAULTS_BIT (1UL)

// This is the same as memcpy, except that it takes the additional argument of
// &current_thread()->arch.data_fault_resume, where it temporarily stores the fault recovery PC for
// bad page faults to user addresses during the call.
extern "C" {
Riscv64UserCopyRet _riscv64_user_copy(void* dst, const void* src, size_t len,
                                      uint64_t* fault_return, uint64_t capture_faults_mask);

Riscv64UserCopyRet rust_arch_copy_from_user_capture_faults(void* dst, const void* src, size_t len);
Riscv64UserCopyRet rust_arch_copy_to_user_capture_faults(void* dst, const void* src, size_t len);
}

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_USER_COPY_H_
