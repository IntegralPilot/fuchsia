// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_FUCHSIA_CONFIG_FUCHSIA_CONFIG_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_FUCHSIA_CONFIG_FUCHSIA_CONFIG_H_

#include <lib/driver/devicetree/manager/visitor.h>

namespace fdf_devicetree {

class FuchsiaConfigVisitor : public fdf_devicetree::Visitor {
 public:
  FuchsiaConfigVisitor() = default;
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DEFAULT_FUCHSIA_CONFIG_FUCHSIA_CONFIG_H_
