// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BUS_DRIVERS_PLATFORM_NODE_UTIL_H_
#define SRC_DEVICES_BUS_DRIVERS_PLATFORM_NODE_UTIL_H_

#include <fidl/fuchsia.hardware.platform.bus/cpp/natural_types.h>

#include <optional>

namespace platform_bus {

constexpr bool IsValid(const fuchsia_hardware_platform_bus::Mmio& mmio) {
  // Both or neither must be set.
  return (mmio.base() != std::nullopt) == (mmio.length() != std::nullopt);
}

constexpr bool IsValid(const fuchsia_hardware_platform_bus::Irq& irq) {
  return (irq.irq() != std::nullopt) && (irq.mode() != std::nullopt);
}

constexpr bool IsValid(const fuchsia_hardware_platform_bus::Bti& bti) {
  return (bti.bti_id() != std::nullopt) && (bti.iommu_id() != std::nullopt);
}

constexpr bool IsValid(const fuchsia_hardware_platform_bus::Smc& smc) {
  return (smc.count() != std::nullopt) && (smc.service_call_num_base() != std::nullopt) &&
         (smc.exclusive() != std::nullopt);
}

constexpr bool IsValid(const fuchsia_hardware_platform_bus::Metadata& meta) {
  return (meta.data() != std::nullopt) && (meta.id() != std::nullopt);
}

constexpr bool IsValid(const fuchsia_hardware_platform_bus::BootMetadata& meta) {
  return (meta.zbi_extra() != std::nullopt) && (meta.zbi_type() != std::nullopt);
}

// Returns the index of the resource matching the resource name.
std::optional<uint32_t> GetMmioIndex(const fuchsia_hardware_platform_bus::Node& node,
                                     std::string_view mmio_name);
std::optional<uint32_t> GetIrqIndex(const fuchsia_hardware_platform_bus::Node& node,
                                    std::string_view irq_name);
std::optional<uint32_t> GetBtiIndex(const fuchsia_hardware_platform_bus::Node& node,
                                    std::string_view bti_name);
std::optional<uint32_t> GetSmcIndex(const fuchsia_hardware_platform_bus::Node& node,
                                    std::string_view smc_name);

}  // namespace platform_bus

#endif  // SRC_DEVICES_BUS_DRIVERS_PLATFORM_NODE_UTIL_H_
