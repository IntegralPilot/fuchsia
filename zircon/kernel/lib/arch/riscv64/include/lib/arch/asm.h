// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_ASM_H_
#define ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_ASM_H_

// Get the generic file.
#include_next <lib/arch/asm.h>

#ifdef __ASSEMBLER__  // clang-format off

#ifndef __has_feature
#define __has_feature(x) 0
#endif

// Kernel code is compiled with -ffixed-x27 (x27 == s11) so the compiler won't
// use it.
#define percpu_ptr s11

// This register is permanently reserved by the ABI in the compiler.
// #if __has_feature(shadow_call_stack) it's used for the SCSP.
#define shadow_call_sp gp

#define DWARF_REGNO_shadow_call_sp 3 // x3 == gp
#define DWARF_REGNO_ra 1 // x1 == ra
#define DWARF_REGNO_t0 5 // x5 == t0

#define DWARF_REGNO_V(n) ((n) + 96)

#define DWARF_REGNO_CSR(n) (4096 + (n))
#define VLENB_REGNO 3106

#define DWARF_REGNO_VLENB DWARF_REGNO_CSR(VLENB_REGNO)

.macro assert.fail
  unimp
.endm

.macro .cfi.all_integer op
  .irp reg,x1,x2,x3,x4,x5,x6,x7,x8,x9,x10,x11,x12,x13,x14,x15,x16,x17,x18,x19,x20,x21,x22,x23,x24,x25,x26,x27,x28,x29,x30,x31
    \op \reg
  .endr
.endm

.macro .cfi.all_vectorfp op
#ifdef __riscv_f
  .irp reg,f0,f1,f2,f3,f4,f5,f6,f7,f8,f9,f10,f11,f12,f13,f14,f15,f16,f17,f18,f19,f20,f21,f22,f23,f24,f25,f26,f27,f28,f29,f30,f31
    \op \reg
  .endr
#endif
  .cfi.all_vector \op
.endm

.macro .cfi.all_vector op
#ifdef __riscv_v
    .irp reg,v0,v1,v2,v3,v4,v5,v6,v7,v8,v9,v10,v11,v12,v13,v14,v15,v16,v17,v18,v19,v20,v21,v22,v23,v24,v25,v26,v27,v28,v29,v30,v31
    \op \reg
  .endr
#endif
.endm

.macro .cfi.all_call_used op
  .irp reg,t0,t1,t2,t3,t4,t5,t6,a0,a1,a2,a3,a4,a5,a6,a7
    \op \reg
  .endr
#ifdef __riscv_f
  .irp reg,ft0,ft1,ft2,ft3,ft4,ft5,ft6,ft7,ft8,ft9,ft10,ft11,fa0,fa1,fa2,fa3,fa4,fa5,fa6,fa7
    \op \reg
  .endr
#endif
  .cfi.all_vector \op
.endm

// Add an immediate value to SP, adjusting CFI assuming the CFA rule
// uses an offset from SP.
.macro add.sp imm
  add sp, sp, \imm
  .cfi_adjust_cfa_offset -(\imm)
.endm

.macro sd.spill reg, offset
  sd \reg, \offset(sp)
  .cfi_rel_offset \reg, \offset
.endm

.macro ld.reload reg, offset
  ld \reg, \offset(sp)
  .cfi_same_value \reg
.endm

// Standard prologue sequence for FP setup, with CFI.  This emits .stack_sizes
// metadata unless the optional third argument prevents it, in which case a
// separate .llvm.stack_size invocation should be used.
.macro .prologue.fp frame_extra_size=0, rareg=ra, no_metadata=
  .llvm.stack_size (\frame_extra_size + 16), \no_metadata
  // The RISC-V psABI defines the frame pointer convention:
  // https://github.com/riscv-non-isa/riscv-elf-psabi-doc/blob/master/riscv-cc.adoc#frame-pointer-convention
  // fp must point to CFA.
  // ra must reside at fp - XLEN/8.
  // Previous fp must reside at fp - x * XLEN/8.
  add.sp -(16 + (\frame_extra_size))
  // The CFA is still computed relative to the SP so code will
  // continue to use .cfi_adjust_cfa_offset for pushes and pops.
  sd.spill fp, \frame_extra_size
  sd.spill \rareg, (8 + (\frame_extra_size))
  add fp, sp, (16 + (\frame_extra_size))
.endm

// Epilogue sequence to match .prologue.fp with the same argument.
.macro .epilogue.fp frame_extra_size=0, rareg=ra
  ld fp, (\frame_extra_size)(sp)
  .cfi_same_value fp
  ld.reload \rareg, (8 + (\frame_extra_size))
  add.sp 16 + (\frame_extra_size)
.endm

// Helper defining the encoding for setting rules requiring
//   DW_OP_breg3(-8) as a 2 byte sequence
#define CFI_SCS_BREG(op, regno) .cfi_escape op, regno, 2, \
  DW_OP_breg(DWARF_REGNO_shadow_call_sp), (-8) & 0x7f

// Standard prologue sequence for shadow call stack, with CFI.
.macro .prologue.shadow_call_sp rareg=ra
#if __has_feature(shadow_call_stack)
  sd \rareg, (shadow_call_sp)
  // Set the ra (x1) rule to DW_CFA_expression{DW_OP_breg3(-8)}.
  .ifc \rareg, ra
    CFI_SCS_BREG(DW_CFA_expression, DWARF_REGNO_ra)
  .else
    .ifc \rareg, t0
      CFI_SCS_BREG(DW_CFA_expression, DWARF_REGNO_t0)
    .else
      .error "Return address must be `ra` or `t0`"
    .endif
  .endif
  add shadow_call_sp, shadow_call_sp, 8
  // Set the x3 (gp) rule to DW_CFA_val_expression{DW_OP_breg3(-8)} to
  // compensate for the increment just done.
  CFI_SCS_BREG(DW_CFA_val_expression, DWARF_REGNO_shadow_call_sp)
#endif
.endm

// Epilogue sequence to match .prologue.shadow_call_sp.
.macro .epilogue.shadow_call_sp rareg=ra
#if __has_feature(shadow_call_stack)
  ld \rareg, -8(shadow_call_sp)
  .cfi_same_value \rareg
  add shadow_call_sp, shadow_call_sp, -8
  .cfi_same_value shadow_call_sp
#endif
.endm

#endif  // clang-format on

#endif  // ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_ASM_H_
