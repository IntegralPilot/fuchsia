// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <lib/ld/tlsdesc.h>
#include "pthread_arch.h"

// Similar to other machines, the RISC-V psABI defines a 2-Slot layout for
// TLSDESC resolution. In both the static and dynamic cases, a0 will point to
// the GOT. In the static case, the second slot has the offset from tp. In the
// dynamic case, the second slot has to be a pointer to the pair of module ID
// and offset within that module's PT_TLS segment.
//
// In both cases, the return address is in t0 rather than ra.  No registers
// other than a0 and t0 may be clobbered.

// For static TLS, the offset stored is the tp offset.  Just return it.
.tlsdesc.function __tlsdesc_static
  ld a0, 8(a0)
  .cfi_undefined a0
  jr t0
.end_function

// For dynamic TLS, the offset stored is the offset within the module's
// PT_TLS block and the module ID is stored too.
.tlsdesc.function __tlsdesc_dynamic

  // __tlsdesc_dynamic must preserve all registers, with the exception of the
  // return value in a0. The fast path only needs to save and restore 3
  // registers from scratch (a1, a2, a3). But the slow path needs a
  // variable-sized frame, because vector register size (VLENB) is only known
  // at runtime. Therefore, it needs an FP to allow the SP to vary according to
  // the VLENB value. Additionally, it will call into C code, so it should set
  // up FP links and SCS anyway. For simplicity, the hot path sets up the FP,
  // though it could do just the SP adjustment and defer the stores until the
  // slow path. The slow path sets up SCS before the call to __tls_get_new.
  //
  // The following spill/reload operations use an index, `idx`, relative to
  // the FP to spill into/reload from. idx = 0, corresponds to the CFA, idx = -1
  // corresponds to the stack slot just below the CFA.
  //
  // * Slots [-1,-6] are added via .prologue.fp . Slots [-3,-6] are added as
  //   extra space, with slot -6 being alignment-padding.
  #define PROLOGUE_SLOTS 6

  // * Slots [-7,-36] are added via on_saved_regs. These are scalar registers.
  #define NUM_SAVED_REGS 30

  // * The stack area below Slot -36 is used for V registers added via
  //   `spill_vector_regs`. Since we use vs8r.v/vl8r.v which stores 8 registers
  //   in one instruction, we track them in groups of 8 variable-sized blocks
  //   (RISC-V V registers are scalable, and the precise size in bytes is only
  //   known at runtime through VLENB). These are marked `V Spill Area` in the
  //   diagram.
  //
  //  In total, we require saving 35 scalar register, and 32 vector registers.
  //  Additionally, we require 1 stack slot to keep the stack aligned to 16
  //  bytes.
  //
  // The following diagram shows the layout of the stack.
  //
  //      idx  incoming parameters
  //        1+ Caller's stack frame (including incoming parameters)
  //        0   -------- <-- CFA = caller's SP = post-prologue FP
  //       -1      fp
  //       -2      t0 (alternate return register)
  //            --------  <-- SP after .prologue.fp
  //       -3      a1
  //       -4      a2
  //       -5      a3
  //       -6   alignment-padding
  //            --------  <-- SP after on_each_a spill
  //       -7      ra    NOTE: The stack area from here on is only used
  //       -8      a4          on the slow path.
  //       -9      a5
  //       -10     a6
  //       -11     a7
  //       -12     t2
  //       -13     t3
  //       -14     t4
  //       -15     t5
  //       -16     t6
  //       -17     ft0
  //       -18     ft1
  //       -19     ft2
  //       -20     ft3
  //       -21     ft4
  //       -22     ft5
  //       -23     ft6
  //       -24     ft7
  //       -25     ft8
  //       -26     ft9
  //       -27     ft10
  //       -28     ft11
  //       -29     fa0
  //       -30     fa1
  //       -31     fa2
  //       -32     fa3
  //       -33     fa4
  //       -34     fa5
  //       -35     fa6
  //       -36     fa7
  //            --------  <-- SP after on_saved_regs spill
  // V Spill Area  v31    NOTE: Each Vector register is of size VLENB.
  //               v30          The size of each spill area is 8 * VLENB.
  //               v29
  //               v28
  //               v27
  //               v26
  //               v25
  //               v24
  //            --------  <-- SP after spilling v23-v31 (sp = sp - (8*VLENB))
  // V Spill Area  v23
  //               v22
  //               v21
  //               v20
  //               v19
  //               v18
  //               v17
  //               v16
  //            --------  <-- SP after spilling v16-v23 (sp = sp - (8*VLENB))
  // V Spill Area  v15
  //               v14
  //               v13
  //               v12
  //               v11
  //               v10
  //               v9
  //               v8
  //            --------  <-- SP after spilling v8-v15 (sp = sp - (8*VLENB))
  // V Spill Area  v7
  //               v7
  //               v6
  //               v5
  //               v4
  //               v3
  //               v2
  //               v1
  //               v0
  //            --------  <-- SP after spilling v0-v7 (sp = sp - (8*VLENB))

  // Spill `reg` into slot `idx` from the fp (CFA).
  // A second argument of exactly `f` is required for f* instructions.
  .macro spill reg:req, idx:req, f=
    \f\()sd \reg, -((\idx) * 8)(fp)
    .cfi_rel_offset \reg, -((\idx) * 8)
  .endm

  // Reload `reg` from slot `idx` from the fp (CFA).
  // A second argument of exactly `f` is required for f* instructions.
  .macro reload reg:req, idx:req, f=
    \f\()ld \reg, -((\idx) * 8)(fp)
    .cfi_same_value \reg
  .endm

  // Applies op to a1,a2,a3 at slot n+2, from the fp (CFA).
  .macro on_each_a op:req
    // a1,a2,a3 are used in the fast path.
    .irp n,1,2,3
      // Slots for a1,a2,a3 are just after the 2 slots for the ra/fp pair.
      \op a\n, \n + 2
    .endr
  .endm

  // On the fast path we need an additional 3 slots to save/restore a1,a2,a3,
  // plus one slot to align the stack.
  .Lframe_size = 4 * 8

  // All the normally call-clobbered registers are actually preserved.
  .tlsdesc.cfi
  .prologue.fp .Lframe_size, t0
  .cfi_def_cfa fp, 0
  on_each_a spill

  // size_t __tlsdesc_dynamic(size_t *a)
  // {
  // 	struct {size_t modidx,off;} *p = (void*)a[1];
  // 	size_t *dtv = *(size_t**)(tp - 24);
  // 	if (p->modidx <= dtv[0])
  // 		return dtv[p->modidx] + p->off - tp;
  // 	return __tls_get_new(p[1], p[0]) - tp;
  // }

  ld a1, 8(a0)   // Pointer to struct: p
  ld a0, 8(a1)   // Offset: p->offset
  ld a1, (a1)    // module ID: p->modidx
  ld a2, -24(tp) // DTV
  ld a3, (a2)    // DTV[0], generation ID
  bgt a1, a3, .Lneed_new_dtv
  .cfi_remember_state
  sll a1, a1, 3  // Scale module ID to words.
  add a1, a1, a2 // &DTV[ID]
  ld a1, (a1)    // DTV[ID]
  add a0, a0, a1 // DTV[ID] + offset

.Lret:
  sub a0, a0, tp // TLS block pointer - tp

  on_each_a reload
  .epilogue.fp .Lframe_size, t0

  jr t0

.Lneed_new_dtv:
  // Perform operation `op` on saved registers, beginning at PROLOGUE_SLOT + 1.
  // Currently used with spill and reload macros.
  .macro on_saved_regs op
    \op ra, PROLOGUE_SLOTS + 1
    \op a4, PROLOGUE_SLOTS + 2
    \op a5, PROLOGUE_SLOTS + 3
    \op a6, PROLOGUE_SLOTS + 4
    \op a7, PROLOGUE_SLOTS + 5
    \op t2, PROLOGUE_SLOTS + 6
    \op t3, PROLOGUE_SLOTS + 7
    \op t4, PROLOGUE_SLOTS + 8
    \op t5, PROLOGUE_SLOTS + 9
    \op t6, PROLOGUE_SLOTS + 10
    // Note however that the LP64D calling convention allows __tls_get_new to
    // clobber the high halves when they're actually Q registers. We're not
    // preserving those since we can't presume the Q instructions are
    // available.
    \op ft0, PROLOGUE_SLOTS + 11, f
    \op ft1, PROLOGUE_SLOTS + 12, f
    \op ft2, PROLOGUE_SLOTS + 13, f
    \op ft3, PROLOGUE_SLOTS + 14, f
    \op ft4, PROLOGUE_SLOTS + 15, f
    \op ft5, PROLOGUE_SLOTS + 16, f
    \op ft6, PROLOGUE_SLOTS + 17, f
    \op ft7, PROLOGUE_SLOTS + 18, f
    \op ft8, PROLOGUE_SLOTS + 19, f
    \op ft9, PROLOGUE_SLOTS + 20, f
    \op ft10, PROLOGUE_SLOTS + 21, f
    \op ft11, PROLOGUE_SLOTS + 22, f
    \op fa0, PROLOGUE_SLOTS + 23, f
    \op fa1, PROLOGUE_SLOTS + 24, f
    \op fa2, PROLOGUE_SLOTS + 25, f
    \op fa3, PROLOGUE_SLOTS + 26, f
    \op fa4, PROLOGUE_SLOTS + 27, f
    \op fa5, PROLOGUE_SLOTS + 28, f
    \op fa6, PROLOGUE_SLOTS + 29, f
    \op fa7, PROLOGUE_SLOTS + 30, f
  .endm

  // Provide macros for consistent spill/reload ordering for vector groups
  //
  //  NOTE: We assume vector registers are spilled in descending order v31-v0
  //  (e.g., vector groups v24,v16,v8,v0 in that order). Spilling groups in
  //  ascending order (v0,v8,v16,v24) requires a more complex expression to
  //  represent:
  //    .cfa_offset v<n>, offset - (vlenb * (8*((n+8)/8) - n%8 -1))
  //
  //  We're including the expression, in case its ever needed, but the simpler
  //  expression described for v_spill_cfi.escape is what we use, and should be
  //  preferred.
  #define ALL_V_GROUP_SPILL_ORDER 24,16,8,0
  #define ALL_V_GROUP_RELOAD_ORDER 0,8,16,24

  // Load VLENB into `reg` and scale it up for a block of 8 V registers.
  // `reg` will be clobbered, so either use one that is saved or scavenge one.
  .macro compute_vector_group_size reg:req
    csrr \reg, vlenb
    sll \reg, \reg, 3
  .endm

  // Vector instructions are only available with V, so guard this with
  // __riscv_v. The logic here isn't very intuitive.
  //
  // Spill vector registers where the SP is \start_slots from the CFA.
  // It uses \reg as a scratch register to adjust sp, and thus should
  // be saved prior to these spills.
  .macro spill_vector_regs start_slots:req, reg:req
  #ifdef __riscv_v
    // This entire section assumes we're using vs8r.v/vl8r.v instructions
    // to spill/reload 8 vector registers at a time. That also means we
    // need to add .cfi_* directives for all 8 registers for each spill/reload.

    // Because RISC-V supports scalable vectors, VLENB is not a fixed
    // size, so we need to get its value before trying to spill anything.
    // `reg` will hold the size in bytes of a block of 8 V registers.
    compute_vector_group_size \reg

    // Adjust the SP by 8*VLENB bytes, and then store 8 registers [vn, vn+7]
    // to the stack for each group of V register (v0-7,v8-15,v16-23,v24-31).
    .irp n,ALL_V_GROUP_SPILL_ORDER
      // Adjust the SP down to make space for the 8 vector registers.
      sub sp, sp, \reg
      // vl8r.v only accepts v0,v8,v16,v24 as the register argument.
      vs8r.v v\n, (sp)
      v_cfi_spill_group \n, -((\start_slots) * 8)
    .endr
  #endif
  .endm

  // Emit cfi_* directives for vector spills with scalable vectors.
  //
  // The operation we're trying to express is:
  //    .cfa_offset v<n>, offset - (vlenb * (31 -n))
  //
  // That can be translated to the following DWARF expression:
  //
  //  DWARF OP             ; Description                               ; Stack op ; Stack
  //  -------------------- ; ----------------------------------------- ; -------- ; -----
  //  bregx(fp, -\offset)  ; offset from fp to the start of the spill  ; (push a) ; a
  //  bregx(vlenb, 0)      ; vlenb: the size in bytes of a vector reg  ; (push b) ; ab
  //  lit0+(31 - \n)       ; lit(31 - \n)                              ; (push c) ; abc
  //  mul                  ; (vlenb * \n) = offset from start of spill ; (pop cb) ; ad
  //  minus                ; (cfa - offset) - (vlenb * n)              ; (pop da) ; (a-d)
  //
  // `n` is the register number (.e.g., v<n>).
  // `offset_nbytes` is number of bytes of the SLEB128 encoded offset.
  // `offset_sleb_bytes` are the bytes of the SLEB128 encoded offset.
  .macro v_spill_cfi.escape n:req, offset_nbytes:req, offset_sleb_bytes:vararg
    .if DWARF_REGNO_V(\n) > 128 || DWARF_REGNO_V(\n) < 96
      .error "DWARF register number for vector registers must be in the range [96-128)"
    .endif

    .cfi_escape DW_CFA_expression, DWARF_REGNO_V(\n), (\offset_nbytes) + 8, \
                DW_OP_breg(8) /* fp = x8 */, \offset_sleb_bytes, \
                DW_OP_bregx, ULEB128_2BYTE(DWARF_REGNO_VLENB), 0, \
                DW_OP_lit((31-\n)), DW_OP_mul, DW_OP_minus
  .endm

  .macro v_cfi_spill_1reg_1byte value:req, suff:vararg
      v_spill_cfi.escape \stuff, 1, SLEB128_1BYTE(\value)
  .endm

  .macro v_cfi_spill_1reg_2byte value:req, stuff:vararg
      v_spill_cfi.escape \stuff, 2, SLEB128_2BYTE(\value)
  .endm

  // Emit cfi_* directives for the spill of a single vector register, v<n>.
  // Assumes `offset` is within the range of one-byte or two-byte SLEB_128.
  .macro v_cfi_spill_1reg n:req offset:req
    .sleb128.size_dispatch v_cfi_spill_1reg_1byte, v_cfi_spill_1reg_2byte, \
                           \offset, \n
  .endm

  // Emit cfi_* directives for spilling vector registers in group v<n>.
  .macro v_cfi_spill_group n:req, offset:req
    on_v_cfi_group_v\n v_cfi_spill_1reg, \offset
  .endm

  // Helper to allow passing some optional arguments.
  .macro op.with.optional.args op:req, n:req, args:vararg
    .ifb \args
      \op \n
    .else
      \op \n, \args
    .endif
  .endm

  // The following four macros operate on groups of 8 vector registers that are
  // compatible with vs8r/vl8r (v0-v7, v8-v15, v16-v23, v24-v31).
  .macro on_v_cfi_group_v0 op:req, args:vararg
    .irp n,0,1,2,3,4,5,6,7
       op.with.optional.args \op, \n, \args
    .endr
  .endm

  .macro on_v_cfi_group_v8 op:req, args:vararg
    .irp n,8,9,10,11,12,13,14,15
       op.with.optional.args \op, \n, \args
    .endr
  .endm

  .macro on_v_cfi_group_v16 op:req, args:vararg
    .irp n,16,17,18,19,20,21,22,23
       op.with.optional.args \op, \n, \args
    .endr
  .endm

  .macro on_v_cfi_group_v24 op:req, args:vararg
    .irp n,24,25,26,27,28,29,30,31
       op.with.optional.args \op, \n, \args
    .endr
  .endm

  // Helper to make .cfi_same_value compatible w/ on_v_cfi_group_v<n>.
  .macro v_cfi_same_value n:req
    .cfi_same_value v\n
  .endm

  // Reload a group of 8 vector registers for group n.
  //
  // `n` is the register number (e.g. v<n>) of the register group.
  // `reg` is the register used to adjust the stack by (8 * VLENB).
  .macro v_cfi_reload_vector_group n:req, reg:req
    // Restore 8 registers [vn, vn+7] from the stack for each group of V
    // registers (v0-v7,v8-v15,v16-v23,v24-v31).
    //
    // vl8r.v only accepts v0,v8,v16,v24 as the register argument.
    vl8r.v v\n, (sp)
    on_v_cfi_group_v\n v_cfi_same_value

    // Adjust the SP by 8*VLENB bytes.
    add sp, sp, \reg
  .endm

  // Reloads all vector registers in groups of 8 using vl8r.v.
  //
  // `reg` is the output register for compute_vector_group_size, and will hold
  // the size in bytes of a block of 8 V registers. It is used for stack
  // adjustment.
  .macro reload_vector_regs reg:req
  #ifdef __riscv_v
    // Because RISC-V supports scalable vectors, VLENB is not a fixed
    // size, so we need to get its value before trying to spill anything.
    // `reg` will hold the size in bytes of a block of 8 V registers.
    compute_vector_group_size \reg

    // vl8r.v only accepts v0,v8,v16,v24 as the register argument, so use `n`
    // that correspond to those register numbers.
    .irp n,ALL_V_GROUP_RELOAD_ORDER
      v_cfi_reload_vector_group \n, \reg
    .endr
  #endif
  .endm

  // a0 is the offset and a1 is the module ID.
  // Those are the arguments to __tls_get_new.
  .cfi_restore_state
  .prologue.shadow_call_sp t0

  // We need NUM_SAVED_REGS slots for scalar registers.
  add sp, sp, -(NUM_SAVED_REGS * 8)
  on_saved_regs spill
  spill_vector_regs (NUM_SAVED_REGS + PROLOGUE_SLOTS), a3

  // The first argument to __tls_get_new is the offset value as set for a
  // DTPREL64 relocation, which subtracts this bias. However, we save an
  // instruction in the fast path by not including the offset in the TLSDESC
  // offset slot.
  addi a0, a0, -(DTP_OFFSET)
  call __tls_get_new
  reload_vector_regs a3
  on_saved_regs reload
  add sp, sp, (NUM_SAVED_REGS * 8)

  .epilogue.shadow_call_sp t0
  j .Lret

.end_function
