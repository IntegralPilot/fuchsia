// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "phys/boot-shim/devicetree.h"

#include <lib/boot-options/boot-options.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>
#include <lib/linux-boot-config/linux-boot-config.h>
#include <lib/memalloc/range.h>
#include <zircon/assert.h>

#include <cstddef>
#include <cstring>

#include <fbl/alloc_checker.h>
#include <ktl/array.h>
#include <ktl/limits.h>
#include <ktl/type_traits.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/boot-options.h>
#include <phys/main.h>
#include <phys/uart.h>

#include <ktl/enforce.h>

DevicetreeBoot gDevicetreeBoot;

namespace {

// Other platforms such as Linux provide a few of preallocated buffers for storing memory ranges,
// |kMaxRanges| is a big enough upperbound for the combined number of ranges provided by such
// buffers.
//
// This represents a recommended number of entries to be allocated for storing real world memory
// ranges.
constexpr size_t kDevicetreeMaxMemoryRanges = 512;

ktl::optional<linux_boot_config::LinuxBootConfig> GetBootConfig(
    ktl::span<const ktl::byte> ramdisk) {
  auto boot_config = linux_boot_config::LinuxBootConfig::Create(ramdisk);
  if (boot_config.is_error()) {
    // UART is already set up.
    linux_boot_config::ParseError err = boot_config.error_value();
    ktl::string_view msg = err.description;
    printf("%s: Failed to parse boot config. %.*s", ProgramName(), static_cast<int>(msg.size()),
           msg.data());
    if (err.offset) {
      printf(" at offset %zu\n", *err.offset);
    } else {
      printf("\n");
    }
    return ktl::nullopt;
  }

  // An empty boot config is perfectly valid.
  if (boot_config->size_bytes() == 0) {
    return linux_boot_config::LinuxBootConfig{};
  }

  // Copy the boot config out of the way, so that the range remains valid during the lifetime
  // of this boot shim. It may be overwritten by `BootZbi` when appending items, since this would be
  // at the tail of the DataZbi.
  fbl::AllocChecker ac;

  // Leaking kPhysScratch allocations is OK, these are not maintained across different boot stages.
  auto boot_config_view = ktl::span(new (gPhysNew<memalloc::Type::kPhysScratch>, ac)
                                        ktl::byte[boot_config->size_bytes()],
                                    boot_config->size_bytes());

  if (!ac.check()) {
    printf("%s: Failed to allocate BOOTCONFIG temporary space.", ProgramName());
    return ktl::nullopt;
  }

  memcpy(boot_config_view.data(), boot_config->contents().data(), boot_config->size_bytes());
  ktl::string_view boot_config_contents(reinterpret_cast<const char*>(boot_config_view.data()),
                                        boot_config_view.size_bytes());
  return linux_boot_config::LinuxBootConfig{boot_config_contents};
}

}  // namespace

void InitMemory(const void* dtb, ktl::optional<EarlyBootZbi> zbi, AddressSpace* aspace) {
  static ktl::array<memalloc::Range, kDevicetreeMaxMemoryRanges> range_storage;
  // A devicetree-based boot shim should not have a ZBI encoding boot state at
  // this point.
  ZX_DEBUG_ASSERT(!zbi);

  devicetree::ByteView fdt_blob(static_cast<const uint8_t*>(dtb),
                                ktl::numeric_limits<uintptr_t>::max());
  devicetree::Devicetree fdt(fdt_blob);

  boot_shim::DevicetreeMemoryMatcher memory("init-memory", stdout, range_storage);
  boot_shim::DevicetreeChosenNodeMatcher<> chosen("init-memory", stdout);
  ZX_ASSERT(devicetree::Match(fdt, chosen, memory));

  //
  // The following 'special' memory ranges are those that we already know are
  // populated.
  //
  uint64_t phys_start = reinterpret_cast<uint64_t>(PHYS_LOAD_ADDRESS);
  uint64_t phys_end = reinterpret_cast<uint64_t>(_end);
  ktl::array<memalloc::Range, 4> special_range_storage = {
      memalloc::Range{
          .addr = phys_start,
          .size = phys_end - phys_start,
          .type = memalloc::Type::kPhysKernel,
      },
      {
          .addr = reinterpret_cast<uintptr_t>(fdt.fdt().data()),
          .size = fdt.size_bytes(),
          .type = memalloc::Type::kDevicetreeBlob,
      },
  };
  ktl::span<memalloc::Range> special_ranges = ktl::span{special_range_storage}.subspan(0, 2);

  // Technically this is not the ZBI, this is the initrd provided by the bootloader.
  // This range contains the BOOTCONFIG if its present. This is fine, after memory is initialized,
  // we will just copy it out of the way.
  if (!chosen.ramdisk().empty()) {
    special_range_storage[special_ranges.size()] = memalloc::Range{
        .addr = reinterpret_cast<uintptr_t>(chosen.ramdisk().data()),
        .size = chosen.ramdisk().size(),
        .type = memalloc::Type::kDataZbi,
    };
    special_ranges = ktl::span(special_range_storage).subspan(0, special_ranges.size() + 1);
  }

  if (memory.nvram()) {
    special_range_storage[special_ranges.size()] = {
        .addr = memory.nvram()->base,
        .size = memory.nvram()->length,
        .type = memalloc::Type::kNvram,
    };
    special_ranges = ktl::span(special_range_storage).subspan(0, special_ranges.size() + 1);
  }

  // The matching phase above recorded all of the memory ranges encoded within
  // the devicetree tree structure, leaving the memory reservations. Since
  // bootloaders may sometimes generate spurious memory reservations for things
  // like the devicetree blob and ramdisk, we take care to exclude those ranges
  // from the translation to RESERVED ranges. This is handled by
  // ForEachDevicetreeMemoryReservation below.
  ktl::span<memalloc::Range> ranges;
  {
    // ForEachDevicetreeMemoryReservation requires that the 'exclusions' be
    // non-overlapping and sorted. The special ranges are surely
    // non-overlapping.
    ktl::sort(special_ranges.begin(), special_ranges.end(),
              [](auto a, auto b) { return (a.addr < b.addr); });
    size_t written = memory.ranges().size();
    bool recorded = boot_shim::ForEachDevicetreeMemoryReservation(
        fdt, /*exclusions=*/special_ranges, [&written](devicetree::MemoryReservation res) {
          if (written >= range_storage.size()) {
            return false;
          }
          range_storage[written++] = memalloc::Range{
              .addr = res.start,
              .size = res.size,
              .type = memalloc::Type::kReserved,
          };
          return true;
        });
    ZX_ASSERT_MSG(recorded, "Insufficient space to record devicetree memory reservations");
    ranges = ktl::span{range_storage}.subspan(0, written);
  }

  // This instance of |BootOptions| is not meant to be wired anywhere, its sole purpose is to select
  // the proper uart from the cmdline if its present.
  static BootOptions boot_options;

  // If the chosen matcher did not find a serial console setting, keep whatever
  // current setting was in place before calling InitMemory. That's often the
  // null driver, but could be something else.
  //
  // TODO(https://fxbug.dev/397523685): Match operation returns a config and matcher stores a config
  // instead of a driver object.
  boot_options.serial = chosen.uart_config().value_or(GetUartDriver().config());
  EarlyBootZbiBytes early_zbi_bytes{chosen.ramdisk()};
  SetBootOptionsWithoutEntropy(boot_options, EarlyBootZbi{&early_zbi_bytes},
                               chosen.cmdline().value_or(""));
  SetUartConsole(boot_options.serial);

  Allocation::Init(ranges, special_ranges);
  if (aspace) {
    ArchSetUpAddressSpace(*aspace);
  }
  Allocation::GetPool().PrintMemoryRanges(ProgramName());

  gDevicetreeBoot = {
      .cmdline = chosen.cmdline().value_or(""),
      .ramdisk = chosen.ramdisk(),
      .linux_boot_config = GetBootConfig(chosen.ramdisk()),
      .fdt = fdt,
      .nvram = memory.nvram(),
  };
}
