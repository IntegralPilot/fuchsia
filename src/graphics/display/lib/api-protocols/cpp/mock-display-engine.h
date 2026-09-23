// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_MOCK_DISPLAY_ENGINE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_MOCK_DISPLAY_ENGINE_H_

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <cstddef>
#include <cstdint>
#include <mutex>
#include <span>
#include <vector>

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

namespace display::testing {

// Strict mock for DisplayEngineInterface implementations.
//
// This is a very rare case where strict mocking is warranted. The code under
// test is an adapter that maps FIDL calls 1:1 to C++ calls. So, the API
// contract being tested is expressed in terms of individual function calls.
class MockDisplayEngine : public display::DisplayEngineInterface {
 public:
  // Expectation containers for display::DisplayEngineInterface:
  using CompleteCoordinatorConnectionChecker = fit::function<display::EngineInfo()>;
  using ImportBufferCollectionChecker = fit::function<zx::result<>(
      display::DriverBufferCollectionId buffer_collection_id,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token)>;
  using ReleaseBufferCollectionChecker =
      fit::function<zx::result<>(display::DriverBufferCollectionId buffer_collection_id)>;
  using ImportImageChecker = fit::function<zx::result<display::DriverImageId>(
      const display::ImageMetadata& image_metadata,
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index)>;
  using ImportImageForCaptureChecker = fit::function<zx::result<display::DriverCaptureImageId>(
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index)>;
  using ReleaseImageChecker = fit::function<void(display::DriverImageId driver_image_id)>;
  using CheckConfigurationChecker = fit::function<display::ConfigCheckResult(
      display::DisplayId display_id, display::ModeId display_mode_id,
      display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers)>;
  using SubmitConfigurationChecker = fit::function<void(
      display::DisplayId display_id, display::ModeId display_mode_id,
      display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers,
      display::DriverConfigStamp config_stamp)>;
  using SetBufferCollectionConstraintsChecker =
      fit::function<zx::result<>(const display::ImageBufferUsage& image_buffer_usage,
                                 display::DriverBufferCollectionId buffer_collection_id)>;
  using SetDisplayPowerModeChecker =
      fit::function<zx::result<>(display::DisplayId display_id, display::PowerMode power_mode)>;
  using IsCaptureSupportedChecker = fit::function<bool()>;
  using StartCaptureChecker =
      fit::function<zx::result<>(display::DriverCaptureImageId capture_image_id)>;
  using ReleaseCaptureChecker =
      fit::function<zx::result<>(display::DriverCaptureImageId capture_image_id)>;
  using SetMinimumRgbChecker = fit::function<zx::result<>(uint8_t minimum_rgb)>;

  MockDisplayEngine();
  MockDisplayEngine(const MockDisplayEngine&) = delete;
  MockDisplayEngine& operator=(const MockDisplayEngine&) = delete;
  ~MockDisplayEngine();

  // Expectations for display::DisplayEngineInterface:
  void ExpectCompleteCoordinatorConnection(CompleteCoordinatorConnectionChecker checker);
  void ExpectImportBufferCollection(ImportBufferCollectionChecker checker);
  void ExpectReleaseBufferCollection(ReleaseBufferCollectionChecker checker);
  void ExpectImportImage(ImportImageChecker checker);
  void ExpectImportImageForCapture(ImportImageForCaptureChecker checker);
  void ExpectReleaseImage(ReleaseImageChecker checker);
  void ExpectCheckConfiguration(CheckConfigurationChecker checker);
  void ExpectSubmitConfiguration(SubmitConfigurationChecker checker);
  void ExpectSetBufferCollectionConstraints(SetBufferCollectionConstraintsChecker checker);
  void ExpectSetDisplayPowerMode(SetDisplayPowerModeChecker checker);
  void ExpectIsCaptureSupported(IsCaptureSupportedChecker checker);
  void ExpectStartCapture(StartCaptureChecker checker);
  void ExpectReleaseCapture(ReleaseCaptureChecker checker);
  void ExpectSetMinimumRgb(SetMinimumRgbChecker checker);

  // Must be called at least once during an instance's lifetime.
  //
  // Tests are recommended to call this in a TearDown() method, or at the end of
  // the test case implementation.
  void CheckAllCallsReplayed();

  // display::DisplayEngineInterface:
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
  void ReleaseImage(display::DriverImageId driver_image_id) override;
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

 private:
  struct Expectation;

  std::mutex mutex_;
  std::vector<Expectation> expectations_ __TA_GUARDED(mutex_);
  size_t call_index_ __TA_GUARDED(mutex_) = 0;
  bool check_all_calls_replayed_called_ __TA_GUARDED(mutex_) = false;
};

}  // namespace display::testing

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_MOCK_DISPLAY_ENGINE_H_
