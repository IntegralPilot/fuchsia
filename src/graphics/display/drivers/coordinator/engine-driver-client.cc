// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/engine-driver-client.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/coordinator/engine-driver-client-fidl.h"

namespace display_coordinator {

namespace {

zx::result<std::unique_ptr<EngineDriverClient>> CreateFidlEngineDriverClient(
    fdf::Namespace& incoming) {
  zx::result<fdf::ClientEnd<fuchsia_hardware_display_engine::Engine>> connect_engine_client_result =
      incoming.Connect<fuchsia_hardware_display_engine::Service::Engine>();
  if (connect_engine_client_result.is_error()) {
    fdf::warn("Failed to connect to display engine FIDL client: {}", connect_engine_client_result);
    return connect_engine_client_result.take_error();
  }
  fdf::ClientEnd<fuchsia_hardware_display_engine::Engine> engine_client =
      std::move(connect_engine_client_result).value();

  if (!engine_client.is_valid()) {
    fdf::warn("Display engine FIDL device is invalid");
    return zx::error(ZX_ERR_BAD_HANDLE);
  }

  fbl::AllocChecker alloc_checker;
  auto engine_driver_client =
      fbl::make_unique_checked<EngineDriverClientFidl>(&alloc_checker, std::move(engine_client));
  if (!alloc_checker.check()) {
    fdf::warn("Failed to allocate memory for EngineDriverClientFidl");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(engine_driver_client));
}

}  // namespace

// static
zx::result<std::unique_ptr<EngineDriverClient>> EngineDriverClient::Create(
    std::shared_ptr<fdf::Namespace> incoming) {
  ZX_DEBUG_ASSERT(incoming != nullptr);

  // Attempt to connect to FIDL protocol.
  zx::result<std::unique_ptr<EngineDriverClient>> fidl_engine_driver_client_result =
      CreateFidlEngineDriverClient(*incoming);
  if (fidl_engine_driver_client_result.is_ok()) {
    fdf::info("Using the FIDL Engine driver client");
    return fidl_engine_driver_client_result.take_value();
  }
  fdf::error("Failed to create FIDL Engine driver client: {}", fidl_engine_driver_client_result);
  return fidl_engine_driver_client_result;
}

}  // namespace display_coordinator
