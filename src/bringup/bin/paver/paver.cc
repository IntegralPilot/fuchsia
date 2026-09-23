// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/paver.h"

#include <fidl/fuchsia.process.lifecycle/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/wire/server.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/status.h>

#include "src/bringup/bin/paver/config.h"
#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/pave-logging.h"
#include "src/sys/lib/stdout-to-debuglog/cpp/stdout-to-debuglog.h"

#if defined(LEGACY_PAVER)
#include "src/firmware/paver/android.h"
#include "src/firmware/paver/astro.h"
#include "src/firmware/paver/iris.h"
#include "src/firmware/paver/luis.h"
#include "src/firmware/paver/moonflower.h"
#include "src/firmware/paver/nelson.h"
#include "src/firmware/paver/sherlock.h"
#include "src/firmware/paver/uefi.h"
#include "src/firmware/paver/vim3.h"
#elif defined(astro)
#include "src/firmware/paver/astro.h"
#elif defined(moonflower)
#include "src/firmware/paver/moonflower.h"
#elif defined(luis)
#include "src/firmware/paver/luis.h"
#elif defined(nelson)
#include "src/firmware/paver/nelson.h"
#elif defined(sherlock)
#include "src/firmware/paver/sherlock.h"
#elif defined(vim3)
#include "src/firmware/paver/vim3.h"
#elif defined(uefi)
#include "src/firmware/paver/uefi.h"
#elif defined(android)
#include "src/firmware/paver/android.h"
#elif defined(iris)
#include "src/firmware/paver/iris.h"
#endif

class LifecycleServer final : public fidl::WireServer<fuchsia_process_lifecycle::Lifecycle> {
 public:
  using FinishedCallback = fit::callback<void(zx_status_t status)>;
  using ShutdownCallback = fit::callback<void(FinishedCallback)>;

  explicit LifecycleServer(ShutdownCallback shutdown) : shutdown_(std::move(shutdown)) {}

  void Stop(StopCompleter::Sync& completer) override;

 private:
  ShutdownCallback shutdown_;
};

void LifecycleServer::Stop(StopCompleter::Sync& completer) {
  LOG("Received shutdown command over lifecycle interface");
  shutdown_([completer = completer.ToAsync()](zx_status_t status) mutable {
    if (status != ZX_OK) {
      ERROR("Shutdown failed: %s", zx_status_get_string(status));
    } else {
      LOG("Paver shutdown complete");
    }
    completer.Close(status);
  });
}

int main(int argc, char** argv) {
  zx_status_t status = StdoutToDebuglog::Init();
  if (status != ZX_OK) {
    LOG("Failed to redirect stdout to debuglog, assuming test environment and continuing\n");
  }
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  component::OutgoingDirectory outgoing(dispatcher);

  auto config = config::Config::TakeFromStartupHandle();
  paver::PaverConfig paver_config;
  paver_config.astro_sysconfig_abr_wear_leveling = config.astro_sysconfig_abr_wear_leveling();
  paver_config.zvb_current_slot = config.zvb_current_slot();
  paver_config.zvb_boot_partition_uuid = config.zvb_boot_partition_uuid();
  paver_config.android_boot_slot_suffix = config.android_boot_slot_suffix();
  if (config.merge_super_and_userdata()) {
    // NB: This name is a hard-coded value from the GPT component implementation
    // (//src/storage/gpt/component).
    // TODO(https://fxbug.dev/443980711): This should come from configuration.
    paver_config.system_partition_names.push_back("super_and_userdata");
  }

  zx::result paver = paver::Paver::Create(paver_config);
  if (paver.is_error()) {
    return paver.status_value();
  }
  paver->set_dispatcher(dispatcher);

#if defined(LEGACY_PAVER)
  // NOTE: Ordering matters!
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::AstroPartitionerFactory>());
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::NelsonPartitionerFactory>());
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::SherlockPartitionerFactory>());
  paver::DevicePartitionerFactory::Register(
      std::make_unique<paver::MoonflowerPartitionerFactory>());
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::LuisPartitionerFactory>());
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::Vim3PartitionerFactory>());
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::UefiPartitionerFactory>());
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::IrisPartitionerFactory>());
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::AndroidPartitionerFactory>());
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::DefaultPartitionerFactory>());
#elif defined(astro)
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::AstroPartitionerFactory>());
#elif defined(nelson)
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::NelsonPartitionerFactory>());
#elif defined(sherlock)
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::SherlockPartitionerFactory>());
#elif defined(moonflower)
  paver::DevicePartitionerFactory::Register(
      std::make_unique<paver::MoonflowerPartitionerFactory>());
#elif defined(luis)
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::LuisPartitionerFactory>());
#elif defined(vim3)
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::Vim3PartitionerFactory>());
#elif defined(uefi)
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::UefiPartitionerFactory>());
#elif defined(android)
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::AndroidPartitionerFactory>());
#elif defined(iris)
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::IrisPartitionerFactory>());
#else
  paver::DevicePartitionerFactory::Register(std::make_unique<paver::DefaultPartitionerFactory>());
#endif

  fidl::ServerBindingGroup<fuchsia_paver::Paver> bindings;
  zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_paver::Paver>(
      bindings.CreateHandler(paver.value().get(), dispatcher, fidl::kIgnoreBindingClosure));
  if (result.is_error()) {
    ERROR("paver: error: Failed add paver protocol: %s.\n", result.status_string());
    return 1;
  }

  result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    ERROR("paver: error: Failed to serve outgoing directory: %s.\n", result.status_string());
    return 1;
  }

  zx::channel lifecycle_channel = zx::channel(zx_take_startup_handle(PA_LIFECYCLE));
  if (!lifecycle_channel.is_valid()) {
    ERROR("PA_LIFECYCLE startup handle is required.");
    return 1;
  }
  fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle_request(
      std::move(lifecycle_channel));

  LifecycleServer lifecycle(
      fit::bind_member<&paver::Paver::LifecycleStopCallback>(paver.value().get()));
  fidl::ServerBinding lifecycle_binding(dispatcher, std::move(lifecycle_request), &lifecycle,
                                        fidl::kIgnoreBindingClosure);
  if (result.is_error()) {
    ERROR("paver: error: Failed add paver protocol: %s.\n", result.status_string());
    return 1;
  }

  return loop.Run();
}
