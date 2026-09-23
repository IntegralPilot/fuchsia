// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "arch/arm64/periphmap.h"

#include <debug.h>
#include <lib/console.h>
#include <lib/mmio-ptr/mmio-ptr.h>
#include <stdint.h>

#include <ktl/algorithm.h>
#include <ktl/bit.h>
#include <ktl/optional.h>
#include <lk/init.h>
#include <phys/handoff.h>

#include <ktl/enforce.h>

namespace {

// Backed by permanent hand-off memory.
ktl::span<const MappedMmioRange> gPeriphRanges;

void RecordPeriphRanges(uint level) {
  constexpr size_t kExpectedMax = 4;

  gPeriphRanges = gPhysHandoff->periph_ranges.get();
  if (gPeriphRanges.size() > kExpectedMax) {
    dprintf(INFO, "Warning: unexpected large number of PERIPHERAL ranges: %zu",
            gPeriphRanges.size());
  }
}

LK_INIT_HOOK(record_periph_ranges, RecordPeriphRanges, LK_INIT_LEVEL_EARLIEST)

struct Phys2VirtTrait {
  static uint64_t src(const MappedMmioRange& r) { return r.paddr(); }
  static uint64_t dst(const MappedMmioRange& r) { return reinterpret_cast<uintptr_t>(r.data()); }
};

struct Virt2PhysTrait {
  static uint64_t src(const MappedMmioRange& r) { return reinterpret_cast<uintptr_t>(r.data()); }
  static uint64_t dst(const MappedMmioRange& r) { return r.paddr(); }
};

template <typename Fetch>
struct PeriphUtil {
  // Translate (without range checking) the (virt|phys) peripheral provided to
  // its (phys|virt) counterpart using the provided range.
  static uint64_t Translate(const MappedMmioRange& range, uint64_t addr) {
    return addr - Fetch::src(range) + Fetch::dst(range);
  }

  // Find the index (if any) of the peripheral range which contains the
  // (virt|phys) address <addr>
  static ktl::optional<uint32_t> LookupNdx(uint64_t addr) {
    for (uint32_t i = 0; i < ktl::size(gPeriphRanges); ++i) {
      const auto& range = gPeriphRanges[i];
      if (range.size_bytes() == 0) {
        break;
      } else if (addr >= Fetch::src(range)) {
        uint64_t offset = addr - Fetch::src(range);
        if (offset < range.size_bytes()) {
          return {i};
        }
      }
    }
    return {};
  }

  // Map the (virt|phys) peripheral provided to its (phys|virt) counterpart (if
  // any)
  static ktl::optional<uint64_t> Map(uint64_t addr) {
    auto ndx = LookupNdx(addr);
    if (ndx.has_value()) {
      return Translate(gPeriphRanges[ndx.value()], addr);
    }
    return {};
  }
};

using Phys2Virt = PeriphUtil<Phys2VirtTrait>;
using Virt2Phys = PeriphUtil<Virt2PhysTrait>;

template <typename T>
uint64_t rd_reg(vaddr_t addr) {
  return MmioRead(reinterpret_cast<const MMIO_PTR volatile T*>(static_cast<uintptr_t>(addr)));
}

template <typename T>
void wr_reg(vaddr_t addr, uint64_t val) {
  MmioWrite(static_cast<T>(val),
            reinterpret_cast<MMIO_PTR volatile T*>(static_cast<uintptr_t>(addr)));
}

// Note; the choice of these values must also align with the definitions in the
// options array below.
enum class AccessWidth {
  Byte = 0,
  Halfword = 1,
  Word = 2,
  Doubleword = 3,
};
constexpr struct {
  const char* tag;
  void (*print)(uint64_t);
  uint64_t (*rd)(vaddr_t);
  void (*wr)(vaddr_t, uint64_t);
  uint32_t byte_width;
} kDumpModOptions[] = {
    {
        .tag = "byte",
        .print = [](uint64_t val) { printf(" %02" PRIx64, val); },
        .rd = rd_reg<uint8_t>,
        .wr = wr_reg<uint8_t>,
        .byte_width = 1,
    },
    {
        .tag = "halfword",
        .print = [](uint64_t val) { printf(" %04" PRIx64, val); },
        .rd = rd_reg<uint16_t>,
        .wr = wr_reg<uint16_t>,
        .byte_width = 2,
    },
    {
        .tag = "word",
        .print = [](uint64_t val) { printf(" %08" PRIx64, val); },
        .rd = rd_reg<uint32_t>,
        .wr = wr_reg<uint32_t>,
        .byte_width = 4,
    },
    {
        .tag = "doubleword",
        .print = [](uint64_t val) { printf(" %016" PRIx64, val); },
        .rd = rd_reg<uint64_t>,
        .wr = wr_reg<uint64_t>,
        .byte_width = 8,
    },
};

zx_status_t dump_periph(paddr_t phys, uint64_t count, AccessWidth width) {
  const auto& opt = kDumpModOptions[static_cast<uint32_t>(width)];

  // Sanity check count
  if (!count) {
    printf("Illegal count %lu\n", count);
    return ZX_ERR_INVALID_ARGS;
  }

  uint64_t byte_amt = count * opt.byte_width;
  paddr_t phys_end_addr = phys + byte_amt - 1;

  // Sanity check alignment.
  if (phys & (opt.byte_width - 1)) {
    printf("%016lx is not aligned to a %u byte boundary!\n", phys, opt.byte_width);
    return ZX_ERR_INVALID_ARGS;
  }

  // Validate that the entire requested range fits within a single mapping.
  auto start_ndx = Phys2Virt::LookupNdx(phys);
  auto end_ndx = Phys2Virt::LookupNdx(phys_end_addr);
  if (!start_ndx.has_value() || !end_ndx.has_value() || (start_ndx.value() != end_ndx.value())) {
    printf("Physical range [%016lx, %016lx] is not contained in a single mapping!\n", phys,
           phys_end_addr);
    return ZX_ERR_INVALID_ARGS;
  }

  // OK, all of our sanity checks are complete.  Time to start dumping data.
  constexpr uint32_t bytes_per_line = 16;
  const uint64_t count_per_line = bytes_per_line / opt.byte_width;
  vaddr_t virt = Phys2Virt::Translate(gPeriphRanges[start_ndx.value()], phys);
  vaddr_t virt_end_addr = virt + byte_amt;

  printf("Dumping %lu %s%s starting at phys 0x%016lx\n", count, opt.tag, count == 1 ? "" : "s",
         phys);
  while (virt < virt_end_addr) {
    printf("%016lx :", phys);
    for (uint64_t i = 0; (i < count_per_line) && (virt < virt_end_addr);
         ++i, virt += opt.byte_width) {
      opt.print(opt.rd(virt));
    }
    phys += bytes_per_line;
    printf("\n");
  }

  return ZX_OK;
}

zx_status_t mod_periph(paddr_t phys, uint64_t val, AccessWidth width) {
  const auto& opt = kDumpModOptions[static_cast<uint32_t>(width)];

  // Sanity check alignment.
  if (phys & (opt.byte_width - 1)) {
    printf("%016lx is not aligned to a %u byte boundary!\n", phys, opt.byte_width);
    return ZX_ERR_INVALID_ARGS;
  }

  // Translate address
  auto vaddr = Phys2Virt::Map(phys);
  if (!vaddr.has_value()) {
    printf("Physical addr %016lx in not in the peripheral mappings!\n", phys);
  }

  // Perform the write, then report what we did.
  opt.wr(vaddr.value(), val);
  printf("Wrote");
  opt.print(val);
  printf(" to phys addr %016lx\n", phys);

  return ZX_OK;
}

int cmd_peripheral_map(int argc, const cmd_args* argv, uint32_t flags) {
  auto usage = [cmd = argv[0].str](bool not_enough_args = false) -> zx_status_t {
    if (not_enough_args) {
      printf("not enough arguments\n");
    }

    printf("usage:\n");
    printf("%s dump\n", cmd);
    printf("%s phys2virt <addr>\n", cmd);
    printf("%s virt2phys <addr>\n", cmd);
    printf(
        "%s dd|dw|dh|db <phys_addr> [<count>] :: Dump <count> (double|word|half|byte) from "
        "<phys_addr> (count default = 1)\n",
        cmd);
    printf(
        "%s md|mw|mh|mb <phys_addr> <value> :: Write the contents of <value> to the "
        "(double|word|half|byte) at <phys_addr>\n",
        cmd);

    return ZX_ERR_INTERNAL;
  };

  if (argc < 2) {
    return usage(true);
  }

  if (!strcmp(argv[1].str, "dump")) {
    for (const auto& range : gPeriphRanges) {
      DEBUG_ASSERT(range.size_bytes() > 0);
      printf("Phys [%016lx, %016lx) ==> Virt [%16p, %016lx) (len 0x%08lx)\n", range.paddr(),
             range.paddr_end(), range.data(), range.vaddr_end(), range.size_bytes());
    }
    printf("Dumped %zu defined peripheral map ranges\n", gPeriphRanges.size());
  } else if (!strcmp(argv[1].str, "phys2virt") || !strcmp(argv[1].str, "virt2phys")) {
    if (argc < 3) {
      return usage(true);
    }

    bool phys_src = !strcmp(argv[1].str, "phys2virt");
    auto map_fn = phys_src ? Phys2Virt::Map : Virt2Phys::Map;
    auto res = map_fn(argv[2].u);
    if (res.has_value()) {
      printf("%016lx ==> %016lx\n", argv[2].u, res.value());
    } else {
      printf("Failed to find the %s address 0x%016lx in the peripheral mappings.\n",
             phys_src ? "physical" : "virtual", argv[2].u);
    }
  } else if ((argv[1].str[0] == 'd') || (argv[1].str[0] == 'm')) {
    // If this is a valid display or modify command, its length will be exactly 2.
    if (strlen(argv[1].str) != 2) {
      return usage();
    }

    // Parse the next letter to figure out the width of the operation.
    AccessWidth width;
    switch (argv[1].str[1]) {
      case 'd':
        width = AccessWidth::Doubleword;
        break;
      case 'w':
        width = AccessWidth::Word;
        break;
      case 'h':
        width = AccessWidth::Halfword;
        break;
      case 'b':
        width = AccessWidth::Byte;
        break;
      default:
        return usage();
    }

    paddr_t phys_addr = argv[2].u;
    if (argv[1].str[0] == 'd') {
      // Dump commands have a default count of 1
      return dump_periph(phys_addr, (argc < 4) ? 1 : argv[3].u, width);
    } else {
      // Modify commands are required to have a value.
      return (argc < 4) ? usage(true) : mod_periph(phys_addr, argv[3].u, width);
    }

  } else {
    return usage();
  }

  return ZX_OK;
}

}  // namespace

vaddr_t periph_paddr_to_vaddr(paddr_t paddr) {
  auto ret = Phys2Virt::Map(paddr);
  return ret.has_value() ? ret.value() : 0;
}

paddr_t periph_vaddr_to_paddr(vaddr_t vaddr) {
  auto ret = Virt2Phys::Map(vaddr);
  return ret.has_value() ? ret.value() : 0;
}

STATIC_COMMAND_START
STATIC_COMMAND_MASKED("pm", "peripheral mapping commands", &cmd_peripheral_map, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_END(pmap)
