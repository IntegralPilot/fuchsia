// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_SMC_SMC_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_SMC_SMC_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace fdf_devicetree {

// The |SmcVisitor| populates the smc entry of each device tree
// node based on the "smcs" property if present.
class SmcVisitor : public Visitor {
 public:
  SmcVisitor();
  ~SmcVisitor() override = default;
  zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override;

 private:
  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_SMC_SMC_H_
