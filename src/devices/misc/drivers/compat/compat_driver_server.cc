// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/misc/drivers/compat/compat_driver_server.h"

#include <fidl/fuchsia.driver.framework/cpp/type_conversions.h>
#include <lib/driver/compat/cpp/symbols.h>
#include <lib/driver/component/cpp/internal/start_args.h>
#include <lib/driver/component/cpp/internal/symbols.h>

#include "src/devices/misc/drivers/compat/driver.h"

namespace compat {

CompatDriverServer::CompatDriverServer(fdf_dispatcher_t* dispatcher, fdf_handle_t server_handle)
    : dispatcher_(dispatcher) {
  binding_.emplace(dispatcher_,
                   fdf::ServerEnd<fuchsia_driver_framework::Driver>(fdf::Channel(server_handle)),
                   this, fidl::kIgnoreBindingClosure);
}

CompatDriverServer::~CompatDriverServer() {
  if (driver_.has_value()) {
    delete driver_.value();
  }
}

void* CompatDriverServer::initialize(fdf_handle_t server_handle) {
  fdf_dispatcher_t* dispatcher = fdf_dispatcher_get_current_dispatcher();
  CompatDriverServer* driver_server = new CompatDriverServer(dispatcher, server_handle);
  return driver_server;
}

void CompatDriverServer::destroy(void* token) {
  CompatDriverServer* driver_server = static_cast<CompatDriverServer*>(token);
  delete driver_server;
}

Driver* CompatDriverServer::CreateDriver(fuchsia_driver_framework::DriverStartArgs start_args,
                                         fdf::UnownedSynchronizedDispatcher driver_dispatcher,
                                         fdf::StartCompleter start_completer) {
  auto compat_device = fdf_internal::GetSymbol<const device_t*>(start_args.symbols(), kDeviceSymbol,
                                                                &kDefaultDevice);
  const zx_protocol_device_t* ops =
      fdf_internal::GetSymbol<const zx_protocol_device_t*>(start_args.symbols(), kOps);

  // Open the compat driver's binary within the package.
  auto compat = fdf_internal::ProgramValue(start_args.program(), "compat");
  if (compat.is_error()) {
    start_completer(compat.take_error());
    return nullptr;
  }
  auto node_properties = std::move(start_args.node_properties_2());
  zx::vmo config_vmo = std::move(start_args.config()).value_or(zx::vmo());
  auto driver =
      std::make_unique<Driver>(std::move(config_vmo), std::move(driver_dispatcher), *compat_device,
                               ops, "/pkg/" + *compat, std::move(node_properties));
  auto context = fdf::DriverContext(std::move(start_args));
  driver->Start(std::move(context), std::move(start_completer));
  return driver.release();
}

void CompatDriverServer::Start(StartRequestView request, fdf::Arena& arena,
                               StartCompleter::Sync& completer) {
  fuchsia_driver_framework::DriverStartArgs start_args = fidl::ToNatural(request->start_args);

  fdf::StartCompleter start_completer(
      [reply_arena = std::move(arena), reply_completer = completer.ToAsync()](
          zx::result<> result) mutable { reply_completer.buffer(reply_arena).Reply(result); });

  // Post a task to do this so that the WireServerDispatcher, the caller of this method,
  // can clean up correctly. Otherwise the destruction of the arena from the callback could
  // run too early, causing use-after-frees during the cleanup of the request.
  async::PostTask(fdf_dispatcher_get_async_dispatcher(dispatcher_),
                  [this, moved_start_args = std::move(start_args),
                   moved_start_completer = std::move(start_completer)]() mutable {
                    driver_.emplace(CreateDriver(std::move(moved_start_args),
                                                 fdf::UnownedSynchronizedDispatcher(dispatcher_),
                                                 std::move(moved_start_completer)));
                  });
}

void CompatDriverServer::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  ZX_ASSERT(driver_.has_value());
  driver_.value()->Stop(fdf::StopCompleter([this](zx::result<> result) { binding_.reset(); }));
}

void CompatDriverServer::Suspend(fdf::Arena& arena, SuspendCompleter::Sync& completer) {
  // The compat driver doesn't support automatic suspend yet.
  // Reply with success to avoid blocking the build or runtime.
  completer.buffer(arena).Reply(zx::ok());
}

void CompatDriverServer::Resume(ResumeRequestView request, fdf::Arena& arena,
                                ResumeCompleter::Sync& completer) {
  // The compat driver doesn't support automatic resume yet.
  // Reply with success to avoid blocking the build or runtime.
  completer.buffer(arena).Reply(zx::ok());
}

}  // namespace compat
