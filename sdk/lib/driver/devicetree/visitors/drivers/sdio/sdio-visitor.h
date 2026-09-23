// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_SDIO_SDIO_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_SDIO_SDIO_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>

namespace sdio_dt {

class SdioVisitor : public fdf_devicetree::Visitor {
 public:
  SdioVisitor() = default;
  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  bool is_match(fdf_devicetree::Node& node);
  zx::result<> ParseChild(fdf_devicetree::Node& parent, fdf_devicetree::ChildNode& child);
};

}  // namespace sdio_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_SDIO_SDIO_VISITOR_H_
