// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_BIND_PROPERTY_BIND_PROPERTY_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_BIND_PROPERTY_BIND_PROPERTY_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/zx/result.h>

namespace fdf_devicetree {

// The |BindPropertyVisitor| populates the bind properties of each device tree
// node based on the "compatible" string.
// TODO(https://fxbug.dev/42058369): support extra "bind,..." properties as bind properties.
class BindPropertyVisitor : public Visitor {
 public:
  explicit BindPropertyVisitor() : Visitor() {}
  ~BindPropertyVisitor() override = default;
  zx::result<> Visit(Node& node, const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_BIND_PROPERTY_BIND_PROPERTY_H_
