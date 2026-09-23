// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_EXAMPLES_EXAMPLE_VISITOR2_EXAMPLE_VISITOR2_H_
#define LIB_DRIVER_DEVICETREE_EXAMPLES_EXAMPLE_VISITOR2_EXAMPLE_VISITOR2_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>

namespace example {

class ExampleDriverVisitor2 : public fdf_devicetree::DriverVisitor {
 public:
  ExampleDriverVisitor2() : DriverVisitor({"fuchsia,sample-device"}) {}

  zx::result<> DriverVisit(fdf_devicetree::Node& node,
                           const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace example

#endif  // LIB_DRIVER_DEVICETREE_EXAMPLES_EXAMPLE_VISITOR2_EXAMPLE_VISITOR2_H_
