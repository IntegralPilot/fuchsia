// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/directory.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/scheduler/role.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/channel.h>
#include <lib/zx/thread.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/status.h>

#include <memory>

#include "src/graphics/display/lib/coordinator-getter/client.h"
#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/ui/scenic/bin/app.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/scenic_structured_config.h"

int main(int argc, const char** argv) {
  async::Loop render_loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  if (!fxl::SetLogSettingsFromCommandLine(command_line, {"scenic"})) {
    return 1;
  }

  trace::TraceProviderWithFdio trace_provider(render_loop.dispatcher());
  // This call creates ComponentContext, but does not start serving immediately. Outgoing directory
  // is served by App, after App::InitializeServices() is completed.
  std::unique_ptr<sys::ComponentContext> app_context = sys::ComponentContext::Create();

  // Set up an inspect::Node to inject into the App.
  inspect::ComponentInspector inspector(render_loop.dispatcher(), {});

  // `watcher` and `display_coordinator_promise` will be run on the render loop's dispatcher.
  fpromise::bridge<fidl::ClientEnd<fuchsia_hardware_display::Provider>>
      display_service_provider_bridge;
  component::ServiceMemberWatcher<fuchsia_hardware_display::Service::Provider> watcher;
  zx::result<> watch_result =
      watcher.Begin(render_loop.dispatcher(),
                    [completer = std::move(display_service_provider_bridge.completer)](
                        fidl::ClientEnd<fuchsia_hardware_display::Provider> client_end) mutable {
                      completer.complete_ok(std::move(client_end));
                    });
  if (watch_result.is_error()) {
    FX_LOGS(FATAL) << "Failed to watch fuchsia.hardware.display.Provider: "
                   << zx_status_get_string(watch_result.error_value());
  }
  fpromise::promise<::display::CoordinatorClientChannels, zx_status_t> display_coordinator_promise =
      display_service_provider_bridge.consumer.promise()
          .or_else([]() { return fpromise::error<zx_status_t>(ZX_ERR_CANCELED); })
          .and_then([](fidl::ClientEnd<fuchsia_hardware_display::Provider>& provider) {
            return display::GetCoordinator(std::move(provider));
          });

  zx::channel pkg_dir, pkg_server;
  zx::channel::create(0, &pkg_dir, &pkg_server);
  const zx_status_t pkg_status = fdio_open3(
      "/pkg", static_cast<uint64_t>(fuchsia_io::kPermReadable | fuchsia_io::kPermExecutable),
      pkg_server.release());
  FX_CHECK(pkg_status == ZX_OK) << "Failed to open /pkg: " << zx_status_get_string(pkg_status);
  const zx_handle_t directory_request_handle = zx_take_startup_handle(PA_DIRECTORY_REQUEST);
  zx::channel out_dir{directory_request_handle};
  const zx_handle_t config_handle = zx_take_startup_handle(PA_VMO_COMPONENT_CONFIG);
  zx::vmo config_vmo{config_handle};
  auto config = scenic_structured_config::Config::CreateFromVmo(std::move(config_vmo));
  if (config.prefetch()) {
    scenic_impl::PrefetchBinary(pkg_dir.get(), "bin/scenic");
  }

  // Only use a dedicated input loop/dispatcher/thread if configured to do so.  Otherwise, use the
  // same dispatcher for rendering and input.
  std::unique_ptr<async::Loop> input_loop;
  async_dispatcher_t* input_dispatcher = render_loop.dispatcher();
  if (config.use_separate_input_thread()) {
    input_loop = std::make_unique<async::Loop>(&kAsyncLoopConfigNoAttachToCurrentThread);
    zx_status_t input_thread_status = input_loop->StartThread("scenic.input");
    FX_CHECK(input_thread_status == ZX_OK)
        << "Failed to start input thread: " << zx_status_get_string(input_thread_status);

    // Set the role for the input thread.
    async::PostTask(input_loop->dispatcher(), [] {
      const zx_status_t role_status =
          fuchsia_scheduler::SetRoleForThisThread("fuchsia.scenic.input");
      if (role_status == ZX_OK) {
        FX_LOGS(INFO) << "Applied profile to input thread";
      } else {
        FX_LOGS(WARNING) << "Failed to apply profile to input thread: " << role_status;
      }
    });

    input_dispatcher = input_loop->dispatcher();
  }
  utils::ScopedThreadDispatcherSetter setter(render_loop.dispatcher(), input_dispatcher);

  // Instantiate Scenic app.
  scenic_impl::App app(render_loop.dispatcher(), input_dispatcher, std::move(app_context),
                       fidl::ClientEnd<fuchsia_io::Directory>(std::move(pkg_dir)),
                       fidl::ServerEnd<fuchsia_io::Directory>(std::move(out_dir)),
                       std::move(config), inspector.root(), std::move(display_coordinator_promise),
                       [&render_loop, &input_loop] {
                         // `Quit()` signals a graceful shutdown, allowing the loops to complete any
                         // active task before terminating.  We must use `Quit()` instead of
                         // `Shutdown()` here because this lambda runs on the render loop's own
                         // thread, and calling `Shutdown()` (which blocks to join the loop's
                         // threads) would result in a guaranteed self-join deadlock.
                         render_loop.Quit();
                         if (input_loop) {
                           input_loop->Quit();
                         }
                       });

  // Apply the scheduler role defined for Scenic.
  const zx_status_t thread_status = fuchsia_scheduler::SetRoleForThisThread("fuchsia.scenic.main");
  if (thread_status != ZX_OK) {
    FX_LOGS(WARNING) << "Failed to apply profile to main thread: " << thread_status;
  }

  render_loop.Run();
  // Reaching this point guarantees that `Quit()` has been called on both loops;
  // otherwise, `render_loop.Run()` would not have returned.
  if (input_loop) {
    // `input_loop->Quit()` only signals the thread to stop asynchronously.  We must call
    // `JoinThreads()` to guarantee the background thread has completely exited before `app`
    // goes out of scope, preventing a shutdown use-after-free in input subsystems.
    input_loop->JoinThreads();
  }
  FX_LOGS(INFO) << "Quit main Scenic loop.";

  return 0;
}
