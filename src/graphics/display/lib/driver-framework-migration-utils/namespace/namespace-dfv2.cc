// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/namespace/namespace-dfv2.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdio/directory.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <string_view>

#include <fbl/alloc_checker.h>

namespace display {

// static
zx::result<std::unique_ptr<Namespace>> NamespaceDfv2::Create(fdf::Namespace* fdf_namespace) {
  ZX_DEBUG_ASSERT(fdf_namespace != nullptr);

  fbl::AllocChecker alloc_checker;
  auto namespace_dfv2 = fbl::make_unique_checked<NamespaceDfv2>(&alloc_checker, fdf_namespace);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for NamespaceDfv2.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(namespace_dfv2));
}

NamespaceDfv2::NamespaceDfv2(fdf::Namespace* fdf_namespace) : fdf_namespace_(*fdf_namespace) {
  ZX_DEBUG_ASSERT(fdf_namespace != nullptr);
}

zx::result<> NamespaceDfv2::ConnectServerEndToFidlProtocol(zx::channel server_end,
                                                           std::string_view service,
                                                           std::string_view service_member,
                                                           std::string_view instance) const {
  const std::string protocol_path =
      std::string(service).append("/").append(instance).append("/").append(service_member);
  zx_status_t status = fdio_service_connect_at(fdf_namespace_.svc_dir().handle()->get(),
                                               protocol_path.c_str(), server_end.release());
  return zx::make_result(status);
}

}  // namespace display
