// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.driver.test/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <fbl/unique_fd.h>

int main() {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"simple_driver_test_realm"}).BuildAndInitialize();

  auto client_end = component::Connect<fuchsia_driver_test::Realm>();
  if (!client_end.is_ok()) {
    FX_LOG_KV(ERROR, "Failed to connect to Realm FIDL", FX_KV("error", client_end.error_value()));
    return 1;
  }
  fidl::WireSyncClient client{std::move(*client_end)};

  fidl::Arena arena;
  auto wire_result = client->Start(fuchsia_driver_test::wire::RealmArgs(arena));
  if (wire_result.status() != ZX_OK) {
    FX_LOG_KV(ERROR, "Failed to connect to Realm:Start", FX_KV("error", wire_result.status()));
    return 1;
  }
  if (wire_result.value().is_error()) {
    FX_LOG_KV(ERROR, "Realm:Start failed", FX_KV("error", wire_result.value().error_value()));
    return 1;
  }

  // Wait until /dev/sys/test has been populated so we know the test realm has been set up.
  // Otherwise, tearing down the realm at the end of a test can race with asynchronous operations
  // that occur during setup (see b/316579125).
  fbl::unique_fd dev(open("/dev", O_RDONLY));
  if (!dev) {
    FX_LOG_KV(ERROR, "Failed to open /dev");
    return 1;
  }
  if (zx_status_t status =
          device_watcher::RecursiveWaitForFile(dev.get(), "sys/test").status_value();
      status != ZX_OK) {
    FX_LOG_KV(ERROR, "Failed to open /dev/sys/test");
    return 1;
  }
  return 0;
}
