// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/namespace/namespace.h"

#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <string_view>

namespace display {

zx::result<zx::channel> Namespace::ConnectToFidlProtocol(std::string_view service,
                                                         std::string_view service_member,
                                                         std::string_view instance) const {
  zx::channel client_end, server_end;
  zx_status_t status = zx::channel::create(/*flags=*/0, &client_end, &server_end);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  zx::result<> connect_result =
      ConnectServerEndToFidlProtocol(std::move(server_end), service, service_member, instance);
  if (connect_result.is_error()) {
    return connect_result.take_error();
  }

  return zx::ok(std::move(client_end));
}

}  // namespace display
