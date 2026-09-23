// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "legacy-boot-shim.h"

#include <lib/acpi_lite.h>
#include <lib/boot-shim/boot-shim.h>
#include <lib/fit/defer.h>
#include <lib/memalloc/pool.h>
#include <lib/uart/all.h>
#include <lib/zbi-format/zbi.h>
#include <stdlib.h>

#include <ktl/optional.h>
#include <ktl/span.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/boot-zbi.h>
#include <phys/main.h>
#include <phys/stdio.h>
#include <phys/symbolize.h>
#include <phys/uart-console.h>
#include <phys/zbi.h>

#include "stdout.h"

#include <ktl/enforce.h>

void PhysMain(void* ptr, arch::EarlyTicks boot_ticks) {
  InitStdout();

  ApplyRelocations();

  MainSymbolize symbolize(kLegacyShimName);

  // This also fills in gLegacyBoot.
  AddressSpace aspace;
  InitMemory(ptr, {}, &aspace);

  const ktl::span ramdisk = ktl::as_bytes(gLegacyBoot.ramdisk);
  gLegacyBoot.uart_config = UartFromZbi(LegacyBootShim::InputZbi(ramdisk), gLegacyBoot.uart_config);
  UartFromCmdLine(gLegacyBoot.cmdline, gLegacyBoot.uart_config);
  LegacyBootSetUartConsole(gLegacyBoot.uart_config);

  LegacyBootShim shim(symbolize.name(), gLegacyBoot);
  shim.set_build_id(symbolize.build_id());
  shim.Get<boot_shim::UartItem<>>().Init(GetUartDriver().config());

  // The pool knows all the memory details, so populate the ZBI item that way.
  memalloc::Pool& memory = Allocation::GetPool();
  shim.InitMemConfig(memory);

  BootZbi boot;
  if (shim.Load(boot)) {
    memory.PrintMemoryRanges(symbolize.name());
    boot.Log();
    boot.Boot();
  }

  abort();
}

bool LegacyBootShim::Load(BootZbi& boot) { return BootQuirksLoad(boot) || StandardLoad(boot); }

// This is overridden in the special bug-compatibility shim.
[[gnu::weak]] bool LegacyBootShim::BootQuirksLoad(BootZbi& boot) { return false; }

bool LegacyBootShim::StandardLoad(BootZbi& boot) {
  return Check("Not a bootable ZBI", boot.Init(input_zbi())) &&
         Check("Failed to load ZBI", boot.Load(static_cast<uint32_t>(size_bytes()))) &&
         Check("Failed to append boot loader items to data ZBI", AppendItems(boot.DataZbi()));
}

bool LegacyBootShim::IsProperZbi() const {
  bool result = true;
  InputZbi zbi = input_zbi_;
  for (auto [header, payload] : zbi) {
    result = header->type == kArchZbiKernelType;
    break;
  }
  zbi.ignore_error();
  return result;
}

// The default implementation assumes a conforming ZBI image; that is, the
// first item is the kernel item, and items are appended.  The symbols is weak,
// such that bug compatible shims can override this.  Examples of such bugs are
// bootloaders prepending items to the ZBI (preceding the original kernel).
[[gnu::weak]] uart::all::Config<> UartFromZbi(LegacyBootShim::InputZbi zbi,
                                              const uart::all::Config<>& uart_config) {
  auto cleanup = fit::defer([&zbi]() { zbi.ignore_error(); });

  if (ktl::optional new_uart_config = GetUartFromRange(zbi.begin(), zbi.end())) {
    return *new_uart_config;
  }
  return uart_config;
}

ktl::optional<uart::all::Config<>> GetUartFromRange(  //
    LegacyBootShim::InputZbi::iterator start, LegacyBootShim::InputZbi::iterator end) {
  ktl::optional<uart::all::Config<>> uart_config;

  while (start != end && start != start.view().end()) {
    auto& [header, payload] = *start;
    if (header->type == ZBI_TYPE_KERNEL_DRIVER) {
      if (ktl::optional config = uart::all::Config<>::Match(*header, payload.data())) {
        uart_config = *config;
      }
    }
    start++;
  }
  return uart_config;
}
