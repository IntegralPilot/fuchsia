// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.memorypressure/cpp/fidl.h>
#include <fidl/fuchsia.process.lifecycle/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/processargs.h>

#include "src/graphics/bin/vulkan_loader/app.h"
#include "src/graphics/bin/vulkan_loader/icd_runner.h"
#include "src/graphics/bin/vulkan_loader/loader.h"
#include "src/graphics/bin/vulkan_loader/magma_dependency_injection.h"
#include "src/graphics/bin/vulkan_loader/structured_config_lib.h"
#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"

namespace {
zx::result<fidl::ClientEnd<fuchsia_memorypressure::Provider>> GetMemoryPressureProvider() {
  auto endpoints = fidl::CreateEndpoints<fuchsia_memorypressure::Provider>();
  if (endpoints.is_error()) {
    FX_LOGS(INFO) << "Failed to create endpoints: " << endpoints.status_string();
    return endpoints.take_error();
  }
  if (auto result = component::Connect(std::move(endpoints->server)); result.is_error()) {
    return result.take_error();
  }
  return zx::ok(std::move(endpoints->client));
}

class LifecycleHandler : public fidl::Server<fuchsia_process_lifecycle::Lifecycle> {
 public:
  static LifecycleHandler Create(async::Loop* loop) {
    fidl::ServerEnd server_end = fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle>{
        zx::channel(zx_take_startup_handle(PA_LIFECYCLE))};
    ZX_ASSERT_MSG(server_end.is_valid(), "Invalid handle for PA_LIFECYCLE!");
    return LifecycleHandler(loop, std::move(server_end));
  }

 private:
  explicit LifecycleHandler(async::Loop* loop,
                            fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> server_end)
      : loop_(loop),
        binding_(loop->dispatcher(), std::move(server_end), this, fidl::kIgnoreBindingClosure) {}
  void Stop(StopCompleter::Sync& completer) override {
    loop_->Quit();
    binding_.Close(ZX_OK);
  }

  async::Loop* loop_;
  fidl::ServerBinding<fuchsia_process_lifecycle::Lifecycle> binding_;
};
}  // namespace

int main(int argc, const char* const* argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async::Loop runner_loop(&kAsyncLoopConfigNoAttachToCurrentThread);

  LifecycleHandler lifecycle_handler = LifecycleHandler::Create(&loop);

  runner_loop.StartThread("IcdRunner");
  fxl::SetLogSettingsFromCommandLine(fxl::CommandLineFromArgcArgv(argc, argv));

  auto outgoing_dir = component::OutgoingDirectory(loop.dispatcher());
  auto structured_config = structured_config_lib::Config::TakeFromStartupHandle();

  LoaderApp app(&outgoing_dir, loop.dispatcher(), structured_config);
  zx_status_t status = app.InitDeviceWatcher();

  if (status != ZX_OK) {
    FX_LOGS(INFO) << "Failed to initialize device watcher " << status;
    return -1;
  }

  status = app.InitDebugFs();

  if (status != ZX_OK) {
    FX_LOGS(INFO) << "Failed to initialize debug fs " << status;
    return -1;
  }

  zx::result manager =
      MagmaDependencyInjection::Create(loop.dispatcher(), &GetMemoryPressureProvider);
  if (manager.is_error()) {
    FX_LOGS(INFO) << "Failed to initialize gpu manager " << manager.status_string();
    return -1;
  }

  auto component_runner = std::make_unique<IcdRunnerImpl>(runner_loop.dispatcher());
  if (auto status = IcdRunnerImpl::Add(std::move(component_runner), outgoing_dir);
      status.is_error()) {
    FX_LOGS(ERROR) << "Failed to add ICD runner: " << status.status_string();
    return -1;
  }

  if (auto result = LoaderImpl::Add(outgoing_dir, &app, loop.dispatcher()); result.is_error()) {
    FX_LOGS(ERROR) << "Failed to create loader service: " << result.status_string();
  }

  if (auto status = outgoing_dir.ServeFromStartupInfo(); status.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << status.status_string();
    return -1;
  }

  FX_LOGS(INFO) << "Vulkan loader initialized.";
  loop.Run();
  runner_loop.Shutdown();
  loop.Shutdown();
  return 0;
}
