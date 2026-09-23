// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/boot-options/boot-options.h>
#include <lib/console.h>
#include <zircon/assert.h>

#include <ktl/span.h>
#include <ktl/string_view.h>
#include <vm/physmap.h>
#include <vm/vm_aspace.h>

#include <ktl/enforce.h>

namespace {

// Note that using this can introduce data races on the member variables.
int Set(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc < 2) {
    printf("Usage: %s <key>[=<value>]...\n", argv[0].str);
    return -1;
  }

  // BootOptions::Get() returns a pointer to const and can actually be a
  // read-only page mapping.  A mutable pointer is needed to set options, so
  // find the physical page and use it through the physmap.  Changing the
  // BootOptions object is inherently dangerous and racy, and should only be
  // done in a development context.
  const BootOptions* readonly_boot_options = BootOptions::Get();
  vaddr_t boot_options_vaddr = reinterpret_cast<uintptr_t>(readonly_boot_options);
  paddr_t boot_options_paddr;
  zx_status_t status = VmAspace::kernel_aspace()->arch_aspace().Query(  //
      boot_options_vaddr, &boot_options_paddr, nullptr);
  ZX_ASSERT_MSG(status == ZX_OK, "Cannot find BootOptions %p in aspace!  %d",  //
                readonly_boot_options, status);
  auto* boot_options = static_cast<BootOptions*>(paddr_to_physmap(boot_options_paddr));

  for (const auto& arg : ktl::span(argv, argc).subspan(1)) {
    boot_options->SetMany(arg.str, stdout);
  }

  return 0;
}

int Show(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc > 1) {
    int result = 0;
    for (const auto& arg : ktl::span(argv, argc).subspan(1)) {
      result |= BootOptions::Get()->Show(ktl::string_view{arg.str});
    }
    return result;
  }

  BootOptions::Get()->Show();
  return 0;
}

}  // namespace

STATIC_COMMAND_START
STATIC_COMMAND("setopt", "Set boot options (as from kernel cmdline)", Set)
STATIC_COMMAND("showopt", "Show boot options (from kernel cmdline)", Show)
STATIC_COMMAND_END(options)
