// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_DRIVER_HOST_DRIVER_HOST_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_DRIVER_HOST_DRIVER_HOST_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace fdf_devicetree {

// The |DriverHostVisitor| provides information about which driver hosts to colocate with.
class DriverHostVisitor : public Visitor {
 public:
  explicit DriverHostVisitor() = default;
  ~DriverHostVisitor() override = default;
  zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_DRIVER_HOST_DRIVER_HOST_H_
