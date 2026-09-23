// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/riscv64/feature.h>
#include <lib/arch/riscv64/system.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/items/cpu-topology.h>
#include <lib/zbitl/view.h>
#include <zircon/compiler.h>

#include <ktl/optional.h>
#include <ktl/string_view.h>
#include <phys/arch/arch-phys-info.h>
#include <phys/main.h>
#include <phys/stdio.h>

#include "riscv64.h"

// All globals in phys code must be constinit.
// The boot_hart_id field is initialized by start.S.
__CONSTINIT ArchPhysInfo gArchPhysInfoStorage;

ArchPhysInfo* gArchPhysInfo;

void ArchSetUpCpu() {
  gArchPhysInfo = &gArchPhysInfoStorage;

  arch::RiscvStvec::Get()
      .FromValue(0)
      .set_base(reinterpret_cast<uintptr_t>(ArchPhysExceptionEntry))
      .set_mode(arch::RiscvStvec::Mode::kDirect)
      .Write();
}

void ArchSetUpZbi(EarlyBootZbi zbi) {
  zbitl::CpuTopologyTable topology;
  ktl::span<const char> strtab;
  for (auto it = zbi.begin(); it != zbi.end(); ++it) {
    auto [header, wrapped_payload] = *it;
    switch (header->type) {
      case ZBI_TYPE_CPU_TOPOLOGY: {
        ktl::span payload = wrapped_payload.get();
        auto result = zbitl::CpuTopologyTable::FromPayload(header->type, payload);
        if (result.is_ok()) {
          topology = result.value();
        } else {
          ktl::string_view error = result.error_value();
          printf("%s: Unable to decode CPU topology: %.*s\n", ProgramName(),
                 static_cast<int>(error.size()), error.data());
        }
        break;
      }
      case ZBI_TYPE_RISCV64_ISA_STRTAB: {
        ktl::span payload = wrapped_payload.get();
        strtab = {reinterpret_cast<const char*>(payload.data()), payload.size()};

        // Defensively zero out the last character. This should not overwrite
        // any valid character in the string table, and allows us to treat
        // pointers into the table as C strings (as intended).
        const_cast<char&>(strtab.back()) = '\0';
        break;
      }
    }
  }
  zbi.ignore_error();

  if (strtab.empty()) {
    printf("%s: no RISCV64_ISA_STRTAB ZBI item present. Unable to determine supported features.\n",
           ProgramName());
    return;
  }

  ktl::optional<arch::RiscvFeatures> features;
  for (const zbi_topology_node_t& node : topology) {
    if (node.entity.discriminant != ZBI_TOPOLOGY_ENTITY_PROCESSOR) {
      continue;
    }
    const auto& arch_info = node.entity.processor.architecture_info.riscv64;
    size_t strtab_index = arch_info.isa_strtab_index;
    if (strtab_index == 0 || strtab_index + 1 >= strtab.size()) {
      printf("%s: hart %" PRIu64
             " has an invalid index (%zu) into the RISCV64_ISA_STRTAB ZBI item\n",
             ProgramName(), strtab_index, arch_info.hart_id);
      continue;
    }
    ktl::string_view isa_string{&strtab[strtab_index]};
    debugf("%s: hart %02" PRIu64 " ISA string: %s\n", ProgramName(), arch_info.hart_id,
           isa_string.data());
    if (features) {
      *features &= arch::RiscvFeatures{}.SetMany(isa_string);
    } else {
      features = arch::RiscvFeatures{}.SetMany(isa_string);
    }
  }
  if (features) {
    gArchPhysInfo->cpu_features = *features;
  }
}
