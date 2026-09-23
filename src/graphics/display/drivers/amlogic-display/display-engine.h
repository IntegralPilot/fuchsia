// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_DISPLAY_ENGINE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_DISPLAY_ENGINE_H_

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/interrupt.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <span>

#include <fbl/intrusive_double_list.h>
#include <fbl/mutex.h>

#include "src/graphics/display/drivers/amlogic-display/capture.h"
#include "src/graphics/display/drivers/amlogic-display/common.h"
#include "src/graphics/display/drivers/amlogic-display/hot-plug-detection.h"
#include "src/graphics/display/drivers/amlogic-display/image-info.h"
#include "src/graphics/display/drivers/amlogic-display/structured_config.h"
#include "src/graphics/display/drivers/amlogic-display/video-input-unit.h"
#include "src/graphics/display/drivers/amlogic-display/vout.h"
#include "src/graphics/display/drivers/amlogic-display/vpu.h"
#include "src/graphics/display/drivers/amlogic-display/vsync-receiver.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-interface.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"

namespace amlogic_display {

class DisplayEngine final : public display::DisplayEngineInterface {
 public:
  // Factory method for production use.
  //
  // `incoming` must be non-null.
  // `engine_events` must not be null and must outlive `DisplayEngine`.
  static zx::result<std::unique_ptr<DisplayEngine>> Create(
      std::shared_ptr<fdf::Namespace> incoming,
      display::DisplayEngineEventsInterface* engine_events,
      structured_config::Config structured_config);

  // Creates an uninitialized `DisplayEngine` instance.
  //
  // `incoming` must be non-null.
  // `engine_events` must not be null and must outlive `DisplayEngine`.
  //
  // Production code should use `DisplayEngine::Create()` instead.
  DisplayEngine(std::shared_ptr<fdf::Namespace> incoming,
                display::DisplayEngineEventsInterface* engine_events,
                structured_config::Config structured_config);

  DisplayEngine(const DisplayEngine&) = delete;
  DisplayEngine(DisplayEngine&&) = delete;
  DisplayEngine& operator=(const DisplayEngine&) = delete;
  DisplayEngine& operator=(DisplayEngine&&) = delete;

  ~DisplayEngine();

  // Acquires parent resources and sets up display submodules.
  //
  // Must be called exactly once via the `Create()` factory method during
  // the DisplayEngine lifetime in production code.
  //
  // TODO(https://fxbug.dev/42082357): Replace the two-step initialization with
  // a factory method and a constructor.
  zx_status_t Initialize();

  // Tears down display submodules and turns off the hardware.
  //
  // Must be called before the DisplayEngine instance is destroyed in production code,
  // or called automatically by the destructor.
  void Deinitialize();

  // `display::DisplayEngineInterface`:
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
      display::DisplayId display_id, display::ModeId mode_id,
      display::ColorConversion color_conversion,
      std::span<const display::DriverLayer> layers) override;
  void SubmitConfiguration(display::DisplayId display_id, display::ModeId mode_id,
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

  const inspect::Inspector& inspector() const { return inspector_; }

  void Dump() { vout_->Dump(); }

  void SetFormatSupportCheck(fit::function<bool(display::PixelFormat)> fn) {
    format_support_check_ = std::move(fn);
  }

  // Acquires the common protocols (pdev, sysmem and amlogic-canvas) and
  // resources (Vsync interrupts, capture interrupts and BTIs) from the parent
  // nodes.
  //
  // Must be called once and only once per driver binding procedure, before
  // the protocols / resources mentioned above being used.
  //
  // This is meant for private use, but is marked public because the unit test
  // needs to be able to call it as well to bypass the full `Initialize()` flow.
  zx_status_t GetCommonProtocolsAndResources();

  void SetVoutForTesting(std::unique_ptr<Vout> vout) { vout_ = std::move(vout); }

  void SetVideoInputUnitForTesting(std::unique_ptr<VideoInputUnit> video_input_unit) {
    video_input_unit_ = std::move(video_input_unit);
  }

 private:
  void PopulatePanelType() __TA_REQUIRES(display_mutex_);

  void OnHotPlugStateChange(HotPlugDetectionState current_state);
  void OnVsync(zx::time_monotonic timestamp);
  void OnCaptureComplete();

  // TODO(https://fxbug.dev/42082357): Currently, DisplayEngine has a multi-step
  // initialization procedure when the device manager binds the driver to the
  // device node. This makes the initialization stateful and hard to maintain
  // (submodules may have dependencies on initialization). Instead, it's
  // preferred to turn the initialization procedure into a builder pattern,
  // where resources are acquired before the device is created, and the device
  // is guaranteed to be ready at creation time.

  // Must be called once and only once per driver binding procedure.
  //
  // `GetCommonProtocolsAndResources()` must be called to acquire sysmem
  // protocol before this is called.
  zx_status_t InitializeSysmemAllocator();

  // Allocates and initializes the video output (Vout) hardware submodule of
  // the Video Processing Unit. The type of display (MIPI-DSI or HDMI) is based
  // on the panel metadata provided by the board driver.
  //
  // Must be called once and only once per driver binding procedure.
  // `vout_` must not be allocated nor initialized.
  zx_status_t InitializeVout();

  // A helper allocating and initializing the video output (Vout) hardware
  // submodule for MIPI-DSI displays.
  //
  // `vout_` must not be allocated nor initialized.
  zx_status_t InitializeMipiDsiVout(display::PanelType panel_type);

  // A helper allocating and initializing the video output (Vout) hardware
  // submodule for HDMI displays.
  //
  // `vout_` must not be allocated nor initialized.
  zx_status_t InitializeHdmiVout();

  // Acquires the hotplug display detection hardware resources (GPIO protocol
  // and interrupt for the hotplug detection GPIO pin) from parent nodes, and
  // starts the interrupt handler thread.
  //
  // Must be called once and only once per driver binding procedure, if display
  // hotplug is supported.
  zx_status_t SetupHotplugDisplayDetection();

  // Power cycles the display engine, resets all configurations and re-brings up
  // the video output module. This will cause all the previous display hardware
  // state to be lost.
  //
  // Must only be called during `DisplayEngine` initialization, after `vpu_`
  // and `vout_` are initialized, but before any IRQ handler thread starts
  // running, as access to display resources (registers, IRQs) after powering
  // off the display engine will cause the system to crash.
  zx::result<> ResetDisplayEngine() __TA_REQUIRES(display_mutex_);

  bool fully_initialized() const { return full_init_done_.load(std::memory_order_relaxed); }
  void set_fully_initialized() { full_init_done_.store(true, std::memory_order_release); }

  std::shared_ptr<fdf::Namespace> incoming_;

  // Zircon handles
  zx::bti bti_;

  // Protocol handles used in by this driver
  fidl::WireSyncClient<fuchsia_hardware_platform_device::Device> pdev_;
  fidl::WireSyncClient<fuchsia_hardware_amlogiccanvas::Device> canvas_;
  // The sysmem allocator client used to bind incoming buffer collection tokens.
  fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_;

  // Locks used by the display driver
  fbl::Mutex display_mutex_;  // general display state (i.e. display_id)
  fbl::Mutex image_mutex_;    // used for accessing imported_images_
  fbl::Mutex capture_mutex_;  // general capture state

  // Relaxed is safe because full_init_done_ only ever moves from false to true.
  std::atomic<bool> full_init_done_ = false;

  // Emits engine events over the EngineListener interface.
  display::DisplayEngineEventsInterface& engine_events_ __TA_GUARDED(display_mutex_);

  // Points to the next capture target image to capture displayed contents into.
  // Stores nullptr if capture is not going to be performed.
  ImageInfo* current_capture_target_image_ __TA_GUARDED(capture_mutex_) = nullptr;

  // Imported sysmem buffer collections.
  std::unordered_map<display::DriverBufferCollectionId,
                     fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>>
      buffer_collections_;

  // Imported Images
  fbl::DoublyLinkedList<std::unique_ptr<ImageInfo>> imported_images_ __TA_GUARDED(image_mutex_);
  fbl::DoublyLinkedList<std::unique_ptr<ImageInfo>> imported_captures_ __TA_GUARDED(capture_mutex_);

  // Monitoring. We create a named "amlogic-display" node to allow for easier filtering
  // of inspect tree when defining selectors and metrics.
  inspect::Inspector inspector_;
  inspect::Node root_node_;
  inspect::Node video_input_unit_node_;

  // Objects: only valid if fully_initialized()
  std::unique_ptr<Vpu> vpu_;
  std::unique_ptr<VideoInputUnit> video_input_unit_;
  std::unique_ptr<Vout> vout_;

  display::DisplayId display_id_ __TA_GUARDED(display_mutex_) = kPanelDisplayId;
  bool display_attached_ __TA_GUARDED(display_mutex_) = false;

  std::unique_ptr<HotPlugDetection> hot_plug_detection_;
  std::unique_ptr<Capture> capture_;
  std::unique_ptr<VsyncReceiver> vsync_receiver_;

  fit::function<bool(display::PixelFormat format)> format_support_check_ = nullptr;

  structured_config::Config structured_config_;
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_DISPLAY_ENGINE_H_
