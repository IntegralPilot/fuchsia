// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <stdio.h>

#include <arch/crashlog.h>

void arch_render_crashlog_registers(FILE& target, const crashlog_regs_t& regs) {
  if (!regs.iframe) {
    fprintf(&target, "missing");
    return;
  }
  fprintf(&target,
          // clang-format off
            "          CS: %#18" PRIx64 "\n"
            "          RIP: %#18" PRIx64 "\n"
            "          EFL: %#18" PRIx64 "\n"
            "          CR2: %#18lx\n"
            "          RAX: %#18" PRIx64 "\n"
            "          RBX: %#18" PRIx64 "\n"
            "          RCX: %#18" PRIx64 "\n"
            "          RDX: %#18" PRIx64 "\n"
            "          RSI: %#18" PRIx64 "\n"
            "          RDI: %#18" PRIx64 "\n"
            "          RBP: %#18" PRIx64 "\n"
            "          RSP: %#18" PRIx64 "\n"
            "           R8: %#18" PRIx64 "\n"
            "           R9: %#18" PRIx64 "\n"
            "          R10: %#18" PRIx64 "\n"
            "          R11: %#18" PRIx64 "\n"
            "          R12: %#18" PRIx64 "\n"
            "          R13: %#18" PRIx64 "\n"
            "          R14: %#18" PRIx64 "\n"
            "          R15: %#18" PRIx64 "\n"
            "       vector: %#18" PRIx64 "\n"
            "         errc: %#18" PRIx64 "\n"
            "       fsbase: %#18" PRIx64 "\n"
            "       gsbase: %#18" PRIx64 "\n"
            "swapgs gsbase: %#18" PRIx64 "\n"
            "\n",
          // clang-format on
          regs.iframe->cs, regs.iframe->ip, regs.iframe->flags, regs.cr2, regs.iframe->rax,
          regs.iframe->rbx, regs.iframe->rcx, regs.iframe->rdx, regs.iframe->rsi, regs.iframe->rdi,
          regs.iframe->rbp, regs.iframe->user_sp, regs.iframe->r8, regs.iframe->r9,
          regs.iframe->r10, regs.iframe->r11, regs.iframe->r12, regs.iframe->r13, regs.iframe->r14,
          regs.iframe->r15, regs.iframe->vector, regs.iframe->err_code, regs.fsbase, regs.gsbase,
          regs.swapgs_gsbase);
}
