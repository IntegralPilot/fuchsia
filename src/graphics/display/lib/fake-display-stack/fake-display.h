// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_H_

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <optional>
#include <thread>
#include <unordered_map>
#include <vector>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-interface.h"
#include "src/graphics/display/lib/api-types/cpp/color.h"
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
#include "src/graphics/display/lib/fake-display-stack/fake-display-device-config.h"
#include "src/graphics/display/lib/fake-display-stack/image-info.h"

namespace fake_display {

// Fake implementation of the Display Engine protocol.
//
// This class has a complex threading model. See method-level comments for the
// thread safety properties of each method.
class FakeDisplay : public display::DisplayEngineInterface {
 public:
  // `engine_events` must outlive the newly constructed instance.
  explicit FakeDisplay(display::DisplayEngineEventsInterface* engine_events,
                       fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_client,
                       const FakeDisplayDeviceConfig& device_config, inspect::Inspector inspector);

  FakeDisplay(const FakeDisplay&) = delete;
  FakeDisplay& operator=(const FakeDisplay&) = delete;

  virtual ~FakeDisplay();

  // display::DisplayEngineInterface:
  //
  // When the driver is migrated to FIDL, these methods will only be called on
  // the Display Engine API serving dispatcher.
  display::EngineInfo CompleteCoordinatorConnection() override __TA_EXCLUDES(mutex_);
  zx::result<> ImportBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) override
      __TA_EXCLUDES(mutex_);
  zx::result<> ReleaseBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id) override __TA_EXCLUDES(mutex_);
  zx::result<display::DriverImageId> ImportImage(
      const display::ImageMetadata& image_metadata,
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) override
      __TA_EXCLUDES(mutex_);
  zx::result<display::DriverCaptureImageId> ImportImageForCapture(
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) override
      __TA_EXCLUDES(mutex_);
  void ReleaseImage(display::DriverImageId image_id) override __TA_EXCLUDES(mutex_);
  display::ConfigCheckResult CheckConfiguration(
      display::DisplayId display_id, display::ModeId display_mode_id,
      std::span<const display::DriverLayer> layers) override __TA_EXCLUDES(mutex_);
  void SubmitConfiguration(display::DisplayId display_id, display::ModeId display_mode_id,
                           std::span<const display::DriverLayer> layers,
                           display::DriverConfigStamp config_stamp) override __TA_EXCLUDES(mutex_);
  zx::result<> SetBufferCollectionConstraints(
      const display::ImageBufferUsage& image_buffer_usage,
      display::DriverBufferCollectionId buffer_collection_id) override __TA_EXCLUDES(mutex_);
  zx::result<> SetDisplayPowerMode(display::DisplayId display_id,
                                   display::PowerMode power_mode) override __TA_EXCLUDES(mutex_);
  zx::result<> StartCapture(display::DriverCaptureImageId capture_image_id) override
      __TA_EXCLUDES(mutex_);
  zx::result<> ReleaseCapture(display::DriverCaptureImageId capture_image_id) override
      __TA_EXCLUDES(mutex_);
  zx::result<> SetMinimumRgb(uint8_t minimum_rgb) override __TA_EXCLUDES(mutex_);

  // Can be called from any thread.
  bool IsCaptureSupported() const __TA_EXCLUDES(mutex_);

  // Manually triggers the VSync dispatch logic.
  //
  // Must only be called when periodic VSync is disabled. This restriction
  // serves to ensure good test design, and it is not based on implementation
  // limitations.
  //
  // May not be called before `SubmitConfiguration()`. Each VSync event must
  // include a valid `ConfigStamp`, which requires having an applied
  // configuration.
  //
  // Can be called from any thread.
  void TriggerVsync() __TA_EXCLUDES(mutex_);

  // Can be called from any thread.
  display::DriverConfigStamp LastSubmittedConfigStamp() const __TA_EXCLUDES(mutex_) {
    std::lock_guard lock(mutex_);
    return displayed_config_stamp_;
  }

  // Can be called from any thread.
  uint8_t GetClampRgbValue() const __TA_EXCLUDES(mutex_) {
    std::lock_guard lock(mutex_);
    return clamp_rgb_value_;
  }

  // Can be called from any thread.
  zx::vmo DuplicateInspectorVmoForTesting() __TA_EXCLUDES(mutex_) {
    return inspector_.DuplicateVmo();
  }

 private:
  enum class BufferCollectionUsage : int32_t;

  // The loop executed by the VSync thread.
  void VSyncThread() __TA_EXCLUDES(mutex_);

  // The loop executed by the capture thread.
  void CaptureThread() __TA_EXCLUDES(mutex_);

  // Dispatches an OnVsync() event to the Display Coordinator.
  //
  // Can be called from any thread.
  void SendVsync() __TA_EXCLUDES(mutex_);

  // Simulates a display capture, if a capture was requested.
  zx::result<> ServiceAnyCaptureRequest() __TA_EXCLUDES(mutex_);

  // Simulates a display capture for a single-layer image configuration.
  static zx::result<> DoImageCapture(DisplayImageInfo& source_info,
                                     CaptureImageInfo& destination_info);

  // Simulates a display capture for a single-layer color fill configuration.
  static zx::result<> DoColorFillCapture(display::Color fill_color,
                                         CaptureImageInfo& destination_info);

  // Must be called exactly once before using the Sysmem client.
  void InitializeSysmemClient() __TA_EXCLUDES(mutex_);

  fuchsia_sysmem2::wire::BufferCollectionConstraints CreateBufferCollectionConstraints(
      BufferCollectionUsage usage, fidl::AnyArena& arena);

  void SetCaptureImageFormatConstraints(
      fidl::WireTableBuilder<fuchsia_sysmem2::wire::ImageFormatConstraints>& constraints_builder);

  // Records the display config to the inspector's root node. The root node must
  // be already initialized.
  void RecordDisplayConfigToInspectRootNode();

  // Returns true iff `capture_image` is a valid capture target.
  bool IsValidCaptureTarget(const CaptureImageInfo& capture_image) const;

  // Composites layers in `layers` to `capture_target`.
  //
  // All image layers in `layers` must be accepted by `CheckConfiguration()`,
  // and their backing memory must be mappable for read.
  //
  // The `capture_target` image backing memory must be mappable for read-write.
  zx::result<> CompositeLayersToCaptureTargetLocked(std::span<display::DriverLayer> layers,
                                                    const CaptureImageInfo& capture_target)
      __TA_REQUIRES(mutex_);

  display::DisplayEngineEventsInterface& engine_events_;

  // Safe to access on multiple threads thanks to immutability.
  const FakeDisplayDeviceConfig device_config_;

  // Checked by the VSync thread on every loop iteration.
  std::atomic_bool vsync_thread_shutdown_requested_ = false;

  // Checked by the Capture thread on every loop iteration.
  std::atomic_bool capture_thread_shutdown_requested_ = false;

  // Only accessed by the constructor and destructor.
  std::optional<std::thread> vsync_thread_;
  std::optional<std::thread> capture_thread_;

  // Guards resources accessed by the VSync and capture threads.
  mutable std::mutex mutex_;

  // The sysmem allocator client used to bind incoming buffer collection tokens.
  //
  // When the driver is migrated to FIDL, this member will only be accessed on
  // the Display Engine API serving dispatcher.
  fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_client_ __TA_GUARDED(mutex_);

  // Owns the Sysmem buffer collections imported into the driver.
  //
  // When the driver is migrated to FIDL, this member will only be accessed on
  // the Display Engine API serving dispatcher.
  std::unordered_map<display::DriverBufferCollectionId,
                     fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>>
      buffer_collections_ __TA_GUARDED(mutex_);

  // Owns the display images imported into the driver.
  DisplayImageInfo::HashTable imported_images_ __TA_GUARDED(mutex_);

  // Layers applied in the last `SubmitConfiguration()` call.
  std::vector<display::DriverLayer> applied_layers_ __TA_GUARDED(mutex_);

  // Next image ID assigned to an imported image.
  //
  // When the driver is migrated to FIDL, this member will only be accessed on
  // the Display Engine API serving dispatcher.
  display::DriverImageId next_imported_display_driver_image_id_ __TA_GUARDED(mutex_) =
      display::DriverImageId(1);

  // Owns the capture images imported into the driver.
  CaptureImageInfo::HashTable imported_captures_ __TA_GUARDED(mutex_);

  // ID of the destination image of the currently in-progress capture.
  //
  // Set to `kInvalidDriverCaptureImageId` when no capture is in progress.
  display::DriverCaptureImageId started_capture_target_id_ __TA_GUARDED(mutex_) =
      display::kInvalidDriverCaptureImageId;

  // Next image ID assigned to an imported capture image.
  //
  // When the driver is migrated to FIDL, this member will only be accessed on
  // the Display Engine API serving dispatcher.
  display::DriverCaptureImageId next_imported_driver_capture_image_id_ __TA_GUARDED(mutex_) =
      display::DriverCaptureImageId(1);

  // The config stamp of the applied display configuration.
  //
  // Updated by SubmitConfiguration(), used by the VSync thread.
  display::DriverConfigStamp displayed_config_stamp_ __TA_GUARDED(mutex_) =
      display::kInvalidDriverConfigStamp;

  // Minimum value of RGB channels, via the SetMinimumRgb() method.
  //
  // This is associated with the display capture lock so we have the option to
  // reflect the clamping when we simulate display capture.
  uint8_t clamp_rgb_value_ __TA_GUARDED(mutex_) = 0;

  inspect::Inspector inspector_;
};

}  // namespace fake_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_DISPLAY_H_
