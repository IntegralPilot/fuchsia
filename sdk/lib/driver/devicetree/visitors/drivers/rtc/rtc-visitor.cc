// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "rtc-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/multivisitor.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <bind/fuchsia/cpp/bind.h>

namespace rtc_dt {

RtcVisitor::RtcVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(kRtcReference, 0u));
  rtc_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> RtcVisitor::Visit(fdf_devicetree::Node& node,
                               const devicetree::PropertyDecoder& decoder) {
  auto parser_output = rtc_parser_->Parse(node);
  if (parser_output.is_error()) {
    return parser_output.take_error();
  }

  std::optional<fdf_devicetree::References> rtc_references =
      parser_output->Get<fdf_devicetree::References>(kRtcReference);

  if (!rtc_references.has_value()) {
    return zx::ok();
  }

  for (auto& reference : *rtc_references) {
    auto result = ParseReferenceChild(node, reference.reference_node());
    if (result.is_error()) {
      return result.take_error();
    }
  }
  return zx::ok();
}

zx::result<> RtcVisitor::ParseReferenceChild(fdf_devicetree::Node& child,
                                             fdf_devicetree::ReferenceNode& parent) {
  std::vector bind_rules = {{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.hrtimer.Service"),
  }};

  std::vector bind_properties = {{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.hrtimer.Service"),
  }};

  child.AddNodeSpec(fuchsia_driver_framework::ParentSpec2(bind_rules, bind_properties));
  return zx::ok();
}

}  // namespace rtc_dt

REGISTER_DEVICETREE_VISITOR(rtc_dt::RtcVisitor);
