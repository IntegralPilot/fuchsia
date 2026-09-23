// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_MISC_DRIVERS_COMPAT_COMPAT_DRIVER_SERVER_H_
#define SRC_DEVICES_MISC_DRIVERS_COMPAT_COMPAT_DRIVER_SERVER_H_

#include <fidl/fuchsia.driver.framework/cpp/driver/natural_messaging.h>
#include <fidl/fuchsia.driver.framework/cpp/driver/wire.h>
#include <lib/driver/component/cpp/start_completer.h>

namespace compat {

// Forward declare.
class Driver;

class CompatDriverServer : public fdf::WireServer<fuchsia_driver_framework::Driver> {
 public:
  // Initialize the fuchsia_driver_framework::Driver server.
  static void* initialize(fdf_handle_t server_handle);

  // Destroy the fuchsia_driver_framework::Driver server.
  static void destroy(void* token);

  static Driver* CreateDriver(fuchsia_driver_framework::DriverStartArgs start_args,
                              fdf::UnownedSynchronizedDispatcher driver_dispatcher,
                              fdf::StartCompleter start_completer);

  CompatDriverServer(fdf_dispatcher_t* dispatcher, fdf_handle_t server_handle);

  virtual ~CompatDriverServer();

  void Start(StartRequestView request, fdf::Arena& arena, StartCompleter::Sync& completer) override;

  void Stop(fdf::Arena& arena, StopCompleter::Sync& completer) override;

  void Suspend(fdf::Arena& arena, SuspendCompleter::Sync& completer) override;

  void Resume(ResumeRequestView request, fdf::Arena& arena,
              ResumeCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_driver_framework::Driver> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  fdf_dispatcher_t* dispatcher_;
  std::optional<fdf::ServerBinding<fuchsia_driver_framework::Driver>> binding_;

  // Using a pointer for driver_ since it is an incomplete type here.
  std::optional<Driver*> driver_;
};
}  // namespace compat

#endif  // SRC_DEVICES_MISC_DRIVERS_COMPAT_COMPAT_DRIVER_SERVER_H_
