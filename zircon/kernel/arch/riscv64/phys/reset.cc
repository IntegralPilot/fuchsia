// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/riscv64/sbi-call.h>

#include <phys/main.h>

void ArchPanicReset() {
  // Don't go back into our own handlers when we crash.  Probably trying to
  // crash this way just loops forever, but at least it won't reenter our
  // exception code and confuse things further.
  __asm__ volatile("csrw stvec, zero");

  while (true) {
    arch::RiscvSbi::SystemReset(arch::RiscvSbiResetType::kWarmReboot,
                                arch::RiscvSbiResetReason::kSystemFailure);
  }
}
