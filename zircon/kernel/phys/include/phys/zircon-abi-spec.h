// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ZIRCON_ABI_SPEC_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ZIRCON_ABI_SPEC_H_

#include <lib/arch/asm.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/assert.h>

#include <ktl/byte.h>
#include <ktl/limits.h>
#include <ktl/string_view.h>
#include <ktl/type_traits.h>

#include "handoff-ptr.h"

struct BootConstants;  // <phys/boot-constants.h>
struct PhysHandoff;    // <phys/handoff.h>
struct ZirconAbiSpec;  // Defined below.

extern "C" {

// This is defined in RODATA (or RELRO) by the kernel as the link-time e_entry
// address; physboot finds it in the image after loading and relocation.
extern constinit const ZirconAbiSpec kZirconAbiSpec;

// This is the entry point function for the ELF kernel.  This symbol is not
// used directly as the ELF entry point (Ehdr::e_entry).  Instead e_entry
// points to the kZirconAbiSpec struct and its .entry is the real entry point.
[[noreturn, clang::cfi_unchecked_callee]] void PhysbootHandoff(PhysHandoff* handoff);

// See <phys/boot-constants.h>.  This is "defined" in the kernel, so it resides
// in the kernel's RODATA, but it's filled in by physboot before kernel entry.
extern constinit const BootConstants kBootConstants;

// This always starts false and only exists to be set manually from the
// debugger when using the `kernel.debug.boot-spin` boot option; when that's
// not enabled, nothing ever looks at this.
extern constinit volatile bool gDebugBootSpinReady;

}  // extern "C"

// The kernel ABI specifications needed at the phys stage to properly prepare
// handoff.  The contents are initialized in the definition of kZirconAbiSpec;
// physboot reads that directly after loading and relocating the kernel image.
// To make it simple to find
struct ZirconAbiSpec {
  struct Stack {
    template <size_t PageSize>
    constexpr void AssertValid() const {
      ZX_ASSERT(size_bytes == (size_bytes & -PageSize));
      ZX_ASSERT(lower_guard_size_bytes == (lower_guard_size_bytes & -PageSize));
      ZX_ASSERT(upper_guard_size_bytes == (upper_guard_size_bytes & -PageSize));
    }

    // The size of the stack. Must be page-aligned.
    uint32_t size_bytes = 0;

    // The size of the unmapped 'guard' region to ensure lies below the mapped
    // stack. Must be page-aligned.
    uint32_t lower_guard_size_bytes = 0;

    // The size of the unmapped 'guard' region to ensure lies above the mapped
    // stack. Must be page-aligned.
    uint32_t upper_guard_size_bytes = 0;
  };

  // Validates the ABI spec for a given system paging configuration. The paging
  // configuration is supplied through the constants of page size and the
  // minimal, hardware-prescribed kernel virtual address space base address.
  template <size_t PageSize, uint64_t HwKernelAspaceBase>
  constexpr void AssertValid() const {
    ZX_ASSERT(magic == kMagic);
    machine_stack.AssertValid<PageSize>();
    shadow_call_stack.AssertValid<PageSize>();
    unsafe_stack.AssertValid<PageSize>();
    ZX_ASSERT(entry);
    ZX_ASSERT(boot_constants);
    ZX_ASSERT(kernel_aspace_base % PageSize == 0);
    ZX_ASSERT(kernel_aspace_base >= HwKernelAspaceBase);
    ZX_ASSERT(kernel_aspace_size % PageSize == 0);
    ZX_ASSERT(kernel_aspace_size > 0);
    ZX_ASSERT(kernel_aspace_size - 1 <= ktl::numeric_limits<uint64_t>::max() - kernel_aspace_base);
  }

  // This never changes and is just checked by assertions.
  static constexpr uint64_t kMagic = 0xfeed'f00d'bad'4'face;
  const uint64_t magic = kMagic;

  // These instruct physboot what kinds of stack to set up for the kernel.
  Stack machine_stack;
  Stack shadow_call_stack;
  Stack unsafe_stack;

  // This instructs physboot where to enter the kernel.  The kernel's first PC
  // expects the ABI of the normal C++ function signature of PhysbootHandoff,
  // but the call is made at the top of the stack with a zero return address,
  // and the function must not return.  The member is declared const so that
  // its default initializer still applies with designated initializers.
  decltype(PhysbootHandoff)* const entry = PhysbootHandoff;

  // This instructs physboot where to initialize the BootConstants before the
  // kernel starts.  Then the kernel just accesses kBootConstants directly.
  PhysHandoffKernelImagePtr<const BootConstants> boot_constants;

  // The chosen, page-aligned, dimensions of the the kernel's address space,
  // which may be more constrained then maximizing, hardware-prescribed values
  // within a given system paging configuration.
  uint64_t kernel_aspace_base = 0;
  uint64_t kernel_aspace_size = 0;

  // This tells physboot where the gDebugBootSpinReady variable sits.  This
  // variable is never actually touched by kernel code to implement the
  // kernel.debug.boot-spin boot option, but physboot prints out its runtime
  // address to aid in setting it manually when debugging.
  PhysHandoffKernelImagePtr<volatile bool> debug_boot_spin_ready;
  static constexpr ktl::string_view kDebugBootSpinVariable = "gDebugBootSpinReady";

  // This tells physboot the (relocated) address of the .text section, which is
  // what GDB's `add-symbol-file` needs to be fed.
  PhysHandoffKernelImagePtr<const ktl::byte> text_start;

  // If set, this overrides the kernel.bypass-debuglog boot option to always on
  // even if omitted or specified in the negative on the kernel command line.
  // We need to preserve the compile time switch (ENABLE_KERNEL_LL_DEBUG), even
  // though we add a kernel cmdline (kernel.bypass-debuglog), to bypass the
  // debuglog.  This is to allow very early prints in the kernel to go to the
  // serial console immediately.
  bool always_bypass_debuglog =
#if ENABLE_KERNEL_LL_DEBUG
      true
#else
      false
#endif
      ;

  // This is just used in physboot's version info text rendering.
  uint8_t kernel_debug_level = LK_DEBUGLEVEL;
};
static_assert(ktl::is_trivially_destructible_v<ZirconAbiSpec>);

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ZIRCON_ABI_SPEC_H_
