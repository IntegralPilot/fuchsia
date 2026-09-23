// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "arm-gic-visitor.h"

#include <lib/driver/devicetree/visitors/interrupt-parser.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/stdcompat/array.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include "arm-gicv2.h"

namespace fpbus = fuchsia_hardware_platform_bus;
namespace {

constexpr auto kGicV1V2CompatibleDevices = cpp20::to_array<std::string_view>({
    // V1 and V2 compatible list
    "arm,arm11mp-gic",
    "arm,cortex-a15-gic",
    "arm,cortex-a7-gic",
    "arm,cortex-a5-gic",
    "arm,cortex-a9-gic",
    "arm,eb11mp-gic",
    "arm,gic-400",
    "arm,pl390",
    "arm,tc11mp-gic",
    "qcom,msm-8660-qgic",
    "qcom,msm-qgic2",
});

constexpr auto kGicV3CompatibleDevices = cpp20::to_array<std::string_view>({
    // V3 compatible list
    "arm,gic-v3",
});

std::vector<std::string> GetCompatibleList() {
  auto compatible_list =
      std::vector<std::string>(kGicV1V2CompatibleDevices.begin(), kGicV1V2CompatibleDevices.end());
  compatible_list.insert(compatible_list.end(), kGicV3CompatibleDevices.begin(),
                         kGicV3CompatibleDevices.end());
  return compatible_list;
}

bool IsArmGicV1V2(devicetree::StringList<> compatible_strings) {
  auto matched =
      std::find_first_of(compatible_strings.begin(), compatible_strings.end(),
                         kGicV1V2CompatibleDevices.begin(), kGicV1V2CompatibleDevices.end());
  return matched != compatible_strings.end();
}

bool IsArmGicV3(devicetree::StringList<> compatible_strings) {
  auto matched = std::find_first_of(compatible_strings.begin(), compatible_strings.end(),
                                    kGicV3CompatibleDevices.begin(), kGicV3CompatibleDevices.end());
  return matched != compatible_strings.end();
}

}  // namespace

namespace arm_gic_dt {

class InterruptPropertyV2 {
 public:
  static constexpr uint32_t kModeMask = 0x000F;

  explicit InterruptPropertyV2(fdf_devicetree::PropertyCells cells)
      : interrupt_cells_(cells, 1, 1, 1) {}

  // 1st cell contains the interrupt type; 0 for SPI interrupts, 1 for PPI interrupts.
  bool is_spi() { return *interrupt_cells_[0][0] == GIC_SPI; }

  // 2nd cell contains the interrupt number.
  // SPI interrupts are in the range [0-987].
  // PPI interrupts are in the range [0-15].
  uint32_t irq() {
    uint32_t irq = static_cast<uint32_t>(*interrupt_cells_[0][1]);
    if (is_spi()) {
      // SPI interrupts start at 32.
      // See https://developer.arm.com/documentation/101206/0003/Operation/Interrupt-types/SPIs.
      irq += 32;
    } else {
      // PPI interrupts start at 16.
      // See https://developer.arm.com/documentation/101206/0003/Operation/Interrupt-types/PPIs.
      irq += 16;
    }
    return irq;
  }

  // 3rd cell contains the flags.
  //     bits[3:0] contains trigger type and level.
  //        1 = low-to-high edge triggered
  //        2 = high-to-low edge triggered (invalid for SPI)
  //        4 = active high level-sensitive
  //        8 = active low level-sensitive (invalid for SPI).
  zx::result<fuchsia_hardware_platform_bus::ZirconInterruptMode> mode() {
    uint64_t mode = *interrupt_cells_[0][2];
    switch (mode & kModeMask) {
      case GIC_IRQ_MODE_EDGE_RISING:
        return zx::ok(fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeHigh);
      case GIC_IRQ_MODE_EDGE_FALLING:
        if (is_spi()) {
          fdf::error("Edge low mode not supported for SPI interrupt");

          return zx::error(ZX_ERR_INVALID_ARGS);
        }
        return zx::ok(fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeLow);
      case GIC_IRQ_MODE_LEVEL_HIGH:
        return zx::ok(fuchsia_hardware_platform_bus::ZirconInterruptMode::kLevelHigh);
      case GIC_IRQ_MODE_LEVEL_LOW:
        if (is_spi()) {
          fdf::error("Level low mode not supported for SPI interrupt");

          return zx::error(ZX_ERR_INVALID_ARGS);
        }
        return zx::ok(fuchsia_hardware_platform_bus::ZirconInterruptMode::kLevelLow);
      default:
        break;
    }

    fdf::error("Invalid mode {}", mode & kModeMask);

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

 private:
  using InterruptElement = devicetree::PropEncodedArrayElement<3>;
  devicetree::PropEncodedArray<InterruptElement> interrupt_cells_;
};

ArmGicVisitor::ArmGicVisitor() : fdf_devicetree::DriverVisitor(GetCompatibleList()) {}

zx::result<> ArmGicVisitor::Visit(fdf_devicetree::Node& node,
                                  const devicetree::PropertyDecoder& decoder) {
  zx::result<fdf_devicetree::ParsedProperties> properties = interrupt_parser_.Parse(node);
  if (properties.is_error()) {
    return properties.take_error();
  }

  // Interrupt parser converts all interrupts into kInterruptsExtended. No need to look for
  // kInterrupts property.
  auto interrupts = properties->Get<fdf_devicetree::References>(
      fdf_devicetree::InterruptParser::kInterruptsExtended);
  if (!interrupts) {
    return zx::ok();
  }

  auto interrupt_names =
      properties->Get<std::vector<std::string>>(fdf_devicetree::InterruptParser::kInterruptNames);
  auto wake_vector_names = properties->Get<std::vector<std::string>>(
      fdf_devicetree::InterruptParser::kFuchsiaInterruptWakeVectors);

  if (interrupt_names && (interrupts->size() != interrupt_names->size())) {
    // If `interrupt-names` property is present in the dts then we require that
    // it be the same size as the number of interrupts specified in the
    // `interrupts` property.
    fdf::error("Node '{}' has {} interrupts but {} interrupt names.", node.name(),
               interrupts->size(), interrupt_names->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Verify and then add any GIC interrupts we've parsed.
  for (uint32_t i = 0; i < interrupts->size(); i++) {
    auto& reference = (*interrupts)[i];
    auto& parent = reference.reference_node();
    auto cells = reference.property_cells();

    if (!is_match(parent.properties())) {
      continue;
    }

    std::optional<std::string> name;
    if (interrupt_names) {
      name = (*interrupt_names)[i];
    }

    zx::result result = ParseInterrupt(node.name(), parent, cells, name);
    if (result.is_error()) {
      return result.take_error();
    }

    fpbus::Irq irq = std::move(result.value());
    // If the node has a property for specifying a fuchsia interrupt wake vector then set an
    // interrupts wake capability based on whether or not we find their name referenced.
    if (name.has_value() && wake_vector_names) {
      auto it = std::find(wake_vector_names->begin(), wake_vector_names->end(), *name);
      if (it != wake_vector_names->end()) {
        irq.wake_vector() = true;
      }
    }

    node.AddIrq(irq);
  }

  return zx::ok();
}

zx::result<fpbus::Irq> ArmGicVisitor::ParseInterrupt(const std::string& node_name,
                                                     fdf_devicetree::ReferenceNode& parent,
                                                     fdf_devicetree::PropertyCells interrupt_cells,
                                                     std::optional<std::string> interrupt_name) {
  auto compatible_strings = parent.properties().at("compatible").AsStringList().value();
  if (IsArmGicV1V2(compatible_strings) &&
      (interrupt_cells.size_bytes() != (3 * sizeof(uint32_t)))) {
    // For GIC v2 3 cells are expected.
    fdf::error("Incorrect number of cells (expected {}, found {}) for interrupt in node '{}'",
               3 * sizeof(uint32_t), interrupt_cells.size_bytes(), node_name);

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (IsArmGicV3(compatible_strings) && (interrupt_cells.size_bytes() < (3 * sizeof(uint32_t)))) {
    // For GIC v3 at least 3 cells are expected. 4th cell if present represents the phandle of a
    // node that defines the CPU affinity for PPIs. This is not used in Fuchsia currently to
    // configure interrupts. 5th and above cells if present are reserved for future use and should
    // be ignored.
    fdf::error(
        "Incorrect number of cells (expected at least {}, found {}) for interrupt in node '{}'",
        3 * sizeof(uint32_t), interrupt_cells.size_bytes(), node_name);

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Both GIC V2 and V3 share the same cell specifiers for the first 3 cells.
  auto interrupt = InterruptPropertyV2(interrupt_cells.first(3 * sizeof(uint32_t)));

  zx::result mode = interrupt.mode();
  if (mode.is_error()) {
    fdf::error("Failed to parse mode for interrupt {} of node '{}' - {}", interrupt.irq(),
               node_name, mode);

    return mode.take_error();
  }

  fpbus::Irq irq = {{
      .irq = fpbus::IrqSpec::WithIrq(interrupt.irq()),
      .mode = *mode,
      .name = std::move(interrupt_name),
      .wake_vector = false,
  }};
  fdf::debug("IRQ {:#x} named '{}' with mode {:#x} added to node '{}'.", irq.irq()->irq().value(),
             irq.name().value_or("(no name)"), static_cast<uint32_t>(*irq.mode()), node_name);

  return zx::ok(irq);
}

}  // namespace arm_gic_dt

REGISTER_DEVICETREE_VISITOR(arm_gic_dt::ArmGicVisitor);
