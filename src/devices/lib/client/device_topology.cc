// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/client/device_topology.h"

#include <fidl/fuchsia.device.fs/cpp/fidl.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>

namespace fdf_topology {

zx::result<std::string> GetTopologicalPath(fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
                                           const std::string& instance_name) {
  std::string filename = instance_name + "/" + fuchsia_device_fs::kDeviceTopologyName;
  zx::result info_client_end =
      component::ConnectAt<fuchsia_device_fs::TopologicalPath>(dir, filename);
  if (!info_client_end.is_ok()) {
    return info_client_end.take_error();
  }
  fidl::WireResult result = fidl::WireCall(info_client_end.value())->GetTopologicalPath();
  if (!result.ok()) {
    printf("failed to get topological path %s: %s\n", filename.c_str(), result.status_string());
    return zx::error(result.status());
  }
  auto& resp = result.value();
  if (resp.is_error()) {
    printf("GetTopologicalPath returned error %s: %s\n", filename.c_str(),
           zx_status_get_string(resp.error_value()));
    return resp.take_error();
  }
  return zx::ok(resp.value()->path.get());
}

}  // namespace fdf_topology
