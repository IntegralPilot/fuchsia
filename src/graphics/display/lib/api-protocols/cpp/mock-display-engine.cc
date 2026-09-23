// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/mock-display-engine.h"

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cstdint>
#include <mutex>
#include <span>
#include <utility>

#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"

namespace display::testing {

struct MockDisplayEngine::Expectation {
  CompleteCoordinatorConnectionChecker complete_coordinator_connection_checker;
  ImportBufferCollectionChecker import_buffer_collection_checker;
  ReleaseBufferCollectionChecker release_buffer_collection_checker;
  ImportImageChecker import_image_checker;
  ImportImageForCaptureChecker import_image_for_capture_checker;
  ReleaseImageChecker release_image_checker;
  CheckConfigurationChecker check_configuration_checker;
  SubmitConfigurationChecker submit_configuration_checker;
  SetBufferCollectionConstraintsChecker set_buffer_collection_constraints_checker;
  SetDisplayPowerModeChecker set_display_power_mode_checker;
  IsCaptureSupportedChecker is_capture_supported_checker;
  StartCaptureChecker start_capture_checker;
  ReleaseCaptureChecker release_capture_checker;
  SetMinimumRgbChecker set_minimum_rgb_checker;
};

MockDisplayEngine::MockDisplayEngine() = default;

MockDisplayEngine::~MockDisplayEngine() {
  ZX_ASSERT_MSG(check_all_calls_replayed_called_, "CheckAllCallsReplayed() not called on a mock");
}

void MockDisplayEngine::ExpectCompleteCoordinatorConnection(
    CompleteCoordinatorConnectionChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.complete_coordinator_connection_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectImportBufferCollection(ImportBufferCollectionChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.import_buffer_collection_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectReleaseBufferCollection(ReleaseBufferCollectionChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.release_buffer_collection_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectImportImage(ImportImageChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.import_image_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectImportImageForCapture(ImportImageForCaptureChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.import_image_for_capture_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectReleaseImage(ReleaseImageChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.release_image_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectCheckConfiguration(CheckConfigurationChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.check_configuration_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectSubmitConfiguration(SubmitConfigurationChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.submit_configuration_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectSetBufferCollectionConstraints(
    SetBufferCollectionConstraintsChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.set_buffer_collection_constraints_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectSetDisplayPowerMode(SetDisplayPowerModeChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.set_display_power_mode_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectIsCaptureSupported(IsCaptureSupportedChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.is_capture_supported_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectStartCapture(StartCaptureChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.start_capture_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectReleaseCapture(ReleaseCaptureChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.release_capture_checker = std::move(checker)});
}

void MockDisplayEngine::ExpectSetMinimumRgb(SetMinimumRgbChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.set_minimum_rgb_checker = std::move(checker)});
}

void MockDisplayEngine::CheckAllCallsReplayed() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(expectations_.size() == call_index_, "%zu expected calls were not received",
                expectations_.size() - call_index_);
  check_all_calls_replayed_called_ = true;
}

display::EngineInfo MockDisplayEngine::CompleteCoordinatorConnection() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.complete_coordinator_connection_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.complete_coordinator_connection_checker();
}

zx::result<> MockDisplayEngine::ImportBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.import_buffer_collection_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.import_buffer_collection_checker(buffer_collection_id,
                                                           std::move(buffer_collection_token));
}

zx::result<> MockDisplayEngine::ReleaseBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.release_buffer_collection_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.release_buffer_collection_checker(buffer_collection_id);
}

zx::result<display::DriverImageId> MockDisplayEngine::ImportImage(
    const display::ImageMetadata& image_metadata,
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.import_image_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.import_image_checker(image_metadata, buffer_collection_id, buffer_index);
}

zx::result<display::DriverCaptureImageId> MockDisplayEngine::ImportImageForCapture(
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.import_image_for_capture_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.import_image_for_capture_checker(buffer_collection_id, buffer_index);
}

void MockDisplayEngine::ReleaseImage(display::DriverImageId driver_image_id) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.release_image_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.release_image_checker(driver_image_id);
}

display::ConfigCheckResult MockDisplayEngine::CheckConfiguration(
    display::DisplayId display_id, display::ModeId display_mode_id,
    display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.check_configuration_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.check_configuration_checker(display_id, display_mode_id, color_conversion,
                                                      layers);
}

void MockDisplayEngine::SubmitConfiguration(display::DisplayId display_id,
                                            display::ModeId display_mode_id,
                                            display::ColorConversion color_conversion,
                                            std::span<const display::DriverLayer> layers,
                                            display::DriverConfigStamp config_stamp) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.submit_configuration_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.submit_configuration_checker(display_id, display_mode_id, color_conversion,
                                                layers, config_stamp);
}

zx::result<> MockDisplayEngine::SetBufferCollectionConstraints(
    const display::ImageBufferUsage& image_buffer_usage,
    display::DriverBufferCollectionId buffer_collection_id) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.set_buffer_collection_constraints_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.set_buffer_collection_constraints_checker(image_buffer_usage,
                                                                    buffer_collection_id);
}

zx::result<> MockDisplayEngine::SetDisplayPowerMode(display::DisplayId display_id,
                                                    display::PowerMode power_mode) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.set_display_power_mode_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.set_display_power_mode_checker(display_id, power_mode);
}

zx::result<> MockDisplayEngine::StartCapture(display::DriverCaptureImageId capture_image_id) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.start_capture_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.start_capture_checker(capture_image_id);
}

zx::result<> MockDisplayEngine::ReleaseCapture(display::DriverCaptureImageId capture_image_id) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.release_capture_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.release_capture_checker(capture_image_id);
}

zx::result<> MockDisplayEngine::SetMinimumRgb(uint8_t minimum_rgb) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.set_minimum_rgb_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.set_minimum_rgb_checker(minimum_rgb);
}

}  // namespace display::testing
