// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_MANAGER_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_MANAGER_VISITOR_H_

#include <lib/devicetree/devicetree.h>
#include <lib/zx/result.h>

#include <string_view>

#include "lib/driver/devicetree/manager/node.h"

namespace fdf_devicetree {

// A visitor is a class that visits nodes in the devicetree.
// See |Manager::Walk()| for more information.
class Visitor {
 public:
  explicit Visitor() = default;
  virtual ~Visitor() = default;

  // Visit method called during devicetree walk.
  virtual zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) = 0;

  // Method called after all visitors have visited the node once and
  // all references are registered. Any final updates to the node metadata
  // can be done in this method.
  virtual zx::result<> FinalizeNode(Node& node) { return zx::ok(); }
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_MANAGER_VISITOR_H_
