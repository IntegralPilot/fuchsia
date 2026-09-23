// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_COMPOSITE_NODE_SPEC_MANAGER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_COMPOSITE_NODE_SPEC_MANAGER_H_

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/zx/result.h>

#include <unordered_map>

#include "src/devices/bin/driver_manager/composite/composite_manager_bridge.h"

namespace driver_manager {
class Resource;
struct CompositeNodeAndDriver {
  fuchsia_driver_framework::wire::CompositeDriverInfo driver;
  NodeWkPtr node;
};

struct BindSpecResult {
  fidl::VectorView<fuchsia_driver_framework::wire::CompositeParent> bound_composite_parents;
  std::vector<CompositeNodeAndDriver> completed_node_and_drivers;
};

// This class is responsible for managing composite node specs. It keeps track of the specs
// and their matching composite driver and nodes. CompositeNodeSpecManager is owned by a
// CompositeManagerBridge and must be outlived by it.
class CompositeNodeSpecManager {
 public:
  using CompositeNodeSpecMap = std::unordered_map<std::string, std::unique_ptr<CompositeNodeSpec>>;

  explicit CompositeNodeSpecManager(CompositeManagerBridge* bridge);

  // Adds a composite node spec to the driver index. If it's successfully added, then the
  // CompositeNodeSpecManager stores the composite node spec in a map. After that, it sends a call
  // to CompositeManagerBridge to bind all unbound devices.
  void AddSpec(
      fuchsia_driver_framework::wire::CompositeNodeSpec fidl_spec,
      std::unique_ptr<CompositeNodeSpec> spec,
      fit::callback<void(fit::result<fuchsia_driver_framework::CompositeNodeSpecError>)> callback);

  // Binds the resource to the spec parents it was matched to. If |enable_multibind| is false,
  // CompositeNodeSpecManager will only bind the resource to the first unbound parent. Depending
  // on the implementation, completed_node_and_drivers will return an empty vector or a list of
  // completed CompositeNodeAndDrivers.
  zx::result<BindSpecResult> BindParentSpec(
      fidl::AnyArena& arena,
      fidl::VectorView<fuchsia_driver_framework::wire::CompositeParent> composite_parents,
      const ResourceWkPtr& resource, bool enable_multibind = false);

  void Rebind(std::string spec_name, std::optional<std::string> restart_driver_url_suffix,
              fit::callback<void(zx::result<>)> rebind_spec_completer);

  std::vector<fuchsia_driver_development::wire::CompositeNodeInfo> GetCompositeInfo(
      fidl::AnyArena& arena) const;

  void RecordInspect(inspect::Inspector& inspector) const;

  // Exposed for testing only.
  const CompositeNodeSpecMap& specs() const { return specs_; }

 private:
  void OnRequestRebindComplete(std::string spec_name,
                               fit::callback<void(zx::result<>)> rebind_spec_completer);

  // Contains all composite node specs. This maps the name to a CompositeNodeSpec object.
  CompositeNodeSpecMap specs_;

  // The owner of CompositeNodeSpecManager. CompositeManagerBridge must outlive
  // CompositeNodeSpecManager.
  CompositeManagerBridge* bridge_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_COMPOSITE_NODE_SPEC_MANAGER_H_
