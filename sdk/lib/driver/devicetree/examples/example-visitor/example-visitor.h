// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_EXAMPLES_EXAMPLE_VISITOR_EXAMPLE_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_EXAMPLES_EXAMPLE_VISITOR_EXAMPLE_VISITOR_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>

namespace example {

class ExampleDriverVisitor : public fdf_devicetree::DriverVisitor {
 public:
  ExampleDriverVisitor() : DriverVisitor({"fuchsia,sample-device"}) {}

  zx::result<> DriverVisit(fdf_devicetree::Node& node,
                           const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace example

#endif  // LIB_DRIVER_DEVICETREE_EXAMPLES_EXAMPLE_VISITOR_EXAMPLE_VISITOR_H_
