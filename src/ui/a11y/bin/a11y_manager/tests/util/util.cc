// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/bin/a11y_manager/tests/util/util.h"

#include <fuchsia/accessibility/semantics/cpp/fidl.h>
#include <stdint.h>

#include <optional>
#include <string>
#include <vector>

namespace accessibility_test {

fuchsia::accessibility::semantics::Node CreateTestNode(uint32_t node_id,
                                                       std::optional<std::string> label,
                                                       std::vector<uint32_t> child_ids) {
  fuchsia::accessibility::semantics::Node node = fuchsia::accessibility::semantics::Node();
  node.set_node_id(node_id);
  if (!child_ids.empty()) {
    node.set_child_ids(std::move(child_ids));
  }
  node.set_role(fuchsia::accessibility::semantics::Role::UNKNOWN);
  node.set_attributes(fuchsia::accessibility::semantics::Attributes());
  if (label) {
    node.mutable_attributes()->set_label(std::move(label.value()));
  }
  fuchsia::ui::gfx::BoundingBox box;
  node.set_location(box);
  return node;
}

}  // namespace accessibility_test
