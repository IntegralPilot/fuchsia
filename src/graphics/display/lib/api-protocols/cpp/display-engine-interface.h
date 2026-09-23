// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_INTERFACE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_INTERFACE_H_

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/zx/result.h>

#include <cstdint>
#include <span>

#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
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
#include "src/graphics/display/lib/api-types/cpp/power-mode.h"

namespace display {

// Equivalent to the FIDL protocol [`fuchsia.hardware.display.engine/Engine`].
class DisplayEngineInterface {
 public:
  DisplayEngineInterface() = default;

  DisplayEngineInterface(const DisplayEngineInterface&) = delete;
  DisplayEngineInterface(DisplayEngineInterface&&) = delete;
  DisplayEngineInterface& operator=(const DisplayEngineInterface&) = delete;
  DisplayEngineInterface& operator=(DisplayEngineInterface&&) = delete;

  // The engine listener is connected when this method is called.
  virtual EngineInfo CompleteCoordinatorConnection() = 0;

  virtual zx::result<> ImportBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) = 0;
  virtual zx::result<> ReleaseBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id) = 0;

  virtual zx::result<display::DriverImageId> ImportImage(
      const display::ImageMetadata& image_metadata,
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) = 0;
  virtual zx::result<display::DriverCaptureImageId> ImportImageForCapture(
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) = 0;
  virtual void ReleaseImage(display::DriverImageId driver_image_id) = 0;

  // Display engine drivers must override **exactly one** of the following
  // `CheckConfiguration()` methods.

  virtual display::ConfigCheckResult CheckConfiguration(
      display::DisplayId display_id, display::ModeId display_mode_id,
      std::span<const display::DriverLayer> layers);
  // Out-of-tree drivers must not override this overload, because it will be
  // reworked.
  virtual display::ConfigCheckResult CheckConfiguration(
      display::DisplayId display_id, display::ModeId display_mode_id,
      display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers);

  // Display engine drivers must override **exactly one** of the following
  // `SubmitConfiguration()` methods.

  virtual void SubmitConfiguration(display::DisplayId display_id, display::ModeId display_mode_id,
                                   std::span<const display::DriverLayer> layers,
                                   display::DriverConfigStamp driver_config_stamp);
  // Out-of-tree drivers must not override this overload, because it will be
  // reworked.
  virtual void SubmitConfiguration(display::DisplayId display_id, display::ModeId display_mode_id,
                                   display::ColorConversion color_conversion,
                                   std::span<const display::DriverLayer> layers,
                                   display::DriverConfigStamp driver_config_stamp);

  virtual zx::result<> SetBufferCollectionConstraints(
      const display::ImageBufferUsage& image_buffer_usage,
      display::DriverBufferCollectionId buffer_collection_id) = 0;

  virtual zx::result<> SetDisplayPowerMode(display::DisplayId display_id,
                                           display::PowerMode power_mode);

  // OOT drivers must use the default implementation for the capture interface.
  // The interface is not stabilized and will change.
  virtual zx::result<> StartCapture(display::DriverCaptureImageId capture_image_id);
  virtual zx::result<> ReleaseCapture(display::DriverCaptureImageId capture_image_id);

  // OOT drivers must use the default implementation for SetMinimumRgb().
  // The interface is not stabilized and will change.
  virtual zx::result<> SetMinimumRgb(uint8_t minimum_rgb);

 protected:
  // Destruction via base class pointer is not supported intentionally.
  // Instances are not expected to be owned by pointers to base classes.
  ~DisplayEngineInterface() = default;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_INTERFACE_H_
