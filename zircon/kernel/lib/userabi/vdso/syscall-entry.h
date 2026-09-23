// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

#include <lib/arch/asm.h>

#ifdef __ASSEMBLER__
// clang-format off

.macro syscall_entry_begin name
  .function SYSCALL_\name, global
.endm

.macro syscall_entry_end name, public=1
  .end_function

  // Create a hidden alias for the syscall which is prefixed with CODE_.  This
  // allows the macros which perform redirection in the kernel to redirect a
  // VDSO entry to either an explicit CODE_ alternate, or to another syscall if
  // needed.
  .alias CODE_SYSCALL_\name, SYSCALL_\name

  // For wrapper functions, aliasing is handled by the generator.
  .if \public
    .alias _\name, SYSCALL_\name, export
    .alias \name, SYSCALL_\name, weak
    .alias VDSO_\name, SYSCALL_\name
  .endif
.endm

// clang-format on
#endif  // __ASSEMBLER__
