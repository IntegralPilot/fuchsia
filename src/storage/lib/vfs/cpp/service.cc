// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/service.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <lib/zx/channel.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <utility>

namespace fs {

Service::Service(Connector connector) : connector_(std::move(connector)) {}

Service::~Service() = default;

fuchsia_io::NodeProtocolKinds Service::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kConnector;
}

zx_status_t Service::ConnectService(zx::channel channel) {
  if (!connector_) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  return connector_(std::move(channel));
}

}  // namespace fs
