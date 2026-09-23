// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <phys/arch/arch-phys-info.h>
#include <phys/boot-zbi.h>

#include "riscv64.h"

void BootZbi::ZbiBoot(uintptr_t entry, void* data) const {
  // Clear the stack and frame pointers and the link register so no misleading
  // breadcrumbs are left.
  __asm__ volatile(
      R"""(
      csrw satp, zero
      mv a0, %[hartid]
      mv a1, %[zbi]
      mv fp, zero
      mv ra, zero
      mv gp, zero
      mv tp, zero
      mv sp, zero
      jr %[entry]
      )"""
      :
      : [entry] "r"(entry), [hartid] "r"(gArchPhysInfo->boot_hart_id), [zbi] "r"(data)
      // The compiler gets unhappy if s0 (fp) is a clobber.  It's never going
      // to be the register used for %[entry] anyway.  The memory clobber is
      // probably unnecessary, but it expresses that this constitutes access to
      // the memory kernel and zbi point to.
      : "a0", "a1", "memory");
  __builtin_unreachable();
}
