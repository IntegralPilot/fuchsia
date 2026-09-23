// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <inspector/inspector.h>

__EXPORT zx_status_t inspector_read_general_regs(zx_handle_t thread,
                                                 zx_thread_state_general_regs_t* regs) {
  auto status = zx_thread_read_state(thread, ZX_THREAD_STATE_GENERAL_REGS, regs, sizeof(*regs));
  if (status < 0) {
    fprintf(stderr, "inspector: unable to access general regs: %s\n", zx_status_get_string(status));
    return status;
  }
  return ZX_OK;
}

#if defined(__x86_64__)

__EXPORT void inspector_print_general_regs(FILE* f, const zx_thread_state_general_regs_t* regs,
                                           const inspector_excp_data_t* excp_data) {
  fprintf(f, " CS:  %#18llx RIP: %#18" PRIx64 " EFL: %#18" PRIx64, 0ull, regs->rip, regs->rflags);
  if (excp_data) {
    fprintf(f, " CR2: %#18" PRIx64, excp_data->cr2);
  }
  fprintf(f, "\n");
  fprintf(f, " RAX: %#18" PRIx64 " RBX: %#18" PRIx64 " RCX: %#18" PRIx64 " RDX: %#18" PRIx64 "\n",
          regs->rax, regs->rbx, regs->rcx, regs->rdx);
  fprintf(f, " RSI: %#18" PRIx64 " RDI: %#18" PRIx64 " RBP: %#18" PRIx64 " RSP: %#18" PRIx64 "\n",
          regs->rsi, regs->rdi, regs->rbp, regs->rsp);
  fprintf(f, "  R8: %#18" PRIx64 "  R9: %#18" PRIx64 " R10: %#18" PRIx64 " R11: %#18" PRIx64 "\n",
          regs->r8, regs->r9, regs->r10, regs->r11);
  fprintf(f, " R12: %#18" PRIx64 " R13: %#18" PRIx64 " R14: %#18" PRIx64 " R15: %#18" PRIx64 "\n",
          regs->r12, regs->r13, regs->r14, regs->r15);
  fprintf(f, " fs.base: %#18" PRIx64 " gs.base: %#18" PRIx64 "\n", regs->fs_base, regs->gs_base);
  if (excp_data) {
    // These are padded differently to line up with the labels above given the
    // specific lengths of the "vector:" and "errc:" labels.
    fprintf(f, " vector: %#15" PRIx64 " errc: %#17" PRIx64 "\n", excp_data->vector,
            excp_data->err_code);
  }
}

#elif defined(__aarch64__)

__EXPORT void inspector_print_general_regs(FILE* f, const zx_thread_state_general_regs_t* regs,
                                           const inspector_excp_data_t* excp_data) {
  fprintf(f, " x0  %#18" PRIx64 " x1  %#18" PRIx64 " x2  %#18" PRIx64 " x3  %#18" PRIx64 "\n",
          regs->r[0], regs->r[1], regs->r[2], regs->r[3]);
  fprintf(f, " x4  %#18" PRIx64 " x5  %#18" PRIx64 " x6  %#18" PRIx64 " x7  %#18" PRIx64 "\n",
          regs->r[4], regs->r[5], regs->r[6], regs->r[7]);
  fprintf(f, " x8  %#18" PRIx64 " x9  %#18" PRIx64 " x10 %#18" PRIx64 " x11 %#18" PRIx64 "\n",
          regs->r[8], regs->r[9], regs->r[10], regs->r[11]);
  fprintf(f, " x12 %#18" PRIx64 " x13 %#18" PRIx64 " x14 %#18" PRIx64 " x15 %#18" PRIx64 "\n",
          regs->r[12], regs->r[13], regs->r[14], regs->r[15]);
  fprintf(f, " x16 %#18" PRIx64 " x17 %#18" PRIx64 " x18 %#18" PRIx64 " x19 %#18" PRIx64 "\n",
          regs->r[16], regs->r[17], regs->r[18], regs->r[19]);
  fprintf(f, " x20 %#18" PRIx64 " x21 %#18" PRIx64 " x22 %#18" PRIx64 " x23 %#18" PRIx64 "\n",
          regs->r[20], regs->r[21], regs->r[22], regs->r[23]);
  fprintf(f, " x24 %#18" PRIx64 " x25 %#18" PRIx64 " x26 %#18" PRIx64 " x27 %#18" PRIx64 "\n",
          regs->r[24], regs->r[25], regs->r[26], regs->r[27]);
  fprintf(f, " x28 %#18" PRIx64 " x29 %#18" PRIx64 " lr  %#18" PRIx64 " sp  %#18" PRIx64 "\n",
          regs->r[28], regs->r[29], regs->lr, regs->sp);
  fprintf(f, " pc  %#18" PRIx64 " psr %#18" PRIx64, regs->pc, regs->cpsr);
  if (excp_data) {
    fprintf(f, " far %#18" PRIx64 " esr %#18" PRIx32, excp_data->far, excp_data->esr);
  }
  fprintf(f, "\n");
}

#elif defined(__riscv)

__EXPORT void inspector_print_general_regs(FILE* f, const zx_thread_state_general_regs_t* regs,
                                           const inspector_excp_data_t* excp_data) {
  fprintf(f, " pc  %#18" PRIx64 " ra  %#18" PRIx64 " sp  %#18" PRIx64 " gp  %#18" PRIx64 "\n",
          regs->pc, regs->ra, regs->sp, regs->gp);
  fprintf(f, " tp  %#18" PRIx64 " t0  %#18" PRIx64 " t1  %#18" PRIx64 " t2  %#18" PRIx64 "\n",
          regs->tp, regs->t0, regs->t1, regs->t2);
  fprintf(f, " s0  %#18" PRIx64 " s1  %#18" PRIx64 " a0  %#18" PRIx64 " a1  %#18" PRIx64 "\n",
          regs->s0, regs->s1, regs->a0, regs->a1);
  fprintf(f, " a2  %#18" PRIx64 " a3  %#18" PRIx64 " a4  %#18" PRIx64 " a5  %#18" PRIx64 "\n",
          regs->a2, regs->a3, regs->a4, regs->a5);
  fprintf(f, " a6  %#18" PRIx64 " a7  %#18" PRIx64 " s2  %#18" PRIx64 " s3  %#18" PRIx64 "\n",
          regs->a6, regs->a7, regs->s2, regs->s3);
  fprintf(f, " s4  %#18" PRIx64 " s5  %#18" PRIx64 " s6  %#18" PRIx64 " s7  %#18" PRIx64 "\n",
          regs->s4, regs->s5, regs->s6, regs->s7);
  fprintf(f, " s8  %#18" PRIx64 " s9  %#18" PRIx64 " s10 %#18" PRIx64 " s11 %#18" PRIx64 "\n",
          regs->s8, regs->s9, regs->s10, regs->s11);
  fprintf(f, " t3  %#18" PRIx64 " t4  %#18" PRIx64 " t5  %#18" PRIx64 " t6  %#18" PRIx64 "\n",
          regs->t3, regs->t4, regs->t5, regs->t6);
  if (excp_data) {
    fprintf(f, " cause %#16" PRIx64 " tval %#17" PRIx64 "\n", excp_data->cause, excp_data->tval);
  }
}

#else  // unsupported arch

__EXPORT void inspector_print_general_regs(const zx_thread_state_general_regs_t* regs,
                                           const inspector_excp_data_t* excp_data) {
  printf("unsupported architecture\n");
}

#endif
