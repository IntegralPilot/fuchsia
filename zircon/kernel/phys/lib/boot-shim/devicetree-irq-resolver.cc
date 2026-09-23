// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/devicetree/matcher.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <lib/zbi-format/driver-config.h>

#include "lib/boot-shim/devicetree.h"

namespace boot_shim {
namespace {

using IrqConfig = DevicetreeIrqResolver::IrqConfig;

// Function that will parse interrupt prop encoded bytes into an IRQ number if possible.
using IrqResolver = fit::inline_function<std::optional<IrqConfig>(devicetree::ByteView, uint32_t)>;

enum class GicVersion {
  kV2,
  kV3,
};

// Both GIC V2 and V3 have the same structure for interrupt cells.
template <GicVersion Version>
std::optional<IrqConfig> GetGicIrq(devicetree::PropertyValue interrupt_bytes,
                                   uint32_t interrupt_cells) {
  // GIC v2 requires 3 cells.
  // GIC v3 requires at least 3 cells, but cells 4 and beyond are reserved for future use.
  // Also GIC v3 states that cells 4 and beyond act as padding, and may be ignored. It is
  // recommended that padding cells have a value of 0.
  if (interrupt_cells < 3) {
    return std::nullopt;
  }

  auto interrupt = interrupt_bytes.AsBytes().subspan(0, 3 * sizeof(uint32_t));

  // Now we can instantiate a prop encoded array.
  devicetree::PropEncodedArray<devicetree::PropEncodedArrayElement<3>> cells(interrupt, 1, 1, 1);

  // The 1st cell is the interrupt type; 0 for SPI interrupts, 1 for PPI interrupts.
  bool is_spi = *cells[0][0] == 0;

  //  The 2nd cell contains the interrupt number for the interrupt type.
  //  SPI interrupts are in the range [0-987].  PPI interrupts are in the
  //  range [0-15].
  uint32_t irq_offset = static_cast<uint32_t>(*cells[0][1]);

  // PPI:
  //    A PPI is unique to one core. However, the PPIs to other cores can have the same INTID. Up to
  //    16 PPIs can be recorded for each target core, where each PPI has a different INTID in the
  //    ID16-ID31 range.
  //
  // See https://developer.arm.com/documentation/101206/0003/Operation/Interrupt-types/PPIs
  //
  // SPI:
  //    The permitted values are ID32-ID991, in steps of 32. The first SPI has an ID number of 32.
  //
  // See https://developer.arm.com/documentation/101206/0003/Operation/Interrupt-types/SPIs
  //

  uint32_t flags = 0;
  uint32_t config_flags = static_cast<uint32_t>(*cells[0][2]);

  // GIC v2:
  //	bits[3:0] trigger type and level flags.
  //		1 = low-to-high edge triggered
  //		2 = high-to-low edge triggered (invalid for SPIs)
  //		4 = active high level-sensitive
  //		8 = active low level-sensitive (invalid for SPIs).
  if constexpr (Version == GicVersion::kV2) {
    constexpr uint32_t kEdgeTriggered = 0b0011;
    constexpr uint32_t kLevelTriggered = 0b1100;
    constexpr uint32_t kActiveHigh = 0b0101;
    constexpr uint32_t kActiveLow = 0b1010;

    if (kEdgeTriggered & config_flags) {
      flags |= ZBI_KERNEL_DRIVER_IRQ_FLAGS_EDGE_TRIGGERED;

    } else if (kLevelTriggered & config_flags) {
      flags |= ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED;
    }

    if (kActiveHigh & config_flags) {
      flags |= ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH;
    } else if (kActiveLow & config_flags) {
      flags |= ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_LOW;
    }
  } else {
    // The 3rd cell is the flags, encoded as follows:
    //    bits[3:0] trigger type and level flags.
    //    1 = edge triggered
    //    4 = level triggered
    constexpr uint32_t kEdgeTriggered = 0b0001;
    constexpr uint32_t kLevelTriggered = 0b0100;

    if (kEdgeTriggered & config_flags) {
      flags |= ZBI_KERNEL_DRIVER_IRQ_FLAGS_EDGE_TRIGGERED;
      flags |= ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH;
    } else if (kLevelTriggered & config_flags) {
      flags |= ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED;
      flags |= ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH;
    }
  }

  return IrqConfig{.irq = (is_spi ? 32 : 16) + irq_offset, .flags = flags};
}

std::optional<IrqConfig> GetPlicIrq(devicetree::PropertyValue interrupt_bytes,
                                    uint32_t interrupt_cells) {
  // PLIC must have at least one cell.
  if (interrupt_cells < 1) {
    return std::nullopt;
  }

  // Only the first cell matters, which is the Interrupt ID or IRQ Number for our purposes.
  //
  // Global interrupt sources are assigned small unsigned integer identifiers, beginning at the
  // value 1. An interrupt ID of 0 is reserved to mean “no interrupt”.
  //
  // See Chapter 7 Section 5 of  Volume II: RISC-V Privileged Architectures V1.10
  auto interrupt = interrupt_bytes.AsBytes().subspan(0, sizeof(uint32_t));
  devicetree::PropEncodedArray<devicetree::PropEncodedArrayElement<1>> cells(interrupt, 1);

  // While the PLIC supports both edge-triggered and level-triggered interrupts,
  // interrupt handlers are oblivious to this distinction and therefore it is not
  // specified in the PLIC device-tree binding.
  //
  // There are no sources regarding where this information is obtained from, so far we hardcode
  // what existing drivers do.
  uint32_t flags = 0;

  return IrqConfig{.irq = static_cast<uint32_t>(*cells[0][0]), .flags = flags};
}

IrqResolver GetIrqResolver(devicetree::StringList<> compatibles) {
  auto is_compatible = [&compatibles](const auto& bindings) {
    return std::find_first_of(compatibles.begin(), compatibles.end(), bindings.begin(),
                              bindings.end()) != compatibles.end();
  };
  if (is_compatible(ArmDevicetreeGicItem::kGicV2CompatibleDevices)) {
    return &GetGicIrq<GicVersion::kV2>;
  }
  if (is_compatible(ArmDevicetreeGicItem::kGicV3CompatibleDevices)) {
    return &GetGicIrq<GicVersion::kV3>;
  }

  if (is_compatible(RiscvDevicetreePlicItem::kCompatibleDevices)) {
    return &GetPlicIrq;
  }
  return [](devicetree::ByteView view, uint32_t interrupt_cells) { return std::nullopt; };
}

}  // namespace

fit::result<fit::failed, bool> DevicetreeIrqResolver::ResolveIrqController(
    const devicetree::PropertyDecoder& decoder) {
  // Try to match the 'interrupt-parent' phandle.
  if (NeedsInterruptParent()) {
    auto [compatibles, phandle] = decoder.FindProperties("compatible", "phandle");
    if (phandle && phandle->AsUint32() == interrupt_parent_) {
      if (!compatibles) {
        error_ = true;
        return fit::failed();
      }

      if (auto compatible_list = compatibles->AsStringList()) {
        irq_resolver_ = GetIrqResolver(*compatible_list);
        interrupt_cells_ = decoder.num_interrupt_cells();
        return fit::ok(true);
      }
      error_ = true;
      return fit::failed();
    }
    return fit::ok(false);
  }

  // We need to match either the interrupt parent or interrupt controller as we move up the chain in
  // decoder.
  auto* current = &decoder;
  while (current != nullptr) {
    auto [interrupt_parent, interrupt_controller] =
        current->FindProperties("interrupt-parent", "interrupt-controller");

    // This node is the interrupt controller for the target device.
    if (interrupt_controller) {
      auto compatible =
          current->FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
      if (compatible) {
        irq_resolver_ = GetIrqResolver(*compatible);
        interrupt_cells_ = current->num_interrupt_cells();
        return fit::ok(true);
      }
      error_ = true;
      return fit::failed();
    }

    if (interrupt_parent) {
      if (auto irq_parent_phandle = interrupt_parent->AsUint32()) {
        interrupt_parent_ = irq_parent_phandle;
        return fit::ok(false);
      }
      error_ = true;
      return fit::failed();
    }

    current = current->parent();
  }

  return fit::ok(false);
}

}  // namespace boot_shim
