// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <lib/ld/tlsdesc.h>

// See tlsdesc-runtime-dynamic.h comments for the background.
// This file implements the TLSDESC entry points described there.
//
// Each machine has a bespoke calling convention for TLSDESC entry points
// different from the standard C function calling convention on that same
// machine.  But they're all about the same:
//  * The return value register is used as the argument register.  (On most
//    machines, they're the same register in the normal convention anyway.)
//  * That argument register gets a pointer to two (read-only) GOT slots
//    (address-width on most machines, but not on x86-64 ILP32), the second
//    of which encodes the true arguments for the function.
//  * The SP follows normal ABI conventions and can be pushed onto.
//  * The FP is not expected to be touched by a leaf function.
//  * All other registers must be preserved on return.
//  * No other registers have known values except the return address
//    register (for branch-and-link machines).  Machines able to use an
//    alternate return address register do so (RISC-V: t0 instead of ra).
//
// The two entry points here share most of their code for each machine.  The
// "split" version (most commonly used) extracts the two bit fields from the
// GOT value; the "indirect" version (rarely used) uses the GOT value as a
// pointer to two uintptr_t fields: index and offset.  Then it does a TLS
// access in IE model to read _dl_tlsdesc_runtime_dynamic_blocks and compute
// `blocks[index] + offset - $tp` into the return value register.
//
// The implementation pairs for each machine are quite similar as well.  A
// prologue common to both versions spills additional registers to be used
// as temporaries and does the TLS access to get the blocks vector pointer
// into a register.  Each version has different code to load the second GOT
// slot via the argument register and extract an index and offset from it
// into registers.  Finally, a common tail computes the return value of
// `blocks[index] + offset - $tp`; reloads spills; and returns.
//
// Each machine defines a prologue macro used by both entry points.  The
// common tail is implemented directly in the "split" version at the label
// `.Lload_block`; the less-used "indirect" version just jumps there.
//
// The code is written to work for ILP32 as well as LP64 (and for AArch32),
// though that has not been thoroughly tested yet.  Note that GOT slot size
// matches pointer size on most machines, but not on x86-64 (ILP32).

#ifdef __UINTPTR_WIDTH__
#define SPLIT_SHIFT (__UINTPTR_WIDTH__ / 2)
#else
#error "Compiler should predefine __UINTPTR_WIDTH__"
#endif

#if __UINTPTR_WIDTH__ == 64
#define PTR_SHIFT 3
#elif __UINTPTR_WIDTH__ == 32
#define PTR_SHIFT 2
#else
#error "Unexpected __UINTPTR_WIDTH__ value!"
#endif

#if defined(__aarch64__)

// This is the shared beginning portion of both the split and indirect
// versions.  It extracts the the GOT's value slot into x1 and the blocks
// vector pointer into x2, with x0 then available for scratch.  Note that some
// uses are literal x[012] and some instead use tlsdesc_r[012] macros,
// depending on whether it's a base register (always 64 bits) or it's a
// pointer-sized operand.
.macro prologue
  stp.spill x1, x2

  // Fetch the blocks vector pointer using a standard IE model TLS read:
  // load the $tp offset from the GOT; load the value from $tp + offset.
#ifdef __AARCH64_CMODEL_TINY__
  ldr	tlsdesc_r1, :gottprel:_dl_tlsdesc_runtime_dynamic_blocks
#else
  adrp x1, :gottprel:_dl_tlsdesc_runtime_dynamic_blocks
  ldr	tlsdesc_r1, [x1, #:gottprel_lo12:_dl_tlsdesc_runtime_dynamic_blocks]
#endif
  mrs x2, TPIDR_EL0  // System registers are 64 bits.
  ldr tlsdesc_r2, [x2, tlsdesc_uxtw(tlsdesc_r1, #0)]

  // Fetch the value word from the GOT slot (argument pointer).
  ldr tlsdesc_r1, [x0, #tlsdesc.value_offset]

  // The caller's value (the pointer into the GOT) will be clobbered next.
  .cfi_undefined x0
.endm

.tlsdesc.function _dl_tlsdesc_runtime_dynamic_split
  // The first portion of the function is the same here and below.
  prologue

  // x0 gets the low bits: the offset.
  ubfx tlsdesc_r0, tlsdesc_r1, #0, #SPLIT_SHIFT

  // x1 gets the high bits: the block index.
  ubfx tlsdesc_r1, tlsdesc_r1, #SPLIT_SHIFT, #SPLIT_SHIFT

.Lload_block:
  // Fetch the module's block (x1) from the blocks vector.
  ldr tlsdesc_r1, [x2, tlsdesc_uxtw(tlsdesc_r1, #PTR_SHIFT)]

  // x2 is no longer needed; get $tp back there so it can be subtracted out.
  mrs x2, TPIDR_EL0

  // Finally, return value (x0) = block (x1) + offset (x0) - $tp (x2).
  add tlsdesc_r0, tlsdesc_r1, tlsdesc_r0
  sub tlsdesc_r0, tlsdesc_r0, tlsdesc_r2

  ldp.reload x1, x2
  ret
.end_function

.tlsdesc.function _dl_tlsdesc_runtime_dynamic_indirect
  // The first portion of the function is the same here and above.
  prologue

  // x1 gets the first word: the block index.
  // x0 gets the second word: the offset.
  ldp tlsdesc_r1, tlsdesc_r0, [x1]

  // The tail is the same as for the (hotter) split case, so share its code.
  b .Lload_block
.end_function

#elif defined(__arm__)

// This is the shared beginning portion of both the split and indirect
// versions.  It extracts the the GOT's value slot into r0 and the blocks
// vector pointer into r2, with r1 then available for scratch.
.macro prologue
  .tlsdesc.cfi

  push.spill r1, r2

  ldr r0, [r0, #4]
  .cfi_undefined r0  // Caller's value (argument pointer) no longer available.

  // Load the $tp offset from the GOT for IE model TLS access.
  ldr r1, =_dl_tlsdesc_runtime_dynamic_blocks(GOTTPOFF) - (0f + pcrel.bias)
  read_tp r2  // Fetch $tp in between.
0:add r1, pc
  ldr r1, [r1]

  // Load the blocks vector pointer (r2) from $tp (r2) + offset (r1).
  ldr r2, [r2, r1]
.endm

.tlsdesc.function _dl_tlsdesc_runtime_dynamic_split
  // The first portion of the function is the same here and below.
  prologue

  // r1 gets the low bits: the offset.
  uxth r1, r0
  // r0 gets the high bits: the block index.
  lsr r0, r0, #SPLIT_SHIFT

.Lload_block:
  // Fetch the module's block from the blocks vector.
  ldr r0, [r2, r0, lsl #2]

  // Recover $tp so we can subtract it out.
  read_tp r2

  // Finally, add in the offset and subtract out $tp.
  add r0, r0, r1
  sub r0, r0, r2

  // Epilogue.
  pop.reload r1, r2
  bx lr
.end_function

.tlsdesc.function _dl_tlsdesc_runtime_dynamic_indirect
  // The first portion of the function is the same here and above.
  prologue

  // r0 gets the first word: the block index.
  // r1 gets the second word: the offset.
  ldm r0, {r0, r1}

  // The tail is the same as for the split case, so share its code.
  b .Lload_block
.end_function

#elif defined(__riscv)

// This is the shared beginning portion of both the split and indirect
// versions.  It extracts the the GOT's value slot into a1 and the blocks
// vector pointer into a2, with a0 then available for scratch.
.macro prologue
  add sp, sp, -16
  .cfi_adjust_cfa_offset 16
  sd a1, 0(sp)
  .cfi_rel_offset a1, 0
  sd a2, 8(sp)
  .cfi_rel_offset a2, 8

  tlsdesc.load a1, tlsdesc.value_offset(a0)

  // Fetch the tp offset from the GOT for IE model TLS access.
  la.tls.ie a2, _dl_tlsdesc_runtime_dynamic_blocks

  // Load the blocks vector pointer (a2) from tp + offset (a2).
  tlsdesc.add a2, tp, a2
  tlsdesc.load a2, (a2)

  // The caller's value (the pointer into the GOT) will be clobbered next.
  .cfi_undefined a0
.endm

.tlsdesc.function _dl_tlsdesc_runtime_dynamic_split
  // The first portion of the function is the same here and below.
  prologue

  // a0 gets the low bits: the offset.
  // a1 gets the high bits: the block index.
#ifdef _LP64
  zext.w a0, a1
  srl a1, a1, SPLIT_SHIFT
#else
  zext.h a0, a1, SPLIT_SHIFT
  srlw a1, a1, SPLIT_SHIFT
#endif

.Lload_block:
  // The blocks vector element contains a pointer, and we're adding an offset
  // to that.  But the return value is not that final pointer!  Instead, it's
  // that pointer's distance from tp, which the caller will add back in (this
  // makes most sense when the static TLS case is considered).  So adjust the
  // offset down by the tp value here.
  tlsdesc.sub a0, a0, tp

  // Fetch the module's block from the blocks vector.
  sll a1, a1, PTR_SHIFT  // Scale up by pointer size.
  add a1, a2, a1         // Add the blocks vector pointer.
  tlsdesc.load a1, (a1)  // a1 = blocks[index]

  // Add in the offset.
  tlsdesc.add a0, a1, a0

  // Epilogue.
  ld a1, 0(sp)
  .cfi_same_value a1
  ld a2, 8(sp)
  .cfi_same_value a2
  add sp, sp, 16
  .cfi_adjust_cfa_offset -16

  // The caller's return address is in t0, with ra preserved.
  jr t0
.end_function

.tlsdesc.function _dl_tlsdesc_runtime_dynamic_indirect
  // The first portion of the function is the same here and above.
  prologue

  // a0 gets the second word: the offset.
  tlsdesc.load a0, __SIZEOF_POINTER__(a1)
  // a1 gets the first word: the block index.
  tlsdesc.load a1, 0(a1)

  // The tail is the same as for the split case, so share its code.
  tail .Lload_block
.end_function

#elif defined(__x86_64__)

// This is the shared beginning portion of both the split and indirect
// versions.  It extracts the the GOT's value slot into %rax and the blocks
// vector pointer into %rdx, with %rcx then available for scratch.
.macro prologue
  push.spill %rcx
  push.spill %rdx

  // On entry %rax contains the argument: the address of the GOT slot pair.
  // The first word holds our own PC, the second is the value slot.
  // Note: GOT slots are 64 bits even for ILP32!
  mov 8(%rax), %rax
  .cfi_undefined %rax

  // Fetch the $tp offset from the GOT for IE model TLS access.
  mov _dl_tlsdesc_runtime_dynamic_blocks@GOTTPOFF(%rip), %rdx
  // Load the blocks vector pointer (%rdx) from $tp + offset (%rdx).
  mov %fs:(%rdx), %tlsdesc_dx
.endm

// Note that on x86-64 ILP32, GOT entries are still 8 bytes, to facilitate
// use of the indirect addressing modes.  This means that even ILP32 can
// make use of a full 32 bits for each of index and offset.
.tlsdesc.function _dl_tlsdesc_runtime_dynamic_split
  // The first portion of the function is the same here and below.
  prologue

  // %rcx gets the low bits: the offset.
  mov %eax, %ecx
  // %rax gets the high bits: the block index.
  shr $32, %rax

.Lload_block:
  // Fetch the module's block (index %rax) from the blocks vector (%rdx).
  mov (%rdx, %rax, __SIZEOF_POINTER__), %tlsdesc_ax

  // Finally, add in the offset and subtract back out $tp that will be added.
  add %tlsdesc_cx, %tlsdesc_ax
  sub %fs:0, %tlsdesc_ax

  // Epilogue.
  pop.reload %rdx
  pop.reload %rcx
  ret
.end_function

.tlsdesc.function _dl_tlsdesc_runtime_dynamic_indirect
  // The first portion of the function is the same here and above.
  prologue

  // %rcx gets the second word: the offset.
  mov __SIZEOF_POINTER__(%rax), %tlsdesc_cx
  // %rax gets the first word: the block index.
  mov (%rax), %tlsdesc_ax

  // The tail is the same as for the split case, so share its code.
  jmp .Lload_block
.end_function

#else

// Not all machines have TLSDESC support specified in the psABI.

#endif
