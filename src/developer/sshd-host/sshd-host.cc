// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.boot/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/spawn.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/process.h>
#include <zircon/process.h>
#include <zircon/processargs.h>

#include "src/developer/sshd-host/authorized_keys.h"
#include "src/developer/sshd-host/service.h"

const uint16_t kPort = 22;
const char* kKeyGenArgs[] = {"/pkg/bin/hostkeygen", nullptr};

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithDispatcher(loop.dispatcher()).WithTags({"sshd-host"}).BuildAndInitialize();
  // We need to close PA_DIRECTORY_REQUEST otherwise clients that expect us to
  // offer services won't know that we've started and are not going to offer
  // any services.
  //
  // TODO(abarth): Instead of closing this handle, we should offer some
  // introspection services for debugging.
  {
    zx::handle((zx_take_startup_handle(PA_DIRECTORY_REQUEST)));
  }

  FX_LOG_KV(INFO, "sshd-host starting up");

  zx::result client_end = component::Connect<fuchsia_boot::Items>();
  if (!client_end.is_ok()) {
    FX_PLOGS(ERROR, client_end.status_value())
        << "Provisioning keys from boot item: failed to connect to boot items service";
    return client_end.status_value();
  }
  fidl::SyncClient boot_items{std::move(*client_end)};

  if (zx_status_t status = sshd_host::provision_authorized_keys_from_bootloader_file(boot_items);
      status != ZX_OK) {
    FX_LOG_KV(WARNING, "Failed to provision authorized_keys",
              FX_KV("status", zx_status_get_string(status)));
  }

  zx::process process;
  if (zx_status_t status = fdio_spawn(0, FDIO_SPAWN_CLONE_ALL, kKeyGenArgs[0], kKeyGenArgs,
                                      process.reset_and_get_address());
      status != ZX_OK) {
    FX_LOG_KV(FATAL, "Failed to spawn hostkeygen", FX_KV("status", zx_status_get_string(status)));
  }
  if (zx_status_t status = process.wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite(), nullptr);
      status != ZX_OK) {
    FX_LOG_KV(FATAL, "Failed to wait for hostkeygen",
              FX_KV("status", zx_status_get_string(status)));
  }
  zx_info_process_t info;
  if (zx_status_t status = process.get_info(ZX_INFO_PROCESS, &info, sizeof(info), nullptr, nullptr);
      status != ZX_OK) {
    FX_LOG_KV(FATAL, "Failed to get hostkeygen info",
              FX_KV("status", zx_status_get_string(status)));
  }

  if (info.return_code != 0) {
    FX_LOG_KV(FATAL, "Failed to generate host keys", FX_KV("return_code", info.return_code));
  }

  sshd_host::check_authorized_keys();

  uint16_t port = kPort;
  if (argc > 1) {
    int arg = atoi(argv[1]);
    if (arg <= 0) {
      FX_LOG_KV(ERROR, "Invalid port", FX_KV("argv[1]", argv[1]));
      return -1;
    }
    port = static_cast<uint16_t>(arg);
  }
  sshd_host::Service service(loop.dispatcher(), port);

  if (zx_status_t status = loop.Run(); status != ZX_OK) {
    FX_LOG_KV(FATAL, "Failed to run loop", FX_KV("status", zx_status_get_string(status)));
  }

  return 0;
}
