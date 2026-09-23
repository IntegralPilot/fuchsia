// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_CREATE_GOLDENS_MY_DEVICETREE_VISITOR_MY_DEVICETREE_VISITOR_H_
#define TOOLS_CREATE_GOLDENS_MY_DEVICETREE_VISITOR_MY_DEVICETREE_VISITOR_H_


#include <lib/driver/devicetree/manager/visitor.h>

namespace my_devicetree_visitor_dt {

class MyDevicetreeVisitor : public fdf_devicetree::Visitor {
 public:
  MyDevicetreeVisitor() = default;
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace my_devicetree_visitor_dt

#endif  // TOOLS_CREATE_GOLDENS_MY_DEVICETREE_VISITOR_MY_DEVICETREE_VISITOR_H_
