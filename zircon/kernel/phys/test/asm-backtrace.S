// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// We need an assembly function that uses .prologue.fp/.epilogue.fp to test
// that our assembly implementations for prologue/epilogue maintain ABI
// constraints for the various platforms. `asm_func` and its callee `foo` take
// no arguments, but return values in registers according to the architecture's
// calling convention.
.function CallerWithAsmPrologue, global
#if __has_feature(shadow_call_stack)
  .prologue.shadow_call_sp
#endif
  .prologue.fp

#if defined(__aarch64__)
  bl CalledFromAsmWithPrologue
#elif defined(__riscv) || defined(__x86_64__) || defined(__i386__)
  call CalledFromAsmWithPrologue
#else
  #error "unsupported machine"
#endif

  .epilogue.fp
#if __has_feature(shadow_call_stack)
  .epilogue.shadow_call_sp
#endif
  ret
.end_function
