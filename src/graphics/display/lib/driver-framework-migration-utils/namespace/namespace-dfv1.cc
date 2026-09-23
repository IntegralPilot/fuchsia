// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/namespace/namespace-dfv1.h"

#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <string_view>

#include <fbl/alloc_checker.h>

namespace display {

// static
zx::result<std::unique_ptr<Namespace>> NamespaceDfv1::Create(zx_device_t* device) {
  ZX_DEBUG_ASSERT(device != nullptr);

  fbl::AllocChecker alloc_checker;
  auto incoming = fbl::make_unique_checked<NamespaceDfv1>(&alloc_checker, device);
  if (!alloc_checker.check()) {
    zxlogf(ERROR, "Failed to allocate memory for NamespaceDfv1.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(incoming));
}

NamespaceDfv1::NamespaceDfv1(zx_device_t* device) : device_(device) {
  ZX_DEBUG_ASSERT(device != nullptr);
}

zx::result<> NamespaceDfv1::ConnectServerEndToFidlProtocol(zx::channel server_end,
                                                           std::string_view service,
                                                           std::string_view service_member,
                                                           std::string_view instance) const {
  zx_status_t status = device_connect_fragment_fidl_protocol(
      device_, instance.data(), service.data(), service_member.data(), server_end.release());

  return zx::make_result(status);
}

}  // namespace display
