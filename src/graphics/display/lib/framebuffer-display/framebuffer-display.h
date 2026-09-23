// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FRAMEBUFFER_DISPLAY_FRAMEBUFFER_DISPLAY_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FRAMEBUFFER_DISPLAY_FRAMEBUFFER_DISPLAY_H_

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/fit/function.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>

#include <atomic>
#include <cstdint>
#include <memory>
#include <mutex>
#include <unordered_map>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-interface.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace framebuffer_display {

struct DisplayProperties {
  int32_t width_px;
  int32_t height_px;
  int32_t row_stride_px;
  display::PixelFormat pixel_format;
  // Zero preserves the existing direct-copy path. Otherwise, input is BGRA8888.
  uint32_t framebuffer_format = 0;
};

class FramebufferDisplay final : public display::DisplayEngineInterface {
 public:
  // `dispatcher` must be non-null and outlive the newly created instance.
  FramebufferDisplay(display::DisplayEngineEventsInterface* engine_events,
                     fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_client,
                     fdf::MmioBuffer framebuffer_mmio, const DisplayProperties& properties,
                     async_dispatcher_t* dispatcher);
  ~FramebufferDisplay();

  // Initialization logic not suitable in the constructor.
  //
  // Must be called at most once before any other method.
  zx::result<> Initialize();

  // Tears down resources initialized in `Initialize()`.
  //
  // Safe to call multiple times or when `Initialize()` has not been called (in
  // which case this is a no-op).
  void Deinitialize();

  // DisplayEngineInterface:
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
      std::span<const display::DriverLayer> layers) override;
  void SubmitConfiguration(display::DisplayId display_id, display::ModeId display_mode_id,
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

  const std::unordered_map<display::DriverBufferCollectionId,
                           fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>>&
  GetBufferCollectionsForTesting() const {
    return buffer_collections_;
  }

 private:
  void OnPeriodicVSync(async_dispatcher_t* dispatcher, async::TaskBase* task, zx_status_t status);

  // The sysmem allocator client used to bind incoming buffer collection tokens.
  fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_client_;

  // Imported sysmem buffer collections.
  std::unordered_map<display::DriverBufferCollectionId,
                     fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>>
      buffer_collections_;

  async_dispatcher_t& dispatcher_;
  async::TaskMethod<FramebufferDisplay, &FramebufferDisplay::OnPeriodicVSync> vsync_task_{this};

  // Accessed only from the display engine driver dispatcher (ImportImage, ReleaseImage,
  // SubmitConfiguration); no synchronization needed.
  std::unordered_map<display::DriverImageId, fzl::VmoMapper> imported_images_;
  display::DriverImageId next_image_id_{1};

  static_assert(std::atomic<bool>::is_always_lock_free);
  std::atomic<bool> has_image_;
  std::atomic<bool> initialized_{false};

  // A lock is required to ensure the atomicity when setting |config_stamp| in
  // |SubmitConfiguration()| and passing |&config_stamp_| to |OnDisplayVsync()|.
  std::mutex mtx_;
  display::DriverConfigStamp config_stamp_ TA_GUARDED(mtx_) = display::kInvalidDriverConfigStamp;

  // Note that the underlying framebuffer is in regular RAM, not a device register window.
  const fdf::MmioBuffer framebuffer_mmio_;
  const DisplayProperties properties_;
  std::unique_ptr<uint32_t[]> conversion_row_;

  const fuchsia_images2::wire::PixelFormatModifier kFormatModifier =
      fuchsia_images2::wire::PixelFormatModifier::kLinear;

  // Only used on the vsync thread.
  zx::time_monotonic next_vsync_time_;

  display::DisplayEngineEventsInterface& engine_events_;
};

}  // namespace framebuffer_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FRAMEBUFFER_DISPLAY_FRAMEBUFFER_DISPLAY_H_
