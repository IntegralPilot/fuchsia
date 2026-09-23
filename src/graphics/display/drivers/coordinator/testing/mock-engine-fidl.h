// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_ENGINE_FIDL_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_ENGINE_FIDL_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <lib/fdf/cpp/arena.h>
#include <lib/fit/function.h>
#include <zircon/compiler.h>

#include <cstddef>
#include <mutex>
#include <vector>

namespace display_coordinator::testing {

// Strict mock for the FIDL-generated display Engine protocol.
//
// This is a very rare case where strict mocking is warranted. The code under
// test is an adapter that maps C++ calls 1:1 to FIDL calls. So, the API
// contract being tested is expressed in terms of individual function calls.
class MockEngineFidl final : public fdf::WireServer<fuchsia_hardware_display_engine::Engine> {
 public:
  // Expectation containers for fuchsia.hardware.display.engine/Engine:
  using CompleteCoordinatorConnectionChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineCompleteCoordinatorConnectionRequest* request,
      fdf::Arena& arena, CompleteCoordinatorConnectionCompleter::Sync& completer)>;
  using UnsetListenerChecker =
      fit::function<void(fdf::Arena& arena, UnsetListenerCompleter::Sync& completer)>;
  using ImportBufferCollectionChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineImportBufferCollectionRequest* request,
      fdf::Arena& arena, ImportBufferCollectionCompleter::Sync& completer)>;
  using ReleaseBufferCollectionChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineReleaseBufferCollectionRequest* request,
      fdf::Arena& arena, ReleaseBufferCollectionCompleter::Sync& completer)>;
  using ImportImageChecker =
      fit::function<void(fuchsia_hardware_display_engine::wire::EngineImportImageRequest* request,
                         fdf::Arena& arena, ImportImageCompleter::Sync& completer)>;
  using ImportImageForCaptureChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineImportImageForCaptureRequest* request,
      fdf::Arena& arena, ImportImageForCaptureCompleter::Sync& completer)>;
  using ReleaseImageChecker =
      fit::function<void(fuchsia_hardware_display_engine::wire::EngineReleaseImageRequest* request,
                         fdf::Arena& arena)>;
  using CheckConfigurationChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineCheckConfigurationRequest* request,
      fdf::Arena& arena, CheckConfigurationCompleter::Sync& completer)>;
  using SubmitConfigurationChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineSubmitConfigurationRequest* request,
      fdf::Arena& arena, SubmitConfigurationCompleter::Sync& completer)>;
  using SetBufferCollectionConstraintsChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineSetBufferCollectionConstraintsRequest* request,
      fdf::Arena& arena, SetBufferCollectionConstraintsCompleter::Sync& completer)>;
  using SetDisplayPowerModeChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineSetDisplayPowerModeRequest* request,
      fdf::Arena& arena, SetDisplayPowerModeCompleter::Sync& completer)>;
  using SetMinimumRgbChecker =
      fit::function<void(fuchsia_hardware_display_engine::wire::EngineSetMinimumRgbRequest* request,
                         fdf::Arena& arena, SetMinimumRgbCompleter::Sync& completer)>;
  using StartCaptureChecker =
      fit::function<void(fuchsia_hardware_display_engine::wire::EngineStartCaptureRequest* request,
                         fdf::Arena& arena, StartCaptureCompleter::Sync& completer)>;
  using ReleaseCaptureChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineReleaseCaptureRequest* request,
      fdf::Arena& arena, ReleaseCaptureCompleter::Sync& completer)>;

  MockEngineFidl();
  MockEngineFidl(const MockEngineFidl&) = delete;
  MockEngineFidl& operator=(const MockEngineFidl&) = delete;
  ~MockEngineFidl();

  // Expectations for fuchsia.hardware.display.engine/Engine:
  void ExpectCompleteCoordinatorConnection(CompleteCoordinatorConnectionChecker checker);
  void ExpectUnsetListener(UnsetListenerChecker checker);
  void ExpectImportBufferCollection(ImportBufferCollectionChecker checker);
  void ExpectReleaseBufferCollection(ReleaseBufferCollectionChecker checker);
  void ExpectImportImage(ImportImageChecker checker);
  void ExpectImportImageForCapture(ImportImageForCaptureChecker checker);
  void ExpectReleaseImage(ReleaseImageChecker checker);
  void ExpectCheckConfiguration(CheckConfigurationChecker checker);
  void ExpectSubmitConfiguration(SubmitConfigurationChecker checker);
  void ExpectSetBufferCollectionConstraints(SetBufferCollectionConstraintsChecker checker);
  void ExpectSetDisplayPowerMode(SetDisplayPowerModeChecker checker);
  void ExpectSetMinimumRgb(SetMinimumRgbChecker checker);
  void ExpectStartCapture(StartCaptureChecker checker);
  void ExpectReleaseCapture(ReleaseCaptureChecker checker);

  // Must be called at least once during an instance's lifetime.
  //
  // Tests are recommended to call this in a TearDown() method, or at the end of
  // the test case implementation.
  void CheckAllCallsReplayed();

  // `fdf::WireServer<fuchsia_hardware_display_engine::Engine>`:
  void CompleteCoordinatorConnection(
      fuchsia_hardware_display_engine::wire::EngineCompleteCoordinatorConnectionRequest* request,
      fdf::Arena& arena, CompleteCoordinatorConnectionCompleter::Sync& completer) override;
  void UnsetListener(fdf::Arena& arena, UnsetListenerCompleter::Sync& completer) override;
  void ImportBufferCollection(
      fuchsia_hardware_display_engine::wire::EngineImportBufferCollectionRequest* request,
      fdf::Arena& arena, ImportBufferCollectionCompleter::Sync& completer) override;
  void ReleaseBufferCollection(
      fuchsia_hardware_display_engine::wire::EngineReleaseBufferCollectionRequest* request,
      fdf::Arena& arena, ReleaseBufferCollectionCompleter::Sync& completer) override;
  void ImportImage(fuchsia_hardware_display_engine::wire::EngineImportImageRequest* request,
                   fdf::Arena& arena, ImportImageCompleter::Sync& completer) override;
  void ImportImageForCapture(
      fuchsia_hardware_display_engine::wire::EngineImportImageForCaptureRequest* request,
      fdf::Arena& arena, ImportImageForCaptureCompleter::Sync& completer) override;
  void ReleaseImage(fuchsia_hardware_display_engine::wire::EngineReleaseImageRequest* request,
                    fdf::Arena& arena, ReleaseImageCompleter::Sync& completer) override;
  void CheckConfiguration(
      fuchsia_hardware_display_engine::wire::EngineCheckConfigurationRequest* request,
      fdf::Arena& arena, CheckConfigurationCompleter::Sync& completer) override;
  void SubmitConfiguration(
      fuchsia_hardware_display_engine::wire::EngineSubmitConfigurationRequest* request,
      fdf::Arena& arena, SubmitConfigurationCompleter::Sync& completer) override;
  void SetBufferCollectionConstraints(
      fuchsia_hardware_display_engine::wire::EngineSetBufferCollectionConstraintsRequest* request,
      fdf::Arena& arena, SetBufferCollectionConstraintsCompleter::Sync& completer) override;
  void SetDisplayPowerMode(
      fuchsia_hardware_display_engine::wire::EngineSetDisplayPowerModeRequest* request,
      fdf::Arena& arena, SetDisplayPowerModeCompleter::Sync& completer) override;
  void SetMinimumRgb(fuchsia_hardware_display_engine::wire::EngineSetMinimumRgbRequest* request,
                     fdf::Arena& arena, SetMinimumRgbCompleter::Sync& completer) override;
  void StartCapture(fuchsia_hardware_display_engine::wire::EngineStartCaptureRequest* request,
                    fdf::Arena& arena, StartCaptureCompleter::Sync& completer) override;
  void ReleaseCapture(fuchsia_hardware_display_engine::wire::EngineReleaseCaptureRequest* request,
                      fdf::Arena& arena, ReleaseCaptureCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_display_engine::Engine> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  struct Expectation;

  std::mutex mutex_;
  std::vector<Expectation> expectations_ __TA_GUARDED(mutex_);
  size_t call_index_ __TA_GUARDED(mutex_) = 0;
  bool check_all_calls_replayed_called_ __TA_GUARDED(mutex_) = false;
};

}  // namespace display_coordinator::testing

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_ENGINE_FIDL_H_
