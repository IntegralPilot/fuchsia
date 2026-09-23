// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PLATFORM_INCLUDE_PLATFORM_MEXEC_H_
#define ZIRCON_KERNEL_PLATFORM_INCLUDE_PLATFORM_MEXEC_H_

#include <arch/defines.h>

#define MEMMOV_OPS_DST_OFFSET (0)
#define MEMMOV_OPS_SRC_OFFSET (8)
#define MEMMOV_OPS_LEN_OFFSET (16)
#define MEMMOV_OPS_STRUCT_LEN (24)

// An upper bound defined in terms of 4KiB pages, but one that is sufficiently
// generous for any choice of page size.
#define MAX_OPS_PER_PAGE (169)  // (4096 / 24) - 1

#ifndef __ASSEMBLER__

#include <lib/zx/result.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <fbl/array.h>
#include <ktl/byte.h>
#include <ktl/span.h>
#include <vm/vm_object.h>

// Warning: The geometry of this struct is depended upon by the mexec assembly
//          function. Do not modify without also updating mexec.S.
typedef struct __PACKED {
  void* dst;
  void* src;
  size_t len;
} memmov_ops_t;

static_assert(sizeof(memmov_ops_t) == MEMMOV_OPS_STRUCT_LEN,
              "sizeof memmov_ops_t must match MEMMOV_OPS_STRUCT_LEN");

// Implemented in assembly. Copies the new kernel into place and branches to it.
typedef void (*mexec_asm_func)(uint64_t arg0, uint64_t arg1, uint64_t arg2, uint64_t aux,
                               const memmov_ops_t* ops, uintptr_t new_kernel_entry)
    [[clang::cfi_unchecked_callee]];

// Writes an mexec data ZBI into the provided buffer and returns the size of
// that ZBI if successful.
zx::result<size_t> WriteMexecData(ktl::span<ktl::byte> buffer);

/* This function is called at the beginning of mexec.  Interrupts are not yet
 * disabled, but only one CPU is running.
 */
void platform_mexec_prep(uintptr_t final_data_zbi_addr, size_t final_data_zbi_len);

/* Ask the platform to mexec into the next kernel.
 * This function is called after platform_mexec_prep(), with interrupts disabled.
 */
void platform_mexec(mexec_asm_func mexec_assembly, ktl::span<const memmov_ops_t> ops,
                    uintptr_t new_kernel_addr, size_t new_kernel_len, uintptr_t new_kernel_entry,
                    uintptr_t new_data_zbi_addr, size_t new_data_zbi_len);

/* Allocate |count| pages where no page has a physical address less than
 * |lower_bound|.
 * Results are returned via the array pointed to by |paddrs| with the
 * assumption there is enough storage to contain |count| results.
 * |limit| defines the highest address to search before giving up.
 */
zx_status_t alloc_pages_greater_than(paddr_t lower_bound, size_t count, size_t limit,
                                     paddr_t* paddrs);

static_assert(__offsetof(memmov_ops_t, dst) == MEMMOV_OPS_DST_OFFSET, "");
static_assert(__offsetof(memmov_ops_t, src) == MEMMOV_OPS_SRC_OFFSET, "");
static_assert(__offsetof(memmov_ops_t, len) == MEMMOV_OPS_LEN_OFFSET, "");

#endif  // __ASSEMBLER__

#endif  // ZIRCON_KERNEL_PLATFORM_INCLUDE_PLATFORM_MEXEC_H_
