// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/debugdata.h>
#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>
#include <lib/fit/result.h>
#include <lib/memalloc/range.h>
#include <lib/uart/all.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/memory.h>
#include <lib/zbi-format/zbi.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <zircon/assert.h>

#include <array>
#include <cstdint>
#include <optional>
#include <string_view>
#include <type_traits>

#include "lib/boot-shim/devicetree.h"

namespace boot_shim {
namespace {
struct GicV3Regs {
  enum Regs : size_t {
    kGicd = 0,
    kGicr = 1,
    kGicc = 2,
    kGich = 3,
    kGicv = 4,
    kReserved = 5,
  };
};

struct GicV2Regs {
  enum Regs : size_t {
    kGicd = 0,
    kGicc = 1,

    // Virtualization Extension
    kGich = 2,
    kGicv = 3,
    kReserved = 4,
  };
};

// See:
// https://android.googlesource.com/kernel/msm/+/android-msm-hammerhead-3.4-kk-r1/Documentation/devicetree/bindings/memory.txt
// Which is a special case of the memory node. This specific incantation may never happen in the
// real world, if ever needed then this
// constexpr std::string_view kMemoryRegion = "region";

}  // namespace
devicetree::ScanState ArmDevicetreeGicItem::HandleGicChildNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  VisitPayload([&](auto& dcfg) -> void {
    using dtype = std::decay_t<decltype(dcfg)>;
    if constexpr (std::is_same_v<dtype, zbi_dcfg_arm_gic_v2_driver_t>) {
      auto [msi_controller, reg_property] = decoder.FindProperties("msi-controller", "reg");

      if (!msi_controller) {
        return;
      }

      if (!reg_property) {
        return;
      }

      auto reg = reg_property->AsReg(decoder);
      if (!reg || reg->size() < 1) {
        return;
      }

      (*mmio_observer_)(MmioRangeFrom((*reg)[0]));
      uint64_t base_address = *(*reg)[0].address();
      if (auto root_address = decoder.TranslateAddress(base_address)) {
        dcfg.use_msi = true;
        dcfg.msi_frame_phys = *root_address;
      } else {
        OnError("GIC v2: MSI address translation failed.");
      }

    } else if constexpr (std::is_same_v<dtype, zbi_dcfg_arm_gic_v3_driver_t>) {
      // TODO(https://fxbug.dev/42078724) : no support yet.
    } else {
      // No Driver set, but we have seen the GIC, so this is an error.
      ZX_PANIC("GIC Item should have been initialized before looking into its child.");
    }
  });
  return devicetree::ScanState::kActive;
}

devicetree::ScanState ArmDevicetreeGicItem::HandleGicV3(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  const auto& [reg_property] = decoder.FindProperties("reg");
  if (!reg_property) {
    OnError("GIC v3: No 'reg' found.");
    return devicetree::ScanState::kDoneWithSubtree;
  }

  auto reg_ptr = reg_property->AsReg(decoder);
  if (!reg_ptr) {
    OnError("GIC v3: Failed to decode 'reg'.");
    return devicetree::ScanState::kDoneWithSubtree;
  }

  auto& reg = *reg_ptr;

  auto [redistributor_stride] = decoder.FindProperties("redistributor-stride");
  auto [redistributor_regions] = decoder.FindProperties("#redistributor-regions");

  zbi_dcfg_arm_gic_v3_driver_t dcfg{};

  if (reg.size() > 1) {
    auto gicd = decoder.TranslateAddress(*reg[GicV3Regs::kGicd].address());
    if (!gicd) {
      OnError("GIC v3: GICD address translation failed.");
      return devicetree::ScanState::kDone;
    }

    auto gicr = decoder.TranslateAddress(*reg[GicV3Regs::kGicr].address());
    if (!gicr) {
      OnError("GIC v3: GICR address translation failed.");
      return devicetree::ScanState::kDone;
    }
    (*mmio_observer_)(MmioRangeFrom(reg[GicV3Regs::kGicd]));
    dcfg.mmio_phys = std::min(*gicd, *gicr);
    dcfg.gicr_offset = *gicr - dcfg.mmio_phys;
    dcfg.gicd_offset = *gicd - dcfg.mmio_phys;

    if (redistributor_stride) {
      if (auto stride = redistributor_stride->AsUint32()) {
        dcfg.gicr_stride = *stride;
      } else if (auto stride64 = redistributor_stride->AsUint64()) {
        dcfg.gicr_stride = *stride64;
      } else {
        OnError("GIC v3: failed to parse redistributor stride.");
      }
    } else {
      // See:
      //   "Arm Generic Interrupt Controller Architecture Specification GIC architecture version 3
      //   and version 4" 12.10 The GIC Redistributor register map
      // Specifically:
      //    - Each Redistributor defines two 64KB frames in the physical address map
      //
      // Note for GicV4: VLPI (Virtual Local Peripherial Interrupt), which would require an extra
      // two pages.
      dcfg.gicr_stride = 2 * 64 << 10;
    }
  }
  auto regions = 1;
  if (redistributor_stride && redistributor_regions) {
    if (auto parsed_regions = redistributor_regions->AsUint32()) {
      regions = *parsed_regions;
    }
  }
  if (regions < 1 || regions > 256) {
    OnError("GIC v3: Out of bounds '#redistributor-stride'.");
    regions = 1;
  }
  // N.b., many GICv3 definitions only declare a single redistribution region
  // rather than one per-cpu.  They instead ensure the reg for the GICR includes
  // all the redistributors within its range.  We observe the larger of the
  // definitions for MmioRange purposes.
  if (reg[GicV3Regs::kGicr].size() > dcfg.gicr_stride * regions) {
    (*mmio_observer_)(MmioRangeFrom(reg[GicV3Regs::kGicr]));
  } else {
    const size_t total_size = static_cast<size_t>(dcfg.gicr_stride * regions);
    const MmioRange range = {
        .address = reg[GicV3Regs::kGicr].address().value(),
        .size = total_size,
    };
    (*mmio_observer_)(range);
  }
  dcfg.ipi_base = 0;
  dcfg.optional = false;
  set_payload(dcfg);

  return devicetree::ScanState::kDone;
}

devicetree::ScanState ArmDevicetreeGicItem::HandleGicV2(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  const auto& [reg_property] = decoder.FindProperties("reg");

  if (!reg_property) {
    OnError("GIC v2: No 'reg' found.");
    return devicetree::ScanState::kDone;
  }

  auto reg_ptr = reg_property->AsReg(decoder);
  if (!reg_ptr) {
    OnError("GIC v2: Failed to decode 'reg'.");
    return devicetree::ScanState::kDone;
  }

  auto& reg = *reg_ptr;

  zbi_dcfg_arm_gic_v2_driver_t dcfg{};
  if (reg.size() > 1) {
    auto gicd = decoder.TranslateAddress(*reg[GicV2Regs::kGicd].address());
    if (!gicd) {
      OnError("GIC v2: Failed to translate gicd address.");
      return devicetree::ScanState::kDone;
    }

    if (!reg[GicV2Regs::kGicc].address()) {
      OnError("GIC v2: Failed to obtain GICC address.");
      return devicetree::ScanState::kDone;
    }

    auto gicc = decoder.TranslateAddress(*reg[GicV2Regs::kGicc].address());
    if (!gicc) {
      OnError("GIC v2: Failed to translate gicc address.");
      return devicetree::ScanState::kDone;
    }
    (*mmio_observer_)(MmioRangeFrom(reg[GicV2Regs::kGicd]));
    (*mmio_observer_)(MmioRangeFrom(reg[GicV2Regs::kGicc]));
    dcfg.mmio_phys = std::min(*gicd, *gicc);
    dcfg.gicd_offset = *gicd - dcfg.mmio_phys;
    dcfg.gicc_offset = *gicc - dcfg.mmio_phys;
    dcfg.gich_offset = 0;
    dcfg.gicv_offset = 0;
  }

  // If there are more than 2, then the virtualization extension is provided.
  if (reg.size() > 2) {
    if (!reg[GicV2Regs::kGich].address()) {
      OnError("GIC v2: Failed to obtain GICH address.");
      return devicetree::ScanState::kDone;
    }

    auto gich = decoder.TranslateAddress(*reg[GicV2Regs::kGich].address());
    if (!gich) {
      OnError("GIC v2: Failed to translate GICH address.");
      return devicetree::ScanState::kDone;
    }

    if (!reg[GicV2Regs::kGicv].address()) {
      OnError("GIC v2: Failed to obtain GICH address.");
      return devicetree::ScanState::kDone;
    }

    auto gicv = decoder.TranslateAddress(*reg[GicV2Regs::kGicv].address());
    if (!gicv) {
      OnError("GIC v2: Failed to translate GICV address.");
      return devicetree::ScanState::kDone;
    }

    uint64_t min_mmio_phys = std::min(dcfg.mmio_phys, std::min(*gicv, *gich));

    // Need to recalculate offsets from new minimal address.
    (*mmio_observer_)(MmioRangeFrom(reg[GicV2Regs::kGich]));
    (*mmio_observer_)(MmioRangeFrom(reg[GicV2Regs::kGicv]));
    if (min_mmio_phys != dcfg.mmio_phys) {
      dcfg.gicd_offset = dcfg.gicd_offset + dcfg.mmio_phys - min_mmio_phys;
      dcfg.gicc_offset = dcfg.gicc_offset + dcfg.mmio_phys - min_mmio_phys;
      dcfg.mmio_phys = min_mmio_phys;
    }
    dcfg.gich_offset = *gich - dcfg.mmio_phys;
    dcfg.gicv_offset = *gicv - dcfg.mmio_phys;
  }

  dcfg.ipi_base = 0;
  dcfg.optional = false;
  // Default values when msi is not enabled. This is determined by proper handlers.
  // The MSI Frame registers are contigous, so we will pick the lowest base address from all the
  // frames.
  dcfg.use_msi = false;
  // Kernel expects 0 as no MSI.
  dcfg.msi_frame_phys = 0;
  set_payload(dcfg);
  gic_ = &path.back();

  return devicetree::ScanState::kActive;
}

devicetree::ScanState ArmDevicetreeGicItem::OnNode(const devicetree::NodePath& path,
                                                   const devicetree::PropertyDecoder& decoder) {
  if (IsGicChildNode()) {
    return HandleGicChildNode(path, decoder);
  }

  // Figure out which interrupt controller this is.
  const auto& [compatible, interrupt_controller, msi_controller] =
      decoder.FindProperties("compatible", "interrupt-controller", "msi-controller");

  if (!interrupt_controller || !compatible) {
    return devicetree::ScanState::kActive;
  }

  auto compatible_list = compatible->AsStringList();
  if (!compatible_list) {
    return devicetree::ScanState::kActive;
  }

  // Check for gic version.
  for (auto comp : *compatible_list) {
    for (auto v3 : kGicV3CompatibleDevices) {
      if (v3 == comp) {
        return HandleGicV3(path, decoder);
      }
    }

    for (auto v2 : kGicV2CompatibleDevices) {
      if (v2 == comp) {
        return HandleGicV2(path, decoder);
      }
    }
  }

  // Keep looking.
  return devicetree::ScanState::kActive;
}

devicetree::ScanState ArmDevicetreeGicItem::OnSubtree(const devicetree::NodePath& path) {
  if (gic_ == nullptr) {
    return matched_ ? devicetree::ScanState::kDone : devicetree::ScanState::kActive;
  }

  if (gic_ != &path.back()) {
    return devicetree::ScanState::kActive;
  }

  gic_ = nullptr;
  matched_ = true;
  return devicetree::ScanState::kDone;
}

}  // namespace boot_shim
