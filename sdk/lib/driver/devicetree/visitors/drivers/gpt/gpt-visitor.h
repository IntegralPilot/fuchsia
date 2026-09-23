// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_GPT_GPT_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_GPT_GPT_VISITOR_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <cstdint>

namespace gpt_dt {

class GptVisitor : public fdf_devicetree::Visitor {
 public:
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace gpt_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_GPT_GPT_VISITOR_H_
