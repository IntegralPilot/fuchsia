// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "legacy-boot.h"

#include <inttypes.h>
#include <lib/boot-shim/acpi.h>
#include <lib/memalloc/pool.h>
#include <lib/memalloc/range.h>
#include <zircon/assert.h>

#include <ktl/array.h>
#include <ktl/iterator.h>
#include <ktl/limits.h>
#include <ktl/ranges.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <phys/acpi.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/main.h>
#include <phys/symbolize.h>
#include <phys/uart-console.h>
#include <pretty/sizes.h>

#include <ktl/enforce.h>

extern "C" {

// TODO(https://fxbug.dev/42159372): In the linuxboot case, the linking logic gives the
// wrong value for PHYS_LOAD_ADDRESS in the linuxboot case; work around that
// for now.
[[gnu::weak]] extern ktl::byte LINUXBOOT_LOAD_ADDRESS[];

}  // extern "C"

namespace {

constexpr uintptr_t kSmbiosSearchBase = 0xf0000;
constexpr size_t kSmbiosSearchSize = 0x10000;
constexpr size_t kSmbiosAlign = 16;
constexpr ktl::array kSmbiosSignatures{"_SM_"sv, "_SM3_"sv};

void InitAcpi(LegacyBoot& boot_info) {
  auto acpi_parser = MakeAcpiParser(boot_info.acpi_rsdp);
  if (acpi_parser.is_error()) {
    printf("%s: Cannot find ACPI tables (%" PRId32 ") from %#" PRIx64 "\n", ProgramName(),
           acpi_parser.error_value(), gLegacyBoot.acpi_rsdp);
    return;
  }
  boot_info.acpi_rsdp = acpi_parser->rsdp_pa();

  if (auto debug_port = acpi_lite::GetDebugPort(*acpi_parser); debug_port.is_ok()) {
    if (ktl::optional config = uart::all::Config<>::Match(*debug_port)) {
      boot_info.uart_config = *config;
      SetUartConsole(boot_info.uart_config);
    }
  }
}

void InitSmbios(LegacyBoot& boot_info) {
  ktl::string_view scan{
      reinterpret_cast<const char*>(kSmbiosSearchBase),
      kSmbiosSearchSize,
  };
  while (scan.size() >= kSmbiosAlign) {
    for (ktl::string_view sig : kSmbiosSignatures) {
      if (scan.starts_with(sig)) {
        boot_info.smbios = reinterpret_cast<uintptr_t>(scan.data());
        return;
      }
    }
    scan.remove_prefix(kSmbiosAlign);
  }
}

constexpr memalloc::Range AsMemrange(  //
    const ktl::ranges::contiguous_range auto& spanlike,
    memalloc::Type type = memalloc::Type::kLegacyBootData) {
  ktl::span bytes = ktl::as_bytes(ktl::span{spanlike});
  return {
      .addr = reinterpret_cast<uintptr_t>(bytes.data()),
      .size = bytes.size(),
      .type = type,
  };
}

}  // namespace

// A default, weak definition that may be overrode to perform other relevant
// initialization.
[[gnu::weak]] void LegacyBootSetUartConsole(const uart::all::Config<>& uart_config) {
  SetUartConsole(uart_config);
}

void LegacyBootInitMemory(AddressSpace* aspace) {
  // Note that these are done before paging is enabled (at all for 32-bit) with
  // new identity-mapping page tables, because the fixed-address regions they
  // scan won't be among the known RAM ranges that get identity-mapped.
  InitAcpi(gLegacyBoot);
  InitSmbios(gLegacyBoot);

  // TODO(https://fxbug.dev/42159372): See LINUXBOOT_LOAD_ADDRESS comment above.
  uint64_t phys_start = &LINUXBOOT_LOAD_ADDRESS ? reinterpret_cast<uint64_t>(LINUXBOOT_LOAD_ADDRESS)
                                                : reinterpret_cast<uint64_t>(PHYS_LOAD_ADDRESS);
  uint64_t phys_end = reinterpret_cast<uintptr_t>(_end);

  auto in_load_image = [phys_start, phys_end](const auto& obj) -> bool {
    ktl::span bytes = ktl::as_bytes(ktl::span{obj});
    uint64_t start = reinterpret_cast<uintptr_t>(bytes.data());
    uint64_t end = start + bytes.size();
    return phys_start <= start && end <= phys_end;
  };

  // Do not fill in the last three ranges in the array yet; we only need to
  // account for them if they do not lie within the shim's load image.
  memalloc::Range ranges[] = {
      {
          .addr = phys_start,
          .size = phys_end - phys_start,
          .type = memalloc::Type::kPhysKernel,
      },
      AsMemrange(gLegacyBoot.ramdisk, memalloc::Type::kDataZbi),
      {},
      {},
      {},
  };
  size_t num_ranges = 2;
  if (!in_load_image(gLegacyBoot.cmdline)) {
    ranges[num_ranges++] = AsMemrange(gLegacyBoot.cmdline);
  }
  if (!in_load_image(gLegacyBoot.bootloader)) {
    ranges[num_ranges++] = AsMemrange(gLegacyBoot.bootloader);
  }
  if (!in_load_image(gLegacyBoot.mem_config)) {
    ranges[num_ranges++] = AsMemrange(gLegacyBoot.mem_config);
  }

  Allocation::Init(memalloc::AsRanges(gLegacyBoot.mem_config),
                   ktl::span(ranges).subspan(0, num_ranges));

  if (aspace) {
    // Now that the allocator is available, it can be used to set up the
    // identity-mapping page tables and enable paging.
    ArchSetUpAddressSpace(*aspace);
  }
}

ktl::optional<uart::ns8250::PioDriver> LegacyUartFromTty(const boot_shim::Tty& tty) {
  constexpr auto kUartConfig = ktl::to_array<zbi_dcfg_simple_pio_t>({
      {.base = 0x3F8, .irq = 4},
      {.base = 0x2F8, .irq = 3},
      {.base = 0x3E8, .irq = 4},
      {.base = 0x2E8, .irq = 3},
  });

  switch (tty.type) {
    case boot_shim::TtyType::kSerial:
    case boot_shim::TtyType::kAny:
      break;
    default:
      return ktl::nullopt;
  }

  if (tty.index >= kUartConfig.size()) {
    return ktl::nullopt;
  }

  return uart::ns8250::PioDriver(kUartConfig[tty.index]);
}
