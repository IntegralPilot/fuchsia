// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/arm64/system.h>
#include <lib/arch/cache.h>

#include <phys/boot-zbi.h>

void BootZbi::ZbiBoot(uintptr_t entry, void* data) const {
  // The ZBI protocol requires that any data cache lines backing the kernel
  // memory image are cleaned (that is, including the "reserve" memory)
  // allowing the image to be present for its own direct memory access.
  arch::CleanDataCacheRange(static_cast<uintptr_t>(KernelLoadAddress()), KernelMemorySize());

  // The ZBI protocol requires that any instruction cache lines backing the
  // kernel load image are invalidated. We can exclude any "reserve" memory in
  // this as we don't expect that to contain code (and if it does, then
  // coherence around that range should be managed by the kernel itself).
  //
  // While we could more simply issue `ic iallu` to invalidate the whole
  // instruction cache in a single instruction, we choose not to. This is a
  // reference implementation of the ZBI protocol and we want platform
  // exercise of out ZBI kernel code in the context of a minimally compliant
  // bootloader.
  arch::InvalidateInstructionCacheRange(static_cast<uintptr_t>(KernelLoadAddress()),
                                        KernelLoadSize());

  // Precalculate the SCTLR_ELx value that will be installed in assembly below.
  uint64_t is_el1 = arch::ArmCurrentEl::Read().el() == 1 ? 1 : 0;
  uint64_t sctlr;  // Disable the MMU, and the instruction and data caches.
  if (is_el1) {
    sctlr = arch::ArmSctlrEl1::Read().set_m(false).set_i(false).set_c(false).reg_value();
  } else {
    sctlr = arch::ArmSctlrEl2::Read().set_m(false).set_i(false).set_c(false).reg_value();
  }

  // Before turning off the caches with the SCTLR_ELx change, make sure the
  // code right at the PC here is fully cleaned to main memory.  If this code
  // is running in the context of a ZBI or Linux kernel (e.g., a boot shim),
  // then this isn't strictly necessary as we have protocol guarantees that we
  // were loaded with this instruction memory clean (and we weren't likely to
  // have modified ourselves).  But better to be maximally defensive when it
  // comes to cache coherency.  arch::CleanDataCacheRange() has to be called in
  // the same assembly block since the compiler can always decide to duplicate
  // the code and it must materialize the local PC range for cache cleaning.
  //
  // Before handing off, clear the stack and frame pointers and the link
  // register so no misleading breadcrumbs are left.
  __asm__ volatile(
      R"""(
        adr x0, .L.ZbiBoot.%=.mmu_possibly_off
        mov x1, #.L.ZbiBoot.%=.end - .L.ZbiBoot.%=.mmu_possibly_off
        bl CleanDataCacheRange

        cbnz %[is_el1], .L.ZbiBoot.%=.disable_el1
      .L.ZbiBoot.%=.disable_el2:
        msr sctlr_el2, %[sctlr]
      .L.ZbiBoot.%=.mmu_possibly_off:
        b .L.ZbiBoot.%=.mmu_off
      .L.ZbiBoot.%=.disable_el1:
        msr sctlr_el1, %[sctlr]
      .L.ZbiBoot.%=.mmu_off:
        isb

        mov x29, xzr
        mov x30, xzr
        mov sp, x29

        mov x0, %[zbi]
        br %[entry]
      .L.ZbiBoot.%=.end:
      )"""
      :
      : [entry] "r"(entry),    //
        [is_el1] "r"(is_el1),  //
        [sctlr] "r"(sctlr),    //
        [zbi] "r"(data)

      // CleanDataCacheRange is in assembly and only uses a few registers.
      // But just to keep it simple, mark all the call-clobbered registers
      // as clobbered anyway so it doesn't matter how it's implemented.
      : "x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7",  //
        "x8", "x9", "x10", "x11", "x12", "x13", "x14", "x15", "x16", "x17",
        // The compiler gets unhappy if x29 (fp) is a clobber.  It's never
        // going to be the register used for %[entry] anyway.  The memory
        // clobber is probably unnecessary, but it expresses that this
        // constitutes access to the memory kernel and zbi point to.
        "x30", "memory");
  __builtin_unreachable();
}
