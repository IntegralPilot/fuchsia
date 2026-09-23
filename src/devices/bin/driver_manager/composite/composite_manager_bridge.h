// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_COMPOSITE_MANAGER_BRIDGE_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_COMPOSITE_MANAGER_BRIDGE_H_

#include "src/devices/bin/driver_manager/composite/composite_node_spec.h"

namespace driver_manager {
using AddToIndexCallback = fit::callback<void(zx::result<>)>;

// Bridge class for the a driver manager to handle the composite nodes.
class CompositeManagerBridge {
 public:
  virtual ~CompositeManagerBridge() = default;

  // Match and bind all unbound nodes. Called by the CompositeNodeManager
  // after a composite node spec is matched to a composite driver.
  virtual void BindNodesForCompositeNodeSpec() = 0;

  virtual void AddSpecToDriverIndex(fuchsia_driver_framework::wire::CompositeNodeSpec spec,
                                    AddToIndexCallback callback) = 0;

  virtual void RequestRebindFromDriverIndex(std::string spec,
                                            std::optional<std::string> driver_url_suffix,
                                            fit::callback<void(zx::result<>)> callback) {
    callback(zx::error(ZX_ERR_NOT_SUPPORTED));
  }
};
}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_COMPOSITE_MANAGER_BRIDGE_H_
