// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <arch/riscv64.h>

.macro on_csw_regs macro
  \macro ra, CONTEXT_SWITCH_FRAME_OFFSET_RA
  \macro tp, CONTEXT_SWITCH_FRAME_OFFSET_TP
  \macro gp, CONTEXT_SWITCH_FRAME_OFFSET_GP
  \macro s0, CONTEXT_SWITCH_FRAME_OFFSET_S(0)
  \macro s1, CONTEXT_SWITCH_FRAME_OFFSET_S(1)
  \macro s2, CONTEXT_SWITCH_FRAME_OFFSET_S(2)
  \macro s3, CONTEXT_SWITCH_FRAME_OFFSET_S(3)
  \macro s4, CONTEXT_SWITCH_FRAME_OFFSET_S(4)
  \macro s5, CONTEXT_SWITCH_FRAME_OFFSET_S(5)
  \macro s6, CONTEXT_SWITCH_FRAME_OFFSET_S(6)
  \macro s7, CONTEXT_SWITCH_FRAME_OFFSET_S(7)
  \macro s8, CONTEXT_SWITCH_FRAME_OFFSET_S(8)
  \macro s9, CONTEXT_SWITCH_FRAME_OFFSET_S(9)
  \macro s10, CONTEXT_SWITCH_FRAME_OFFSET_S(10)
.endm

// void riscv64_context_switch(vaddr_t *old_sp, vaddr_t new_sp);
.function riscv64_context_switch, global
  // Save old frame.
  add.sp -SIZEOF_CONTEXT_SWITCH_FRAME
  .llvm.stack_size SIZEOF_CONTEXT_SWITCH_FRAME

  on_csw_regs sd.spill

  // Save old sp.
  sd sp, (a0)

  // Load new sp.  From this point, the CFI is exactly the same but now
  // it's describing the switched-to frame rather than the switched-from.
  mv sp, a1

  // Restore new frame.
  on_csw_regs ld.reload

  add.sp SIZEOF_CONTEXT_SWITCH_FRAME
  ret
.end_function
