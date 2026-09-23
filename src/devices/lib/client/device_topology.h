// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_CLIENT_DEVICE_TOPOLOGY_H_
#define SRC_DEVICES_LIB_CLIENT_DEVICE_TOPOLOGY_H_

#include <fidl/fuchsia.device.fs/cpp/fidl.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>

namespace fdf_topology {

zx::result<std::string> GetTopologicalPath(fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
                                           const std::string &instance_name);

}  // namespace fdf_topology

#endif  // SRC_DEVICES_LIB_CLIENT_DEVICE_TOPOLOGY_H_
