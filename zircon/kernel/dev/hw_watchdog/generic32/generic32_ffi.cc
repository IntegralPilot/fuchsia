// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/dev/hw_watchdog/generic32/hw_watchdog.cc

#include <lib/boot-options/boot-options.h>
#include <lib/console.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/vm.h>
#include <dev/hw_watchdog/generic32/init.h>
#include <kernel/ffi.h>
#include <kernel/spinlock.h>
#include <kernel/timer.h>
#include <vm/physmap.h>

#if defined(__aarch64__)
#include <arch/arm64/periphmap.h>
#endif

extern "C" {

static_assert(sizeof(zbi_dcfg_generic32_watchdog_t) == 64,
              "zbi_dcfg_generic32_watchdog_t size mismatch");
static_assert(alignof(zbi_dcfg_generic32_watchdog_t) == 8,
              "zbi_dcfg_generic32_watchdog_t alignment mismatch");

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_watchdog_translate_paddr(uint64_t* paddr);
// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_watchdog_is_force_disabled_cmdline();

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_watchdog_translate_paddr(uint64_t* paddr) {
  if (*paddr == 0 || is_kernel_address(*paddr)) {
    return true;
  }
#if defined(__aarch64__)
  *paddr = periph_paddr_to_vaddr(static_cast<paddr_t>(*paddr));
#elif defined(__riscv)
  *paddr = reinterpret_cast<uint64_t>(paddr_to_physmap(static_cast<paddr_t>(*paddr)));
#else
#error handle this architecture
#endif
  return (*paddr != 0);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_watchdog_is_force_disabled_cmdline() {
  return BootOptions::Get()->force_watchdog_disabled;
}

}  // extern "C"
