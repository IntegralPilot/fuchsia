// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fuchsia/camera3/cpp/fidl.h>
#include <fuchsia/sysmem/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>

#include <set>

#include "src/camera/bin/camera-gym/buffer_collage_flatland.h"
#include "src/camera/bin/camera-gym/controller_receiver.h"
#include "src/camera/bin/camera-gym/frame_capture.h"
#include "src/camera/bin/camera-gym/lifecycle_impl.h"
#include "src/camera/bin/camera-gym/stream_cycler.h"
#include "src/lib/fxl/command_line.h"

using Command = fuchsia::camera::gym::Command;

constexpr zx::duration kDefaultAutoCycleInterval = zx::msec(CONFIGURATION_CYCLE_INTERVAL_MS);

namespace {
int Setup(std::optional<zx::duration> auto_cycle_interval, async::Loop* buffer_collage_loop,
          async::Loop* cycler_loop, std::unique_ptr<sys::ComponentContext> context) {
  fuchsia::sysmem2::AllocatorHandle buffer_collage_allocator;
  zx_status_t status = context->svc()->Connect(buffer_collage_allocator.NewRequest());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to request Allocator service.";
    return EXIT_FAILURE;
  }

  auto flatland_connect = component::Connect<fuchsia_ui_composition::Flatland>();
  if (flatland_connect.is_error()) {
    FX_PLOGS(ERROR, flatland_connect.error_value()) << "Failed to request Flatland service.";
    return EXIT_FAILURE;
  }

  auto flatland_allocator_connect = component::Connect<fuchsia_ui_composition::Allocator>();
  if (flatland_allocator_connect.is_error()) {
    FX_PLOGS(ERROR, flatland_allocator_connect.error_value())
        << "Failed to request flatland allocator service.";
    return EXIT_FAILURE;
  }

  auto graphical_presenter_connect = component::Connect<fuchsia_element::GraphicalPresenter>();
  if (graphical_presenter_connect.is_error()) {
    FX_PLOGS(ERROR, graphical_presenter_connect.error_value())
        << "Failed to request GraphicalPresenter service.";
    return EXIT_FAILURE;
  }

  // Create the collage.
  auto collage_result = camera_flatland::BufferCollageFlatland::Create(
      std::move(flatland_connect.value()), std::move(flatland_allocator_connect.value()),
      std::move(graphical_presenter_connect.value()), std::move(buffer_collage_allocator),
      [&buffer_collage_loop] { buffer_collage_loop->Quit(); });
  if (collage_result.is_error()) {
    FX_PLOGS(ERROR, collage_result.error()) << "Failed to create BufferCollageFlatland.";
    return EXIT_FAILURE;
  }
  auto collage = collage_result.take_value();

  // Connect to required services for the cycler.
  fuchsia::camera3::DeviceWatcherHandle watcher;
  status = context->svc()->Connect(watcher.NewRequest());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to request DeviceWatcher service.";
    return EXIT_FAILURE;
  }

  fuchsia::sysmem2::AllocatorHandle stream_cycler_allocator;
  status = context->svc()->Connect(stream_cycler_allocator.NewRequest());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to request Allocator service.";
    return EXIT_FAILURE;
  }

  // Create the cycler and attach it to the collage.
  auto cycler_result = camera::StreamCycler::Create(
      std::move(watcher), std::move(stream_cycler_allocator), cycler_loop->dispatcher(), false);
  if (cycler_result.is_error()) {
    FX_PLOGS(ERROR, cycler_result.error()) << "Failed to create StreamCycler.";
    return EXIT_FAILURE;
  }
  auto cycler = cycler_result.take_value();
  cycler->set_auto_cycle_interval(auto_cycle_interval);

  status = cycler_loop->StartThread("StreamCycler Thread");
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status);
    return EXIT_FAILURE;
  }

  std::set<uint32_t> collection_ids;

  camera::StreamCycler::AddCollectionHandler add_collection_handler =
      [&collage, &collection_ids](fuchsia::sysmem2::BufferCollectionTokenHandle token,
                                  fuchsia::images2::ImageFormat image_format,
                                  std::string description) -> uint32_t {
    auto result = fpromise::run_single_threaded(
        collage->AddCollection(std::move(token), std::move(image_format), std::move(description)));
    if (result.is_error()) {
      FX_LOGS(FATAL) << "Failed to add collection to collage.";
      return 0;
    }
    collection_ids.insert(result.value());
    return result.value();
  };

  camera::StreamCycler::RemoveCollectionHandler remove_collection_handler =
      [&collage, &collection_ids](uint32_t id) {
        collection_ids.erase(id);
        collage->RemoveCollection(id);
      };

  // TODO(https://fxbug.dev/42073155): Add logic to hide views if device is muted.
  camera::StreamCycler::MuteStateHandler mute_handler = [](bool muted) {};

  camera::StreamCycler::ShowBufferHandler show_buffer_handler =
      [&collage](uint32_t collection_id, uint32_t buffer_index, zx::eventpair* release_fence,
                 std::optional<fuchsia::math::RectF> subregion) {
        collage->PostShowBuffer(collection_id, buffer_index, std::move(release_fence),
                                std::move(subregion));
      };
  cycler->SetHandlers(std::move(add_collection_handler), std::move(remove_collection_handler),
                      std::move(show_buffer_handler), std::move(mute_handler));

  // Present the view.
  collage->PresentView();

  // Serve the Lifecycle protocol that cleanly quits the component.
  // This binds to a process handle, so it's not added to the outgoing directory.
  LifecycleImpl lifecycle([&buffer_collage_loop] { buffer_collage_loop->Quit(); });

  buffer_collage_loop->Run();
  cycler_loop->Quit();
  cycler_loop->JoinThreads();
  return EXIT_SUCCESS;
}

}  // namespace

int main(int argc, char* argv[]) {
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);

  // Default must match existing behavior as started from UI.
  std::optional<zx::duration> auto_cycle_interval = kDefaultAutoCycleInterval;
  std::string value_string;
  if (command_line.GetOptionValue("auto-cycle-interval-ms", &value_string)) {
    auto auto_cycle_interval_ms_value = std::stoul(value_string, nullptr, 0);

    // Value of 0 means "infinite". (Meaning no config cycling at all.)
    if (auto_cycle_interval_ms_value == 0) {
      auto_cycle_interval.reset();
    } else {
      auto_cycle_interval = zx::msec(auto_cycle_interval_ms_value);
    }
  }
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithMinLogSeverity(CAMERA_MIN_LOG_LEVEL).WithTags({"camera-gym"}).BuildAndInitialize();

  async::Loop buffer_collage_loop(&kAsyncLoopConfigAttachToCurrentThread);
  async::Loop cycler_loop(&kAsyncLoopConfigNoAttachToCurrentThread);

  trace::TraceProviderWithFdio trace_provider(buffer_collage_loop.dispatcher());
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();

  return Setup(auto_cycle_interval, &buffer_collage_loop, &cycler_loop, std::move(context));
}
