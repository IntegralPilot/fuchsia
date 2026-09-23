// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_ASM_UTILS_INCLUDE_STARNIX_ASM_UTILS_ASM_H_
#define SRC_STARNIX_LIB_ASM_UTILS_INCLUDE_STARNIX_ASM_UTILS_ASM_H_

// Macros cribbed from //zircon/kernel/include/asm.h for use in trampolines.

#ifndef __ASSEMBLER__
#error for assembly files only
#endif

// clang-format off

#define LOCAL_FUNCTION_LABEL(x) .type x,STT_FUNC; x:
#define LOCAL_FUNCTION(x) LOCAL_FUNCTION_LABEL(x) .cfi_startproc
#define FUNCTION(x) .global x; .hidden x; LOCAL_FUNCTION(x)

#define END_FUNCTION(x) .cfi_endproc; .size x, . - x

#endif  // SRC_STARNIX_LIB_ASM_UTILS_INCLUDE_STARNIX_ASM_UTILS_ASM_H_
