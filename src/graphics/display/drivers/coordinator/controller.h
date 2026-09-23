// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CONTROLLER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CONTROLLER_H_

#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/channel.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <list>
#include <memory>
#include <optional>
#include <span>

#include <fbl/vector.h>

#include "src/graphics/display/drivers/coordinator/added-display-info.h"
#include "src/graphics/display/drivers/coordinator/capture-image.h"
#include "src/graphics/display/drivers/coordinator/client-id.h"
#include "src/graphics/display/drivers/coordinator/client-set.h"
#include "src/graphics/display/drivers/coordinator/display-info.h"
#include "src/graphics/display/drivers/coordinator/engine-driver-client.h"
#include "src/graphics/display/drivers/coordinator/engine-listener-fidl-adapter.h"
#include "src/graphics/display/drivers/coordinator/engine-listener.h"
#include "src/graphics/display/drivers/coordinator/id-map.h"
#include "src/graphics/display/drivers/coordinator/image-lifecycle-listener.h"
#include "src/graphics/display/drivers/coordinator/vsync-monitor.h"
#include "src/graphics/display/lib/api-types/cpp/client-priority.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace display_coordinator {

class Client;
class DisplayConfig;

// Multiplexes between display controller clients and display engine drivers.
//
// Instances are not thread-safe, and must only be used on the driver
// dispatcher.
class Controller : public fidl::WireServer<fuchsia_hardware_display::Provider>,
                   public EngineListener,
                   public ImageLifecycleListener {
 public:
  // Factory method for production use.
  // Creates and initializes a Controller instance.
  //
  // Asynchronous work that manages the state of the display clients and
  // coordinates the display state between clients and engine drivers runs on
  // `driver_dispatcher`.
  //
  // `engine_driver_client` must not be null.
  //
  // `driver_dispatcher` must be running until `PrepareStop()` is called.
  // `driver_dispatcher` must be shut down when `Stop()` is called.
  static zx::result<std::unique_ptr<Controller>> Create(
      std::unique_ptr<EngineDriverClient> engine_driver_client,
      fdf::UnownedSynchronizedDispatcher driver_dispatcher, inspect::Inspector inspector,
      uint32_t fallback_horizontal_size_mm, uint32_t fallback_vertical_size_mm);

  // Creates a new coordinator Controller instance. It creates a new Inspector
  // which will be solely owned by the Controller instance.
  //
  // `engine_driver_client` must not be null.
  explicit Controller(std::unique_ptr<EngineDriverClient> engine_driver_client,
                      fdf::UnownedSynchronizedDispatcher driver_dispatcher,
                      inspect::Inspector inspector, uint32_t fallback_horizontal_size_mm,
                      uint32_t fallback_vertical_size_mm);

  uint32_t fallback_horizontal_size_mm() const { return fallback_horizontal_size_mm_; }
  uint32_t fallback_vertical_size_mm() const { return fallback_vertical_size_mm_; }

  Controller(const Controller&) = delete;
  Controller& operator=(const Controller&) = delete;

  ~Controller() override;

  // References the `PrepareStop()` method in the DFv2 (fdf::DriverBase) driver
  // lifecycle.
  void PrepareStop();

  // `EngineListener`:
  // Must run on `driver_dispatcher_`.
  void OnDisplayAdded(std::unique_ptr<AddedDisplayInfo> added_display_info) override;
  void OnDisplayRemoved(display::DisplayId removed_display_id) override;
  void OnCaptureComplete() override;
  void OnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                      display::DriverConfigStamp driver_config_stamp) override;

  // `client` must not be null.
  //
  // `client` is removed from the client set and destroyed during this call.
  void OnClientDisconnected(Client* client);

  void SubmitConfig(DisplayConfig& display_config, display::ConfigStamp client_config_stamp,
                    ClientId client_id);

  // ImageLifecycleListener:
  void ImageWillBeDestroyed(display::DriverImageId driver_image_id) override;

  void ReleaseCaptureImage(display::DriverCaptureImageId driver_capture_image_id);

  // The display modes are guaranteed to be valid as long as the display with
  // `display_id` is valid.
  //
  // For a valid display, it's guaranteed that `GetDisplayPreferredModes()` is
  // non-empty.
  zx::result<std::span<const display::ModeAndId>> GetDisplayPreferredModes(
      display::DisplayId display_id);

  zx::result<fbl::Vector<display::PixelFormat>> GetSupportedPixelFormats(
      display::DisplayId display_id);

  // Calls `callback` with a const DisplayInfo& matching the given `display_id`.
  //
  // Returns true iff a DisplayInfo with `display_id` was found and `callback`
  // was called.
  //
  // The controller mutex is guaranteed to be held while `callback` is called.
  template <typename Callback>
  bool FindDisplayInfo(display::DisplayId display_id, Callback callback);

  EngineDriverClient* engine_driver_client() { return engine_driver_client_.get(); }

  // May only be called after the display engine driver is connected.
  bool supports_capture() { return engine_info_->is_capture_supported(); }

  // May only be called after the display engine driver is connected.
  const display::EngineInfo& engine_info() const { return *engine_info_; }

  fdf::UnownedSynchronizedDispatcher driver_dispatcher() const {
    return driver_dispatcher_->borrow();
  }
  bool IsRunningOnDriverDispatcher() {
    return fdf::Dispatcher::GetCurrent()->get() == driver_dispatcher_->get();
  }

  const inspect::Inspector& inspector() const { return inspector_; }

  // Typically called by OpenController/OpenVirtconController. However, this is made public
  // for use by testing services which provide a fake display controller.
  //
  // `client_priority` must be valid.
  zx::result<> CreateClient(
      display::ClientPriority client_priority,
      fidl::ServerEnd<fuchsia_hardware_display::Coordinator> coordinator_server_end,
      fidl::ClientEnd<fuchsia_hardware_display::CoordinatorListener>
          coordinator_listener_client_end);

  display::DriverBufferCollectionId GetNextDriverBufferCollectionId();

  // `fidl::WireServer<fuchsia_hardware_display::Provider>`:
  void OpenCoordinator(OpenCoordinatorRequestView request,
                       OpenCoordinatorCompleter::Sync& completer) override;

  const ClientSet& ClientsForTesting() const { return clients_; }

 private:
  // Initializes logic that is not suitable for the constructor.
  zx::result<> Initialize();

  void HandleClientOwnershipChanges();

  // Processes a display addition notification from an engine driver.
  //
  // Must be called on the driver dispatcher.
  void AddDisplay(std::unique_ptr<AddedDisplayInfo> added_display_info);

  // Processes a display removal notification from an engine driver.
  //
  // Must be called on the driver dispatcher.
  void RemoveDisplay(display::DisplayId removed_display_id);

  // Processes a VSync signal from an engine driver.
  //
  // Must be called on the driver dispatcher.
  void ProcessDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                           display::DriverConfigStamp driver_config_stamp);

  inspect::Inspector inspector_;
  // Currently located at bootstrap/driver_manager:root/display.
  inspect::Node root_;

  fdf::UnownedSynchronizedDispatcher driver_dispatcher_;

  EngineListenerFidlAdapter engine_listener_fidl_adapter_;

  VsyncMonitor vsync_monitor_;

  bool unbinding_ = false;

  // The engine driver client used for hardware operations.
  //
  // `engine_driver_client_` needs to outlive `displays_` and `clients_`.
  std::unique_ptr<EngineDriverClient> engine_driver_client_;

  DisplayInfo::Map displays_;
  ClientId applied_client_id_ = kInvalidClientId;
  display::DriverCaptureImageId pending_release_capture_image_id_ =
      display::kInvalidDriverCaptureImageId;

  // Populated after the engine is initialized.
  std::optional<display::EngineInfo> engine_info_;

  display::DriverBufferCollectionId next_driver_buffer_collection_id_ =
      display::DriverBufferCollectionId(1);

  ClientSet clients_;

  // TODO(https://fxbug.dev/489243434): Switch away from "applied" terminology.
  zx::time_monotonic last_valid_apply_config_timestamp_mono_;
  zx::time_boot last_valid_apply_config_timestamp_boot_;

  // TODO(b/475953032): Remove this property once it is no longer used.
  inspect::UintProperty last_valid_apply_config_timestamp_ns_property_;
  // Similar metric as above, except more explicit naming of the timeline used.
  inspect::UintProperty last_valid_apply_config_timestamp_mono_ns_property_;
  inspect::UintProperty last_valid_apply_config_timestamp_boot_ns_property_;

  inspect::UintProperty last_valid_apply_config_interval_ns_property_;
  // Similar metric as above, except more explicit naming of the timeline used.
  inspect::UintProperty last_valid_apply_config_duration_mono_ns_property_;
  inspect::UintProperty last_valid_apply_config_duration_boot_ns_property_;

  inspect::UintProperty last_valid_apply_config_config_stamp_property_;

  display::DriverConfigStamp last_issued_driver_config_stamp_ = display::kInvalidDriverConfigStamp;

  const uint32_t fallback_horizontal_size_mm_ = 0;
  const uint32_t fallback_vertical_size_mm_ = 0;
};

template <typename Callback>
bool Controller::FindDisplayInfo(display::DisplayId display_id, Callback callback) {
  for (const DisplayInfo& display : displays_) {
    if (display.id() == display_id) {
      callback(display);
      return true;
    }
  }
  return false;
}

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CONTROLLER_H_
