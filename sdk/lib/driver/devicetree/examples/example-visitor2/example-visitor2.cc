// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "example-visitor2.h"

#include <lib/driver/devicetree/visitors/registration.h>

namespace example {

zx::result<> ExampleDriverVisitor2::DriverVisit(fdf_devicetree::Node& node,
                                                const devicetree::PropertyDecoder& decoder) {
  fuchsia_hardware_platform_bus::Metadata metadata{{
      .id = "example-metadata-2",
      .data = std::vector<uint8_t>{5, 6, 7, 8, 9},
  }};
  node.AddMetadata(std::move(metadata));
  return zx::ok();
}

}  // namespace example

REGISTER_DEVICETREE_VISITOR(example::ExampleDriverVisitor2);
