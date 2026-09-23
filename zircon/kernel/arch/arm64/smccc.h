// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_SMCCC_H_
#define ZIRCON_KERNEL_ARCH_ARM64_SMCCC_H_

#ifdef __ASSEMBLER__  // clang-format off

#include <lib/code-patching/asm.h>

#include <arch/code-patches/case-id-asm.h>

// This is the SMCCC conduit instruction, which will be patched in place
// to the appropriate one.
.macro smccc_conduit
  .code_patching.start CASE_ID_SMCCC_CONDUIT
  smc #0
  .code_patching.end
.endm

// This is a `mov w0, #...` instruction to set w0 to the
// SMCCC_ARCH_WORKAROUND_3 function identifier if it's available, or
// else to an alternate function identifier to use in its place.
// It's left unpatched if the vectors it appears in aren't being used at all.
.macro smccc_workaround_function_w0
  .code_patching.start CASE_ID_SMCCC_WORKAROUND_FUNCTION
  udf #0xdead  // Must be patched!
  .code_patching.end
.endm

#else  // clang-format on

#include <lib/arch/arm64/smccc.h>

#include <cstdint>

constexpr uint32_t kMovW0SmcccArchWorkaround3 = 0x32013be0;
constexpr uint32_t kMovW0SmcccArchWorkaround1 = 0x320183e0;
constexpr uint32_t kMovW0PsciVersion = 0x52b08000;

// This is defined in assembly.  The first argument is the Function Identifier
// and the other arguments vary by function.
extern "C" uint64_t ArmSmcccCall(arch::ArmSmcccFunction function, uint64_t arg1 = 0,
                                 uint64_t arg2 = 0, uint64_t arg3 = 0);

#endif

#endif  // ZIRCON_KERNEL_ARCH_ARM64_SMCCC_H_
