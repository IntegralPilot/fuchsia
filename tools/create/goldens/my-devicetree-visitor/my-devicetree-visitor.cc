// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "my-devicetree-visitor.h"

#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

namespace my_devicetree_visitor_dt {

zx::result<> MyDevicetreeVisitor::Visit(fdf_devicetree::Node& node,
                                   const devicetree::PropertyDecoder& decoder) {
  return zx::ok();
}

}  // namespace my_devicetree_visitor_dt

REGISTER_DEVICETREE_VISITOR(my_devicetree_visitor_dt::MyDevicetreeVisitor);
