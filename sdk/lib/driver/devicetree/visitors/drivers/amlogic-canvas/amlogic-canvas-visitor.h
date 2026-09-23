// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_AMLOGIC_CANVAS_AMLOGIC_CANVAS_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_AMLOGIC_CANVAS_AMLOGIC_CANVAS_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>

namespace amlogic_canvas_dt {

class AmlogicCanvasVisitor : public fdf_devicetree::Visitor {
 public:
  AmlogicCanvasVisitor() = default;
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child);
};

}  // namespace amlogic_canvas_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_AMLOGIC_CANVAS_AMLOGIC_CANVAS_VISITOR_H_
