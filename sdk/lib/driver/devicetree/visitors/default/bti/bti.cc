// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "lib/driver/devicetree/visitors/default/bti/bti.h"

#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/devicetree/devicetree.h>
#include <lib/driver/logging/cpp/logger.h>

#include <cstdint>
#include <optional>

namespace fdf_devicetree {

constexpr const char kBtiProp[] = "iommus";
constexpr const char kBtiNameProp[] = "iommu-names";
constexpr const char kIommuCellsProp[] = "#iommu-cells";

// #iommu-cells == 1.
// This is because fuchsia_hardware_platform_bus::Bti only takes a
// bti_id as a specifier which is u32. Therefore iommu specifier should be 1 cell wide.
constexpr const uint32_t kIommuCellSize = 1;

class IommuCell {
 public:
  using IommuPropertyElement = devicetree::PropEncodedArrayElement<kIommuCellSize>;

  explicit IommuCell(PropertyCells cells) : property_array_(cells, kIommuCellSize) {}

  uint32_t bti_id() {
    IommuPropertyElement element = property_array_[0];
    std::optional<uint64_t> cell = element[0];
    return static_cast<uint32_t>(*cell);
  }

 private:
  devicetree::PropEncodedArray<IommuPropertyElement> property_array_;
};

BtiVisitor::BtiVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kBtiProp, kIommuCellsProp, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kBtiNameProp, /* required */ false));
  reference_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> BtiVisitor::Visit(Node& node, const devicetree::PropertyDecoder& decoder) {
  zx::result parser_output = reference_parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Failed to parse reference for node '{}'", node.name());

    return parser_output.take_error();
  }

  auto btis = parser_output->Get<References>(kBtiProp);
  if (!btis) {
    return zx::ok();
  }

  auto iommu_names = parser_output->Get<std::vector<std::string>>(kBtiNameProp);
  if (iommu_names && iommu_names->size() > btis->size()) {
    fdf::error("Node '{}' has {} iommu entries but has {} iommu names.", node.name(), btis->size(),
               iommu_names->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  for (uint32_t index = 0; index < btis->size(); index++) {
    auto& reference = (*btis)[index];
    if (IsIommu(reference.reference_node().name())) {
      std::optional<std::string> name;
      if (iommu_names && index < iommu_names->size()) {
        name = (*iommu_names)[index];
      }
      auto result =
          ReferenceChildVisit(node, reference.reference_node(), reference.property_cells(), name);
      if (result.is_error()) {
        return result.take_error();
      }
    } else {
      fdf::warn("Node {} iommu references illegal parent: {}", node.name(),
                reference.reference_node().name());
    }
  }

  return zx::ok();
}

zx::result<> BtiVisitor::ReferenceChildVisit(Node& child, ReferenceNode& parent,
                                             PropertyCells reference_cells,
                                             std::optional<std::string> iommu_name) {
  const uint32_t iommu_id = parent.id();
  iommu_nodes_.insert(iommu_id);

  auto iommu_cell = IommuCell(reference_cells);

  fuchsia_hardware_platform_bus::Bti bti = {{
      .iommu_id = iommu_id,
      .bti_id = iommu_cell.bti_id(),
      .name = std::move(iommu_name),
  }};
  fdf::debug("BTI {} index: {:#x}, id: {:#x}, added to node '{}'.",
             bti.name().value_or("(no name)"), *bti.iommu_id(), *bti.bti_id(), child.name());

  child.AddBti(bti);

  return zx::ok();
}

zx::result<> BtiVisitor::FinalizeNode(Node& node) {
  uint32_t iommu_id = node.id();
  if (iommu_nodes_.find(iommu_id) != iommu_nodes_.end()) {
    auto reg = node.GetProperty<std::vector<uint32_t>>("reg");
    if (reg.is_ok() && !reg->empty()) {
      std::ignore = node.RegisterIommu(
          node.id(), fuchsia_hardware_platform_bus::Iommu::WithArmSmmu(reg.value()[0]));
    } else {
      std::ignore =
          node.RegisterIommu(node.id(), fuchsia_hardware_platform_bus::Iommu::WithStubIommu({}));
    }
  }
  return zx::ok();
}

}  // namespace fdf_devicetree
