// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/controller.h"

#include <lib/async/cpp/task.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/inplace_vector.h>
#include <lib/trace/event.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zx/channel.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <algorithm>
#include <cinttypes>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <optional>
#include <span>
#include <utility>
#include <vector>

#include <fbl/alloc_checker.h>
#include <fbl/array.h>
#include <fbl/ref_ptr.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/coordinator/added-display-info.h"
#include "src/graphics/display/drivers/coordinator/client-id.h"
#include "src/graphics/display/drivers/coordinator/client-set.h"
#include "src/graphics/display/drivers/coordinator/client.h"
#include "src/graphics/display/drivers/coordinator/display-config.h"
#include "src/graphics/display/drivers/coordinator/image.h"
#include "src/graphics/display/drivers/coordinator/layer.h"
#include "src/graphics/display/drivers/coordinator/post-display-task.h"
#include "src/graphics/display/drivers/coordinator/vsync-monitor.h"
#include "src/graphics/display/lib/api-types/cpp/client-priority.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace fidl_display = fuchsia_hardware_display;

namespace display_coordinator {

void Controller::AddDisplay(std::unique_ptr<AddedDisplayInfo> added_display_info) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  zx::result<std::unique_ptr<DisplayInfo>> display_info_result =
      DisplayInfo::Create(std::move(*added_display_info));
  if (display_info_result.is_error()) {
    // DisplayInfo::Create() has already logged the error.
    return;
  }
  std::unique_ptr<DisplayInfo> display_info = std::move(display_info_result).value();

  display::DisplayId display_id = display_info->id();
  const std::array<display::DisplayId, 1> added_id_candidates = {display_id};
  std::span<const display::DisplayId> added_ids(added_id_candidates);

  // TODO(https://fxbug.dev/339311596): Do not trigger the client's
  // `OnDisplaysChanged` if an added display is ignored.
  //
  // Dropping some add events can result in spurious removes, but
  // those are filtered out in the clients.
  if (!display_info->preferred_modes.is_empty()) {
    display_info->InitializeInspect(&root_);
  } else {
    fdf::warn("Ignoring display with no usable display preferred modes");
    added_ids = {};
  }

  auto display_it = displays_.find(display_id);
  if (display_it != displays_.end()) {
    fdf::warn("Display {} is already created; add display request ignored", display_id.value());
    return;
  }
  displays_.insert(std::move(display_info));

  // TODO(https://fxbug.dev/317914671): Pass parsed display metadata to driver.

  clients_.DispatchOnDisplaysChanged(added_ids, /*removed_display_ids=*/{});
}

void Controller::RemoveDisplay(display::DisplayId removed_display_id) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  std::unique_ptr<DisplayInfo> removed_display = displays_.erase(removed_display_id);
  if (!removed_display) {
    fdf::warn("Display removal references unknown display ID: {}", removed_display_id.value());
    return;
  }

  // Release references to all images on the display.
  while (removed_display->images.pop_front()) {
  }

  const std::array<display::DisplayId, 1> removed_display_ids = {
      removed_display_id,
  };
  clients_.DispatchOnDisplaysChanged(/*added_display_ids=*/{}, removed_display_ids);
}

void Controller::OnDisplayAdded(std::unique_ptr<AddedDisplayInfo> added_display_info) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  // TODO(https://fxbug.dev/438325925): Remove the PostTask after this call
  // is guaranteed not to block an engine driver thread.
  zx::result<> post_task_result = display::PostTask<kDisplayTaskTargetSize>(
      *driver_dispatcher()->async_dispatcher(),
      [this, added_display_info = std::move(added_display_info)]() mutable {
        AddDisplay(std::move(added_display_info));
      });
  if (post_task_result.is_error()) {
    fdf::error("Failed to dispatch AddDisplay task: {}", post_task_result);
  }
}

void Controller::OnDisplayRemoved(display::DisplayId removed_display_id) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  // TODO(https://fxbug.dev/438325925): Remove the PostTask after this call
  // is guaranteed not to block an engine driver thread.
  zx::result<> post_task_result = display::PostTask<kDisplayTaskTargetSize>(
      *driver_dispatcher()->async_dispatcher(),
      [this, removed_display_id]() { RemoveDisplay(removed_display_id); });
  if (post_task_result.is_error()) {
    fdf::error("Failed to dispatch RemoveDisplay task: {}", post_task_result);
  }
}

void Controller::OnCaptureComplete() {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  ZX_DEBUG_ASSERT_MSG(engine_info_.has_value(),
                      "OnCaptureComplete() called before engine connection completed");

  if (!engine_info_->is_capture_supported()) {
    fdf::error("OnCaptureComplete() called by a display engine without display capture support");
    return;
  }

  // TODO(https://fxbug.dev/438325925): Remove the PostTask after this call
  // is guaranteed not to block an engine driver thread.
  zx::result<> post_task_result =
      display::PostTask<kDisplayTaskTargetSize>(*driver_dispatcher()->async_dispatcher(), [this]() {
        // Free an image that was previously used by the hardware.
        if (pending_release_capture_image_id_ != display::kInvalidDriverCaptureImageId) {
          ReleaseCaptureImage(pending_release_capture_image_id_);
          pending_release_capture_image_id_ = display::kInvalidDriverCaptureImageId;
        }

        clients_.DispatchOnCaptureComplete();
      });
  if (post_task_result.is_error()) {
    fdf::error("Failed to dispatch capture complete task: {}", post_task_result);
  }
}

void Controller::OnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                                display::DriverConfigStamp driver_config_stamp) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  // TODO(https://fxbug.dev/438325925): Remove the PostTask after this call
  // is guaranteed not to block an engine driver thread.
  zx::result<> post_task_result = display::PostTask<kDisplayTaskTargetSize>(
      *driver_dispatcher()->async_dispatcher(),
      [this, display_id, timestamp, driver_config_stamp]() {
        ProcessDisplayVsync(display_id, timestamp, driver_config_stamp);
      });
  if (post_task_result.is_error()) {
    fdf::error("Failed to dispatch ProcessVsync task: {}", post_task_result);
  }
}

void Controller::ProcessDisplayVsync(display::DisplayId display_id,
                                     zx::time_monotonic timestamp_mono,
                                     display::DriverConfigStamp driver_config_stamp) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  // TODO(b/475953032): Provide actual timestamp matching `timestamp` above.
  zx::time_boot timestamp_approximate_boot = zx::clock::get_boot();

  // TODO(https://fxbug.dev/402445178): This trace event is load bearing for fps trace processor.
  // Remove it after changing the dependency.
  TRACE_INSTANT("gfx", "VSYNC", TRACE_SCOPE_THREAD, "display_id", display_id.value());
  // Emit a counter called "VSYNC" for visualization in the Trace Viewer. `vsync_edge_flag`
  // switching between 0 and 1 counts represents one vsync period.
  static bool vsync_edge_flag = false;
  TRACE_COUNTER("gfx", "VSYNC", display_id.value(), "",
                TA_UINT32(vsync_edge_flag = !vsync_edge_flag));
  TRACE_DURATION("gfx", "Display::Controller::OnDisplayVsync", "display_id", display_id.value());

  auto displays_it = displays_.find(display_id);
  const std::optional<zx::duration> expected_vsync_interval =
      displays_it.IsValid() ? displays_it->GetCommittedModeVsyncInterval() : std::nullopt;

  vsync_monitor_.OnVsync(display_id, timestamp_mono, timestamp_approximate_boot,
                         driver_config_stamp, expected_vsync_interval);

  if (!displays_it.IsValid()) {
    fdf::error("Dropping VSync for unknown display ID: {}", display_id.value());
    return;
  }
  DisplayInfo& display_info = *displays_it;

  // See ::SubmitConfig for more explanation of how vsync image tracking works.
  //
  // If there's a pending layer change, don't process any present/retire actions
  // until the change is complete.
  if (display_info.pending_layer_change) {
    bool done = driver_config_stamp >= display_info.pending_layer_change_driver_config_stamp;
    if (done) {
      display_info.pending_layer_change = false;
      display_info.pending_layer_change_driver_config_stamp = display::kInvalidDriverConfigStamp;
      display_info.switching_client = false;
    }
  }

  // Obsolete stamps will be removed in `Client::OnDisplayVsync()`.
  std::optional<display::ClientPriority> config_stamp_source =
      clients_.FindConfigStampSource(driver_config_stamp);

  if (!display_info.pending_layer_change) {
    // Each image in the `info->images` set can fall into one of the following
    // cases:
    // - being displayed (its `latest_controller_config_stamp` matches the
    //   incoming `controller_config_stamp` from display driver);
    // - older than the current displayed image (its
    //   `latest_controller_config_stamp` is less than the incoming
    //   `controller_config_stamp`) and should be removed from DisplayInfo::images;
    // - newer than the current displayed image (its
    //   `latest_controller_config_stamp` is greater than the incoming
    //   `controller_config_stamp`) and yet to be presented.
    for (auto it = display_info.images.begin(); it != display_info.images.end();) {
      bool image_should_be_removed = it->latest_driver_config_stamp() < driver_config_stamp;

      // Retire any images which are older than whatever is currently in their
      // layer.
      if (image_should_be_removed) {
        fbl::RefPtr<Image> removed_image = display_info.images.erase(it++);
        // Older images may not be presented. Ending their flows here
        // ensures the correctness of traces.
        //
        // NOTE: If changing this flow name or ID, please also do so in the
        // corresponding FLOW_BEGIN.
        TRACE_FLOW_END("gfx", "present_image", removed_image->id().value());
      } else {
        it++;
      }
    }
  }

  // Evict retired configurations from the queue.
  auto& config_image_queue = display_info.config_image_queue;
  while (!config_image_queue.empty() &&
         config_image_queue.front().config_stamp < driver_config_stamp) {
    config_image_queue.pop();
  }

  // Since the stamps sent from Controller to drivers are in chronological
  // order, the Vsync signals Controller receives should also be in
  // chronological order as well.
  //
  // Applying empty configs won't create entries in |config_image_queue|.
  // Otherwise, we'll get the list of images used at SubmitConfig() with
  // the given |config_stamp|.
  if (!config_image_queue.empty() &&
      config_image_queue.front().config_stamp == driver_config_stamp) {
    for (const auto& image : config_image_queue.front().images) {
      // End of the flow for the image going to be presented.
      //
      // NOTE: If changing this flow name or ID, please also do so in the
      // corresponding FLOW_BEGIN.
      TRACE_FLOW_END("gfx", "present_image", image.image_id.value());
    }
  }

  if (!config_stamp_source.has_value()) {
    // The config was committed by a client that is no longer connected.
    fdf::debug("VSync event dropped; the config owner disconnected");
    return;
  }
  clients_.DispatchOnDisplayVsync(display_id, timestamp_mono, driver_config_stamp,
                                  config_stamp_source.value());
}

void Controller::SubmitConfig(DisplayConfig& display_config,
                              display::ConfigStamp client_config_stamp, ClientId client_id) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  zx::time_monotonic timestamp_mono = zx::clock::get_monotonic();
  zx::time_boot timestamp_boot = zx::clock::get_boot();

  last_valid_apply_config_timestamp_ns_property_.Set(timestamp_mono.get());
  last_valid_apply_config_timestamp_mono_ns_property_.Set(timestamp_mono.get());
  last_valid_apply_config_timestamp_boot_ns_property_.Set(timestamp_boot.get());

  const zx::duration interval_mono = timestamp_mono - last_valid_apply_config_timestamp_mono_;
  last_valid_apply_config_interval_ns_property_.Set(interval_mono.get());
  last_valid_apply_config_duration_mono_ns_property_.Set(interval_mono.get());

  const zx::duration interval_boot = timestamp_boot - last_valid_apply_config_timestamp_boot_;
  last_valid_apply_config_duration_boot_ns_property_.Set(interval_boot.get());

  last_valid_apply_config_timestamp_mono_ = timestamp_mono;
  last_valid_apply_config_timestamp_boot_ = timestamp_boot;

  last_valid_apply_config_config_stamp_property_.Set(client_config_stamp.value());

  // The submitted configuration's stamp.
  display::DriverConfigStamp driver_config_stamp = {};
  cpp26::inplace_vector<display::DriverLayer, display::EngineInfo::kMaxAllowedMaxLayerCount>
      driver_layers;

  {
    bool switching_client = client_id != applied_client_id_;

    ++last_issued_driver_config_stamp_;
    driver_config_stamp = last_issued_driver_config_stamp_;

    auto displays_it = displays_.find(display_config.id());
    if (!displays_it.IsValid()) {
      fdf::warn("SubmitConfig(): Cannot find display with id {}", display_config.id());
      return;
    }
    DisplayInfo& display_info = *displays_it;

    display_info.config_image_queue.push({.config_stamp = driver_config_stamp, .images = {}});

    display_info.switching_client = switching_client;
    display_info.pending_layer_change = display_config.commit_layer_change();
    if (display_info.pending_layer_change) {
      display_info.pending_layer_change_driver_config_stamp = driver_config_stamp;
    }
    display_info.layer_count = display_config.committed_config().layer_count;
    display_info.committed_mode_id = display_config.committed_config().mode_id;

    if (display_info.layer_count == 0) {
      // TODO(https://fxbug.dev/336394440): Make this a fatal error.
      fdf::warn("SubmitConfig(): config doesn't have any valid layer; skipped");
      return;
    }

    ZX_DEBUG_ASSERT(driver_layers.empty());
    for (const LayerNode& committed_layer_node : display_config.get_committed_layers()) {
      const Layer* committed_layer = committed_layer_node.layer;
      driver_layers.push_back(committed_layer->committed_driver_layer_config());
      fbl::RefPtr<Image> applied_image = committed_layer->committed_image();

      if (committed_layer->is_skipped() || applied_image == nullptr) {
        continue;
      }

      // Set the image controller config stamp so vsync knows what config the
      // image was used at.
      applied_image->set_latest_driver_config_stamp(driver_config_stamp);

      // NOTE: If changing this flow name or ID, please also do so in the
      // corresponding FLOW_END.
      TRACE_FLOW_BEGIN("gfx", "present_image", applied_image->id().value());

      // It's possible that the image's layer was moved between displays. The logic around
      // pending_layer_change guarantees that the old display will be done with the image
      // before the new display is, so deleting it from the old list is fine.
      //
      // Even if we're on the same display, the entry needs to be moved to the end of the
      // list to ensure that the last config->current.layer_count elements in the queue
      // are the current images.
      //
      // TODO(https://fxbug.dev/317914671): investigate whether storing Images in doubly-linked
      //                                    lists continues to be desirable.
      if (applied_image->InDoublyLinkedList()) {
        applied_image->RemoveFromDoublyLinkedList();
      }
      display_info.images.push_back(applied_image);
      display_info.config_image_queue.back().images.push_back(
          {.image_id = applied_image->id(), .client_id = applied_image->client_id()});
    }

    applied_client_id_ = client_id;

    Client* client_owning_displays = clients_.GetClientOwningDisplays();
    if (client_owning_displays != nullptr) {
      if (switching_client) {
        client_owning_displays->SubmitSpecialConfigs();
      }

      client_owning_displays->UpdateConfigStampMapping({
          .driver_stamp = driver_config_stamp,
          .client_stamp = client_config_stamp,
      });
    }
  }

  DriverDisplayConfig driver_display_config = display_config.committed_config();
  // Populated by Client::SubmitConfig().
  ZX_DEBUG_ASSERT(static_cast<size_t>(driver_display_config.layer_count) == driver_layers.size());

  engine_driver_client_->SubmitConfiguration(driver_display_config, driver_layers,
                                             driver_config_stamp);
}

void Controller::ImageWillBeDestroyed(display::DriverImageId driver_image_id) {
  engine_driver_client_->ReleaseImage(driver_image_id);
}

void Controller::ReleaseCaptureImage(display::DriverCaptureImageId driver_capture_image_id) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());
  ZX_DEBUG_ASSERT_MSG(engine_info_.has_value(),
                      "CaptureImage created before engine connection completed");
  ZX_DEBUG_ASSERT_MSG(engine_info_->is_capture_supported(),
                      "CaptureImage created by engine without capture support");

  if (driver_capture_image_id == display::kInvalidDriverCaptureImageId) {
    return;
  }

  const zx::result<> result = engine_driver_client_->ReleaseCapture(driver_capture_image_id);
  if (result.is_error() && result.error_value() == ZX_ERR_SHOULD_WAIT) {
    ZX_DEBUG_ASSERT_MSG(pending_release_capture_image_id_ == display::kInvalidDriverCaptureImageId,
                        "multiple pending releases for capture images");
    // Delay the image release until the hardware is done.
    pending_release_capture_image_id_ = driver_capture_image_id;
  }
}

void Controller::OnClientDisconnected(Client* client) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());
  ZX_DEBUG_ASSERT(client != nullptr);

  if (unbinding_) {
    return;
  }

  // `ClientSet::OnClientDisconnected()` logs the client disconnection.
  clients_.OnClientDisconnected(client);
}

zx::result<std::span<const display::ModeAndId>> Controller::GetDisplayPreferredModes(
    display::DisplayId display_id) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  if (unbinding_) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  auto displays_it = displays_.find(display_id);
  if (!displays_it.IsValid()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const DisplayInfo& display_info = *displays_it;
  return zx::ok(std::span(display_info.preferred_modes));
}

zx::result<fbl::Vector<display::PixelFormat>> Controller::GetSupportedPixelFormats(
    display::DisplayId display_id) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  auto displays_it = displays_.find(display_id);
  if (!displays_it.IsValid()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const DisplayInfo& display_info = *displays_it;

  fbl::AllocChecker alloc_checker;
  fbl::Vector<display::PixelFormat> pixel_formats;
  pixel_formats.reserve(display_info.pixel_formats.size(), &alloc_checker);
  if (!alloc_checker.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  std::ranges::copy(display_info.pixel_formats, std::back_inserter(pixel_formats));
  ZX_DEBUG_ASSERT(pixel_formats.size() == display_info.pixel_formats.size());

  return zx::ok(std::move(pixel_formats));
}

zx::result<> Controller::CreateClient(
    display::ClientPriority client_priority,
    fidl::ServerEnd<fidl_display::Coordinator> coordinator_server_end,
    fidl::ClientEnd<fuchsia_hardware_display::CoordinatorListener>
        coordinator_listener_client_end) {
  ZX_DEBUG_ASSERT(client_priority != display::ClientPriority::kInvalid);
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  if (unbinding_) {
    fdf::debug("Client connected during unbind");
    return zx::error(ZX_ERR_UNAVAILABLE);
  }

  cpp26::inplace_vector<display::DisplayId,
                        display::EngineInfo::kMaxAllowedMaxConnectedDisplayCount>
      current_display_ids;
  for (const DisplayInfo& display : displays_) {
    current_display_ids.push_back(display.id());
  }
  ZX_DEBUG_ASSERT(current_display_ids.size() == displays_.size());

  zx::result<> connect_result = clients_.ConnectClient(this, client_priority, current_display_ids,
                                                       std::move(coordinator_server_end),
                                                       std::move(coordinator_listener_client_end));
  if (connect_result.is_error()) {
    fdf::debug("Failed to connect client: {}", connect_result.status_string());
    return connect_result.take_error();
  }
  return zx::ok();
}

display::DriverBufferCollectionId Controller::GetNextDriverBufferCollectionId() {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  return next_driver_buffer_collection_id_++;
}

void Controller::OpenCoordinator(OpenCoordinatorRequestView request,
                                 OpenCoordinatorCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  if (!request->has_coordinator()) {
    fdf::error("OpenCoordinator() missing required table entry: coordinator");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  if (!request->has_coordinator_listener()) {
    fdf::error("OpenCoordinator() missing required table entry: coordinator_listener");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (!request->has_priority()) {
    fdf::error("OpenCoordinator() missing required table entry: priority");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  zx::result<> create_client_result =
      CreateClient(display::ClientPriority(request->priority().value),
                   std::move(request->coordinator()), std::move(request->coordinator_listener()));
  if (create_client_result.is_error()) {
    completer.ReplyError(create_client_result.error_value());
    return;
  }

  completer.ReplySuccess();
}

// static
zx::result<std::unique_ptr<Controller>> Controller::Create(
    std::unique_ptr<EngineDriverClient> engine_driver_client,
    fdf::UnownedSynchronizedDispatcher driver_dispatcher, inspect::Inspector inspector,
    uint32_t fallback_horizontal_size_mm, uint32_t fallback_vertical_size_mm) {
  fbl::AllocChecker alloc_checker;

  auto controller = fbl::make_unique_checked<Controller>(
      &alloc_checker, std::move(engine_driver_client), std::move(driver_dispatcher),
      std::move(inspector), fallback_horizontal_size_mm, fallback_vertical_size_mm);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for Controller");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> initialize_result = controller->Initialize();
  if (initialize_result.is_error()) {
    fdf::error("Failed to initialize the Controller device: {}", initialize_result);
    return initialize_result.take_error();
  }

  return zx::ok(std::move(controller));
}

zx::result<> Controller::Initialize() {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());

  zx::result<> vsync_monitor_init_result = vsync_monitor_.Initialize();
  if (vsync_monitor_init_result.is_error()) {
    // VsyncMonitor::Init() logged the error.
    return vsync_monitor_init_result.take_error();
  }

  auto [fidl_listener_client, fidl_listener_server] =
      fdf::Endpoints<fuchsia_hardware_display_engine::EngineListener>::Create();
  engine_listener_fidl_adapter_.CreateHandler()(std::move(fidl_listener_server));

  engine_info_ =
      engine_driver_client_->CompleteCoordinatorConnection(std::move(fidl_listener_client));
  fdf::info("Engine capabilities - max layers: {}, max displays: {}, display capture: {}",
            engine_info_->max_layer_count(), engine_info_->max_connected_display_count(),
            engine_info_->is_capture_supported() ? "yes" : "no");

  return zx::ok();
}

void Controller::PrepareStop() {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());
  fdf::info("Controller::PrepareStop started");

  {
    unbinding_ = true;
    clients_.Clear();

    vsync_monitor_.Deinitialize();

    // Once this call completes, the engine driver will no longer send events.
    // This means it's safe to stop keeping track of imported resources.
    engine_driver_client_->UnsetListener();

    // Dispose of all images without calling ReleaseImage().
    for (DisplayInfo& display : displays_) {
      while (fbl::RefPtr<Image> displayed_image = display.images.pop_front()) {
        displayed_image->MarkDisposed();
      }
    }
  }

  fdf::info("Controller::PrepareStop finished");
}

Controller::Controller(std::unique_ptr<EngineDriverClient> engine_driver_client,
                       fdf::UnownedSynchronizedDispatcher driver_dispatcher,
                       inspect::Inspector inspector, uint32_t fallback_horizontal_size_mm,
                       uint32_t fallback_vertical_size_mm)
    : inspector_(std::move(inspector)),
      root_(inspector_.GetRoot().CreateChild("display")),
      driver_dispatcher_(std::move(driver_dispatcher)),
      engine_listener_fidl_adapter_(this, driver_dispatcher_->borrow()),
      vsync_monitor_(root_.CreateChild("vsync_monitor"), driver_dispatcher_->async_dispatcher()),
      engine_driver_client_(std::move(engine_driver_client)),
      clients_(root_.CreateChild("clients")),
      fallback_horizontal_size_mm_(fallback_horizontal_size_mm),
      fallback_vertical_size_mm_(fallback_vertical_size_mm) {
  ZX_DEBUG_ASSERT(IsRunningOnDriverDispatcher());
  ZX_DEBUG_ASSERT(engine_driver_client_ != nullptr);

  // TODO(b/475953032): Remove this metric once the "mono_ns" flavor is used.
  // Here and below, too.
  last_valid_apply_config_timestamp_ns_property_ =
      root_.CreateUint("last_valid_apply_config_timestamp_ns", 0);
  last_valid_apply_config_timestamp_mono_ns_property_ =
      root_.CreateUint("last_valid_apply_config_timestamp_mono_ns", 0);
  last_valid_apply_config_timestamp_boot_ns_property_ =
      root_.CreateUint("last_valid_apply_config_timestamp_boot_ns", 0);

  last_valid_apply_config_interval_ns_property_ =
      root_.CreateUint("last_valid_apply_config_interval_ns", 0);
  last_valid_apply_config_duration_mono_ns_property_ =
      root_.CreateUint("last_valid_apply_config_duration_mono_ns", 0);
  last_valid_apply_config_duration_boot_ns_property_ =
      root_.CreateUint("last_valid_apply_config_duration_boot_ns", 0);

  last_valid_apply_config_config_stamp_property_ =
      root_.CreateUint("last_valid_apply_config_stamp", display::kInvalidConfigStamp.value());
}

Controller::~Controller() { fdf::info("Controller::~Controller"); }

}  // namespace display_coordinator
