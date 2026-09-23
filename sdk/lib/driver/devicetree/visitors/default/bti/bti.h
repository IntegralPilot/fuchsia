// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_BTI_BTI_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_BTI_BTI_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <memory>
#include <set>

namespace fdf_devicetree {

// The |BtiVisitor| populates the BTI properties of each device tree
// node based on the "iommus" property if present.
class BtiVisitor : public Visitor {
 public:
  BtiVisitor();
  ~BtiVisitor() override = default;
  zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override;
  zx::result<> FinalizeNode(Node& node) override;

  static bool IsIommu(std::string_view node_name) { return node_name.starts_with("iommu"); }

 private:
  zx::result<> ReferenceChildVisit(Node& child, ReferenceNode& parent,
                                   PropertyCells reference_cells,
                                   std::optional<std::string> iommu_name);

  std::set<Phandle> iommu_nodes_;
  std::unique_ptr<PropertyParser> reference_parser_;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_BTI_BTI_H_
