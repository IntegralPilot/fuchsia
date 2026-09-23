// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/devicetree/visitors/drivers/gpt/gpt-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <zircon/errors.h>

#include <bind/fuchsia/block/gpt/cpp/bind.h>

namespace gpt_dt {

zx::result<> GptVisitor::Visit(fdf_devicetree::Node& node,
                               const devicetree::PropertyDecoder& decoder) {
  if (node.properties().find("partition-names") == node.properties().end()) {
    return zx::ok();
  }
  auto partition_names = node.GetProperty<std::vector<std::string>>("partition-names");
  if (partition_names.is_error()) {
    return partition_names.take_error();
  }
  for (const auto& partition_name : partition_names.value()) {
    node.AddNodeSpec({{
        .bind_rules =
            {
                fdf::MakeAcceptBindRule(bind_fuchsia_block_gpt::PARTITION_NAME, partition_name),
            },
        .properties =
            {
                fdf::MakeProperty2(bind_fuchsia_block_gpt::PARTITION_NAME, partition_name),
            },
    }});
    fdf::info("Adding partition {} node under {}", partition_name, node.name());
  }
  return zx::ok();
}

}  // namespace gpt_dt

REGISTER_DEVICETREE_VISITOR(gpt_dt::GptVisitor);
