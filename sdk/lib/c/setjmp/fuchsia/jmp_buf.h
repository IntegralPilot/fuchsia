// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_SETJMP_FUCHSIA_JMP_BUF_H_
#define LIB_C_SETJMP_FUCHSIA_JMP_BUF_H_

#include "asm-linkage.h"

// These get mangled so the raw pointer values don't leak into the heap.
#define JB_PC 0
#define JB_FP 1
#define JB_SP 2
#ifdef __x86_64__
#define JB_USP 3
#else
#define JB_SCSP 3
#endif
#define JB_MANGLE_COUNT 4
#define JB_CHECKSUM JB_MANGLE_COUNT
#define JB_COMMON_COUNT (JB_CHECKSUM + 1)

#ifdef __x86_64__

// Other callee-saves registers.
#define JB_RBX (JB_COMMON_COUNT + 0)
#define JB_R12 (JB_COMMON_COUNT + 1)
#define JB_R13 (JB_COMMON_COUNT + 2)
#define JB_R14 (JB_COMMON_COUNT + 3)
#define JB_R15 (JB_COMMON_COUNT + 4)
#define JB_COUNT (JB_COMMON_COUNT + 5)

#elif defined(__aarch64__)

// Callee-saves registers are [x19,x28] and [d8,d15].
#define JB_X(n) (JB_COMMON_COUNT + (n) - 19)
#define JB_D(n) (JB_X(29) + (n) - 8)
#define JB_SPARE JB_D(16)  // Unused.
#define JB_COUNT (JB_SPARE + 1)

#elif defined(__riscv)

// Callee-saves registers are s0..s11, but s0 is FP and so handled above.
#define JB_S(n) (JB_COMMON_COUNT + (n) - 1)

// FP registers fs0..fs11 are also callee-saves.
#define JB_FS(n) (JB_S(12) + (n))
#if JB_FS(0) <= JB_S(11)
#error "JB_FS defined wrong"
#endif

#define JB_SPARE JB_FS(12)  // Unused.
#define JB_COUNT (JB_SPARE + 1)

#else

#error what architecture?

#endif

#ifndef __ASSEMBLER__

#include <setjmp.h>

#include <array>
#include <cstdint>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// This is used by the assembly code via LIBC_ASM_LINKAGE(gJmpBufManglers).
// Early startup initializes it with random bits.
[[gnu::visibility("hidden")]] extern std::array<uint64_t, JB_MANGLE_COUNT> gJmpBufManglers
    LIBC_ASM_LINKAGE_DECLARE(gJmpBufManglers);

static_assert(sizeof(__jmp_buf) == sizeof(uint64_t) * JB_COUNT, "fix __jmp_buf definition");

// longjmp tail-calls this when called with a corrupted jmp_buf.
[[noreturn]] void longjmp_corrupted(jmp_buf) LIBC_ASM_LINKAGE_DECLARE(longjmp_corrupted);

}  // namespace LIBC_NAMESPACE_DECL

#else  // clang-format off

// This is just a shorthand to define all the variants of the names.
.macro jmp_buf.llvm_libc_function name
  .llvm_libc_function \name
  .llvm_libc_public \name
  .llvm_libc_public \name, _\name
  .llvm_libc_public \name, sig\name, weak

  // This is .end_function but also defines an end symbol for use in tests.
  .macro jmp_buf.end_function
    .purgem jmp_buf.end_function
    .label LIBC_ASM_LINKAGE(\name\()_end), global
    .end_function
  .endm

  // This extra alias for the start is also convenient for tests.
  .label LIBC_ASM_LINKAGE(\name\()_start), global
.endm

// CFI to find regno at [jb_regno, #8 * index].
.macro jmp_buf.cfi jb_regno, regno, index
  .sleb128.size_dispatch jmp_buf.cfi.1byte, jmp_buf.cfi.2byte, \
                         (8 * \index), \jb_regno, \regno
.endm
.macro jmp_buf.cfi.1byte offset, jb_regno, regno
  .cfi_escape DW_CFA_expression, \regno, 2, \
              DW_OP_breg(\jb_regno), SLEB128_1BYTE(\offset)
.endm
.macro jmp_buf.cfi.2byte offset, jb_regno, regno
  .cfi_escape DW_CFA_expression, \regno, 3, \
              DW_OP_breg(\jb_regno), SLEB128_2BYTE(\offset)
.endm

#endif // clang-format off

#endif  // LIB_C_SETJMP_FUCHSIA_JMP_BUF_H_
