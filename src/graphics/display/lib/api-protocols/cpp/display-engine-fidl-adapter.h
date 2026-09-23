// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_FIDL_ADAPTER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_FIDL_ADAPTER_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <lib/fdf/dispatcher.h>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-interface.h"

namespace display {

// Translates FIDL API calls to `DisplayEngineInterface` C++ method calls.
//
// This adapter implements the [`fuchsia.hardware.display.engine/Engine`] FIDL API.
class DisplayEngineFidlAdapter : public fdf::WireServer<fuchsia_hardware_display_engine::Engine> {
 public:
  // `engine` receives translated FIDL calls to the
  // [`fuchsia.hardware.display.engine/Engine`] interface. It must not be null, and
  // must outlive the newly created instance.
  //
  // `engine_events` is notified when a
  // [`fuchsia.hardware.display.engine/EngineListener`] FIDL interface
  // implementation is registered with the display engine. It must not be null,
  // and must outlive the newly created instance.
  explicit DisplayEngineFidlAdapter(DisplayEngineInterface* engine,
                                    DisplayEngineEventsFidl* engine_events);

  DisplayEngineFidlAdapter(const DisplayEngineFidlAdapter&) = delete;
  DisplayEngineFidlAdapter& operator=(const DisplayEngineFidlAdapter&) = delete;

  ~DisplayEngineFidlAdapter() override;

  fidl::ProtocolHandler<fuchsia_hardware_display_engine::Engine> CreateHandler(
      fdf_dispatcher_t& dispatcher);

  // fdf::WireServer<fuchsia_hardware_display_engine::Engine>:
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
  DisplayEngineInterface& engine_;
  DisplayEngineEventsFidl& engine_events_;

  fdf::ServerBindingGroup<fuchsia_hardware_display_engine::Engine> bindings_;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_DISPLAY_ENGINE_FIDL_ADAPTER_H_
