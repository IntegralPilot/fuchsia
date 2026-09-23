// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/arch/arm64/system.h>
#include <stdio.h>

#include <arch/arm64/mmu.h>
#include <arch/crashlog.h>
#include <arch/vm.h>
#include <ktl/atomic.h>

namespace {
ktl::atomic<bool> gSuppressELRDump{false};
}

void arch_render_crashlog_registers(FILE& target, const crashlog_regs_t& regs) {
  if (!regs.iframe) {
    fprintf(&target, "missing\n");
    return;
  }

  fprintf(&target,
          // clang-format off
          "         x0: %#18" PRIx64 "\n"
          "         x1: %#18" PRIx64 "\n"
          "         x2: %#18" PRIx64 "\n"
          "         x3: %#18" PRIx64 "\n"
          "         x4: %#18" PRIx64 "\n"
          "         x5: %#18" PRIx64 "\n"
          "         x6: %#18" PRIx64 "\n"
          "         x7: %#18" PRIx64 "\n"
          "         x8: %#18" PRIx64 "\n"
          "         x9: %#18" PRIx64 "\n"
          "        x10: %#18" PRIx64 "\n"
          "        x11: %#18" PRIx64 "\n"
          "        x12: %#18" PRIx64 "\n"
          "        x13: %#18" PRIx64 "\n"
          "        x14: %#18" PRIx64 "\n"
          "        x15: %#18" PRIx64 "\n"
          "        x16: %#18" PRIx64 "\n"
          "        x17: %#18" PRIx64 "\n"
          "        x18: %#18" PRIx64 "\n"
          "        x19: %#18" PRIx64 "\n"
          "        x20: %#18" PRIx64 "\n"
          "        x21: %#18" PRIx64 "\n"
          "        x22: %#18" PRIx64 "\n"
          "        x23: %#18" PRIx64 "\n"
          "        x24: %#18" PRIx64 "\n"
          "        x25: %#18" PRIx64 "\n"
          "        x26: %#18" PRIx64 "\n"
          "        x27: %#18" PRIx64 "\n"
          "        x28: %#18" PRIx64 "\n"
          "        x29: %#18" PRIx64 "\n"
          "         lr: %#18" PRIx64 "\n"
          "        usp: %#18" PRIx64 "\n"
          "        elr: %#18" PRIx64 "\n"
          "       spsr: %#18" PRIx64 "\n"
          "        esr: %#18" PRIx32 "\n"
          "        far: %#18" PRIx64 "\n"
          "  tpidr_el0: %#18" PRIx64 "\n"
          "  tpidr_el1: %#18" PRIx64 "\n"
          "tpidrro_el0: %#18" PRIx64 "\n"
          "\n",
          // clang-format on
          regs.iframe->r[0], regs.iframe->r[1], regs.iframe->r[2], regs.iframe->r[3],
          regs.iframe->r[4], regs.iframe->r[5], regs.iframe->r[6], regs.iframe->r[7],
          regs.iframe->r[8], regs.iframe->r[9], regs.iframe->r[10], regs.iframe->r[11],
          regs.iframe->r[12], regs.iframe->r[13], regs.iframe->r[14], regs.iframe->r[15],
          regs.iframe->r[16], regs.iframe->r[17], regs.iframe->r[18], regs.iframe->r[19],
          regs.iframe->r[20], regs.iframe->r[21], regs.iframe->r[22], regs.iframe->r[23],
          regs.iframe->r[24], regs.iframe->r[25], regs.iframe->r[26], regs.iframe->r[27],
          regs.iframe->r[28], regs.iframe->r[29], regs.iframe->lr, regs.iframe->usp,
          regs.iframe->elr, regs.iframe->spsr, regs.esr, regs.far, regs.tpidr_el0, regs.tpidr_el1,
          regs.tpidrro_el0);

  // Depending on the exception class, conditionally make an attempt to print
  // out some of the memory immediately surrounding the ELR.  Particularly for
  // an UnknownException, we would like to know whether or not the exception
  // was taken because we happened to fetch an instruction which had been
  // corrupted (somehow).
  //
  // Note, do not attempt to do this if we have even attempted to read from
  // this memory before.  We absolutely do not want to be in a situation where
  // we attempt to read the memory that the system faulted on, and we just
  // fault again for some unknown reason, putting us into an infinite fault
  // recursion situation.
  if (gSuppressELRDump.load(ktl::memory_order_seq_cst)) {
    fprintf(&target, "ELR region dumping suppressed\n");
    return;
  }

  using EC = arch::ArmExceptionSyndromeRegister::ExceptionClass;
  const auto esr_reg = arch::ArmExceptionSyndromeRegister::Get().FromValue(regs.esr);

  switch (esr_reg.ec()) {
    case EC::kUnknown:
    case EC::kDataAbortSameEl:
      // Set our suppression bit before we attempt any accesses.
      gSuppressELRDump.store(true, ktl::memory_order_seq_cst);

      if (regs.iframe->elr & 0x3) {
        fprintf(&target, "no ELR memory dump, misaligned ELR\n\n");
      } else {
        for (int offset = -4; offset <= 4; offset += 4) {
          paddr_t paddr{0};
          vaddr_t vaddr{0};

          if (add_overflow(regs.iframe->elr, offset, &vaddr)) {
            fprintf(&target, "ELR[%s%d] = over/underflow\n", (offset < 0) ? "" : "+", offset);
            continue;
          }

          if (!is_kernel_address(vaddr)) {
            fprintf(&target, "ELR[%s%d] = non-kernel\n", (offset < 0) ? "" : "+", offset);
          } else if (arm64_mmu_translate(vaddr, &paddr, false /* user */, false /* write */) !=
                     ZX_OK) {
            fprintf(&target, "ELR[%s%d] = bad addr\n", (offset < 0) ? "" : "+", offset);
          } else {
            uint32_t word = ktl::atomic_ref(reinterpret_cast<uint32_t*>(vaddr)[0])
                                .load(ktl::memory_order_relaxed);
            const bool in_text = ((vaddr >= reinterpret_cast<vaddr_t>(__code_start)) &&
                                  (vaddr < reinterpret_cast<vaddr_t>(__code_end)));
            fprintf(&target, "ELR[%s%d] = 0x%08x%s\n", (offset < 0) ? "" : "+", offset, word,
                    in_text ? "" : " (not in .text)");
          }
        }
        fprintf(&target, "\n");
      }
      break;

    default:
      break;
  };
}
