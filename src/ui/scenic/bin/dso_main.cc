// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/scheduler/role.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>
#include <lib/zx/thread.h>

#include <memory>

#include "src/lib/dso/cpp/async.h"
#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/ui/scenic/bin/app.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/scenic_structured_config.h"

int dso_main_async(int argc, const char* argv[], const char* envp[], zx_handle_t svc_handle,
                   zx_handle_t pkg_handle, zx_handle_t directory_request_handle,
                   zx_handle_t lifecycle_handle, zx_handle_t config_handle,
                   fdf_dispatcher_t* fdf_dispatcher) {
  if (svc_handle == ZX_HANDLE_INVALID || pkg_handle == ZX_HANDLE_INVALID ||
      directory_request_handle == ZX_HANDLE_INVALID || config_handle == ZX_HANDLE_INVALID ||
      lifecycle_handle == ZX_HANDLE_INVALID) {
    return 1;
  }
  zx::channel lifecycle(lifecycle_handle);

  // This call creates ComponentContext, but does not start serving immediately. Outgoing directory
  // is served by App, after App::InitializeServices() is completed.
  auto svc_dir = std::make_shared<sys::ServiceDirectory>(zx::channel(svc_handle));
  zx::channel pkg_dir(pkg_handle);
  zx::channel out_dir(directory_request_handle);
  zx::vmo config_vmo(config_handle);
  auto config = scenic_structured_config::Config::CreateFromVmo(std::move(config_vmo));
  if (config.prefetch()) {
    scenic_impl::PrefetchBinary(pkg_handle, "lib/libscenic.so");
  }

  if (!config.use_separate_input_thread()) {
    FX_LOGS(ERROR) << "Scenic DSO requires use_separate_input_thread to be true";
    return 8;
  }

  async_dispatcher_t* const input_dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);
  if (input_dispatcher == nullptr) {
    return 2;
  }

  async::Loop render_loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  fxl::LogSettings base_settings;
  if (!ParseLogSettings(command_line, &base_settings)) {
    return 3;
  }

  // Don't setup a trace provider, driver bindings will do that for us.

  // This call creates ComponentContext, but does not start serving immediately. Outgoing directory
  // is served by App, after App::InitializeServices() is completed.
  auto app_context = std::make_unique<sys::ComponentContext>(svc_dir);

  // Set up an inspect::Node to inject into the App.
  auto* const inspector = new inspect::ComponentInspector(render_loop.dispatcher(), {});

  zx::channel log_client, log_server;
  zx::channel::create(0, &log_client, &log_server);
  zx_status_t s = svc_dir->Connect("fuchsia.logger.LogSink", std::move(log_server));
  if (s != ZX_OK) {
    return 4;
  }
  fuchsia_logging::LogSettingsBuilder log_settings;
  log_settings.WithMinLogSeverity(base_settings.min_log_level);
  log_settings.WithDispatcher(nullptr);
  log_settings.WithLogSink(log_client.release());
  log_settings.DisableInterestListener();
  log_settings.BuildAndInitialize();
  FX_LOGS(INFO) << "Started";

  // Set up an inspect::Node to inject into the App.
  auto [inspect_client, inspect_server] = *fidl::CreateEndpoints<fuchsia_inspect::InspectSink>();
  s = svc_dir->Connect("fuchsia.inspect.InspectSink", inspect_server.TakeChannel());
  if (s != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to create inspector: " << zx_status_get_string(s);
    return 6;
  }
  inspect::PublishOptions opts;
  opts.client_end.emplace(std::move(inspect_client));

  component::SyncServiceMemberWatcher<fuchsia_hardware_display::Service::Provider> watcher(
      fidl::UnownedClientEnd<fuchsia_io::Directory>(svc_dir->unowned_channel()));
  zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> provider_result =
      watcher.GetNextInstance(/*stop_at_idle=*/false);
  if (provider_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to display provider: " << provider_result.status_string();
    return 7;
  }
  fidl::ClientEnd<fuchsia_hardware_display::Provider> provider = std::move(provider_result).value();
  auto display_coordinator_promise = display::GetCoordinator(std::move(provider));

  // Setup the input thread dispatcher, which runs on the fdf_dispatcher provided to dso_main.
  utils::ScopedThreadDispatcherSetter setter(render_loop.dispatcher(), input_dispatcher);
  async::PostTask(input_dispatcher, [input_dispatcher]() {
    async_dispatcher_t* const current_dispatcher = async_get_default_dispatcher();
    if (current_dispatcher == nullptr) {
      async_set_default_dispatcher(input_dispatcher);
    } else {
      FX_CHECK(current_dispatcher == input_dispatcher) << "input thread dispatcher is wrong";
    }
  });

  // Instantiate Scenic app.
  // TODO(https://fxbug.dev/485919515): Free `app` and `inspector` when the program terminates. It
  // is only safe to free `app` once `input_dispatcher` is no longer running code that may access
  // it. Not freeing this is a simple way to avoid a use after free during shutdown.
  auto* const app = new scenic_impl::App{render_loop.dispatcher(),
                                         input_dispatcher,
                                         std::move(app_context),
                                         fidl::ClientEnd<fuchsia_io::Directory>(std::move(pkg_dir)),
                                         fidl::ServerEnd<fuchsia_io::Directory>(std::move(out_dir)),
                                         std::move(config),
                                         inspector->root(),
                                         std::move(display_coordinator_promise),
                                         [&render_loop]() { render_loop.Quit(); }};

  // Apply the scheduler role defined for Scenic.
  const zx_status_t thread_status = fuchsia_scheduler::SetRoleForThisThread("fuchsia.scenic.main");
  if (thread_status != ZX_OK) {
    FX_LOGS(WARNING) << "Failed to apply profile to main thread: " << thread_status;
  }

  render_loop.Run();
  FX_LOGS(INFO) << "Quit main Scenic loop.";

  // Dropping `lifecycle` causes the component to exit and dso_runner to shutdown fdf_dispatcher.

  return 0;
}
