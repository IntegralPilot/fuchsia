// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_GOLDFISH_DISPLAY_DISPLAY_ENGINE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_GOLDFISH_DISPLAY_DISPLAY_ENGINE_H_

#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fzl/pinned-vmo.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/result.h>
#include <threads.h>
#include <zircon/types.h>

#include <list>
#include <memory>

#include <fbl/mutex.h>

#include "src/graphics/display/drivers/goldfish-display/render_control.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-interface.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"

namespace goldfish {

class DisplayEngine final : public display::DisplayEngineInterface {
 public:
  // `control`, `pipe`, `sysmem_allocator` must be valid.
  // `render_control` must not be null.
  // `display_event_dispatcher` must be non-null and outlive `DisplayEngine`.
  // `engine_events` must not be null and must outlive `DisplayEngine`.
  explicit DisplayEngine(fidl::ClientEnd<fuchsia_hardware_goldfish::ControlDevice> control,
                         fidl::ClientEnd<fuchsia_hardware_goldfish_pipe::Bus> pipe_bus,
                         fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_allocator,
                         std::unique_ptr<RenderControl> render_control,
                         async_dispatcher_t* display_event_dispatcher,
                         display::DisplayEngineEventsInterface* engine_events);

  DisplayEngine(const DisplayEngine&) = delete;
  DisplayEngine(DisplayEngine&&) = delete;
  DisplayEngine& operator=(const DisplayEngine&) = delete;
  DisplayEngine& operator=(DisplayEngine&&) = delete;

  ~DisplayEngine();

  // Performs initialization that cannot be done in the constructor.
  zx::result<> Initialize();

  // display::DisplayEngineInterface
  display::EngineInfo CompleteCoordinatorConnection() override;

  zx::result<> ImportBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) override;
  zx::result<> ReleaseBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id) override;
  zx::result<display::DriverImageId> ImportImage(
      const display::ImageMetadata& image_metadata,
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) override;
  zx::result<display::DriverCaptureImageId> ImportImageForCapture(
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) override;
  void ReleaseImage(display::DriverImageId image_id) override;
  display::ConfigCheckResult CheckConfiguration(
      display::DisplayId display_id, display::ModeId display_mode_id,
      display::ColorConversion color_conversion,
      std::span<const display::DriverLayer> layers) override;
  void SubmitConfiguration(display::DisplayId display_id, display::ModeId display_mode_id,
                           display::ColorConversion color_conversion,
                           std::span<const display::DriverLayer> layers,
                           display::DriverConfigStamp config_stamp) override;
  zx::result<> SetBufferCollectionConstraints(
      const display::ImageBufferUsage& image_buffer_usage,
      display::DriverBufferCollectionId buffer_collection_id) override;
  zx::result<> SetDisplayPowerMode(display::DisplayId display_id,
                                   display::PowerMode power_mode) override;
  zx::result<> StartCapture(display::DriverCaptureImageId capture_image_id) override;
  zx::result<> ReleaseCapture(display::DriverCaptureImageId capture_image_id) override;
  zx::result<> SetMinimumRgb(uint8_t minimum_rgb) override;

  void SetupPrimaryDisplayForTesting(int32_t width_px, int32_t height_px, int32_t refresh_rate_hz);

 private:
  struct ColorBuffer {
    ~ColorBuffer() = default;

    HostColorBufferId host_color_buffer_id = kInvalidHostColorBufferId;
    size_t size = 0;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t format = 0;

    // TODO(costan): Rename to reflect ownership.
    bool is_linear_format = false;

    zx::vmo vmo;
    fzl::PinnedVmo pinned_vmo;
  };

  struct DisplayConfig {
    // For displays with image framebuffer attached to the display, the
    // framebuffer is represented as a |ColorBuffer| in goldfish graphics
    // device implementation.
    // A configuration with a non-null |color_buffer| field means that it will
    // present this |ColorBuffer| image at Vsync; the |ColorBuffer| instance
    // will be created when importing the image and destroyed when releasing
    // the image or removing the display device. Otherwise, it means the display
    // has no framebuffers to display.
    ColorBuffer* color_buffer = nullptr;

    // The |config_stamp| value of the SubmitConfiguration() call to which this
    // DisplayConfig corresponds.
    display::DriverConfigStamp config_stamp = display::kInvalidDriverConfigStamp;
  };

  // TODO(https://fxbug.dev/335324453): Define DisplayState as a class with
  // proper rep invariants on each config update / config flush.
  struct DisplayState {
    int32_t width_px = 0;
    int32_t height_px = 0;
    int32_t refresh_rate_hz = 60;

    zx::time_monotonic expected_next_flush = zx::time_monotonic::infinite_past();
    display::DriverConfigStamp latest_config_stamp = display::kInvalidDriverConfigStamp;

    // The next display config to be posted through renderControl protocol.
    std::optional<DisplayConfig> incoming_config = std::nullopt;

    // Queues the async wait of goldfish sync device for each frame that is
    // posted (rendered) but hasn't finished rendering.
    //
    // Every time there's a new frame posted through renderControl protocol,
    // a WaitOnce waiting on the sync event for the latest config will be
    // appended to the queue. When a frame has finished rendering on host, all
    // the pending Waits that are queued no later than the frame's async Wait
    // (including the frame's Wait itself) will be popped out from the queue
    // and destroyed.
    std::list<async::WaitOnce> pending_config_waits;
  };

  zx::result<display::DriverImageId> ImportVmoImage(
      const display::ImageMetadata& image_metadata,
      const fuchsia_images2::PixelFormat& pixel_format, zx::vmo vmo, size_t offset);

  zx_status_t SetupPrimaryDisplay();
  zx_status_t PresentPrimaryDisplayConfig(const DisplayConfig& display_config);
  void FlushPrimaryDisplay(async_dispatcher_t* dispatcher);

  fbl::Mutex lock_;
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> control_ TA_GUARDED(lock_);
  fidl::WireSyncClient<fuchsia_hardware_goldfish_pipe::Bus> pipe_bus_ TA_GUARDED(lock_);

  // The sysmem allocator client used to bind incoming buffer collection tokens.
  fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_allocator_client_;

  // Imported sysmem buffer collections.
  std::unordered_map<display::DriverBufferCollectionId,
                     fidl::SyncClient<fuchsia_sysmem2::BufferCollection>>
      buffer_collections_;

  std::unique_ptr<RenderControl> rc_;
  DisplayState primary_display_device_ = {};
  fbl::Mutex flush_lock_;

  async_dispatcher_t* const display_event_dispatcher_;

  // The display coordinator events are Sink'd through this interface.
  // This must not be null and must outlive this object.
  display::DisplayEngineEventsInterface* const engine_events_;
};

}  // namespace goldfish

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_GOLDFISH_DISPLAY_DISPLAY_ENGINE_H_
