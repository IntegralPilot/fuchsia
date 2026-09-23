// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <lib/ld/tlsdesc.h>

#ifndef TLSDESC_RETAIN
#error "build system must define TLSDESC_RETAIN"
#endif

#if defined(__aarch64__)

// The GOT slot contains the offset from TPIDR_EL0.  Just return it.
//
// Since the function is so tiny, keep it naturally-aligned (it's a mere 8
// bytes) just to be sure it can never straddle a cache line.
.tlsdesc.function _ld_tlsdesc_runtime_static, retain=TLSDESC_RETAIN, align=8
  .tlsdesc.lsda kStatic

  // On entry x0 contains the argument: the address of the GOT slot pair.
  // The first word holds our own PC, the second the static TLS offset.
  ldr tlsdesc_r0, [x0, #tlsdesc.value_offset]

  // On exit x0 contains the return value: offset from $tp (TPIDR_EL0).
  .cfi_undefined x0

  ret
.end_function

#elif defined(__arm__)

.tlsdesc.function _ld_tlsdesc_runtime_static, retain=TLSDESC_RETAIN
  .tlsdesc.lsda kStatic

  // On entry r0 contains the argument: the address of the GOT slot pair.
  // The first word holds our own PC, the second the static TLS offset.
  ldr r0, [r0, #4]

  // On exit r0 contains the return value: offset from $tp.
  .cfi_undefined r0

  bx lr
.end_function

#elif defined(__x86_64__)

// The GOT slot contains the offset from %fs.base.  Just return it.
//
// Since the function is so tiny, keep it naturally-aligned (it's actually
// only 5 bytes) just to be sure it can never straddle a cache line.
.tlsdesc.function _ld_tlsdesc_runtime_static, retain=TLSDESC_RETAIN, align=8
  .tlsdesc.lsda kStatic

  // On entry %rax contains the argument: the address of the GOT slot pair.
  // The first word holds our own PC, the second the static TLS offset.
  //
  // Note that on x86-64 ILP32, GOT entries are still 8 bytes, to facilitate
  // use of the indirect addressing modes.
  mov 8(%rax), %rax

  // On exit %rax contains the return value: offset from $tp (%fs.base).
  .cfi_undefined %rax

  ret
.end_function

#elif defined(__riscv)

// The GOT slot contains the offset from tp.  Just return it.
//
// Since the function is so tiny, keep it naturally-aligned just to be sure it
// can never straddle a cache line.
.tlsdesc.function _ld_tlsdesc_runtime_static, retain=TLSDESC_RETAIN, align=4
  .tlsdesc.lsda kStatic

  // On entry a0 contains the argument: the address of the GOT slot range.
  // The first word holds our own PC, the second the static TLS offset.
  tlsdesc.load a0, tlsdesc.value_offset(a0)

  // On exit a0 contains the return value: offset from tp.
  .cfi_undefined a0

  // The caller's return address is in t0, with ra preserved.
  jr t0
.end_function

#else

// Not all machines have TLSDESC support specified in the psABI.

#endif
