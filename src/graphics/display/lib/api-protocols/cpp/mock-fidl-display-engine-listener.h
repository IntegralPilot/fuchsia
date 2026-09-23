// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_MOCK_FIDL_DISPLAY_ENGINE_LISTENER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_MOCK_FIDL_DISPLAY_ENGINE_LISTENER_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fit/function.h>
#include <zircon/compiler.h>

#include <cstddef>
#include <mutex>
#include <vector>

namespace display::testing {

// Strict mock for the FIDL-generated DisplayEngineListener protocol.
//
// This is a very rare case where strict mocking is warranted. The code under
// test is an adapter that maps C++ calls 1:1 to FIDL calls. So, the API
// contract being tested is expressed in terms of individual function calls.
class MockFidlDisplayEngineListener
    : public fdf::WireServer<fuchsia_hardware_display_engine::EngineListener> {
 public:
  // Expectation containers for fuchsia.hardware.display.controller/DisplayEngineListener:
  using OnDisplayAddedChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayAddedRequest* request,
      fdf::Arena& arena, OnDisplayAddedCompleter::Sync& completer)>;
  using OnDisplayRemovedChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayRemovedRequest* request,
      fdf::Arena& arena, OnDisplayRemovedCompleter::Sync& completer)>;
  using OnDisplayVsyncChecker = fit::function<void(
      fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayVsyncRequest* request,
      fdf::Arena& arena, OnDisplayVsyncCompleter::Sync& completer)>;
  using OnCaptureCompleteChecker =
      fit::function<void(fdf::Arena& arena, OnCaptureCompleteCompleter::Sync& completer)>;

  MockFidlDisplayEngineListener();
  MockFidlDisplayEngineListener(const MockFidlDisplayEngineListener&) = delete;
  MockFidlDisplayEngineListener& operator=(const MockFidlDisplayEngineListener&) = delete;
  ~MockFidlDisplayEngineListener();

  // Expectations for fuchsia.hardware.display.controller/DisplayEngineListener:
  void ExpectOnDisplayAdded(OnDisplayAddedChecker checker);
  void ExpectOnDisplayRemoved(OnDisplayRemovedChecker checker);
  void ExpectOnDisplayVsync(OnDisplayVsyncChecker checker);
  void ExpectOnCaptureComplete(OnCaptureCompleteChecker checker);

  // Must be called at least once during an instance's lifetime.
  //
  // Tests are recommended to call this in a TearDown() method, or at the end of
  // the test case implementation.
  void CheckAllCallsReplayed();

  fidl::ProtocolHandler<fuchsia_hardware_display_engine::EngineListener> CreateHandler(
      fdf_dispatcher_t& dispatcher);

  // fidl::WireServer<fuchsia_hardware_display_engine::EngineListener>:
  void OnDisplayAdded(
      fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayAddedRequest* request,
      fdf::Arena& arena, OnDisplayAddedCompleter::Sync& completer) override;
  void OnDisplayRemoved(
      fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayRemovedRequest* request,
      fdf::Arena& arena, OnDisplayRemovedCompleter::Sync& completer) override;
  void OnDisplayVsync(
      fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayVsyncRequest* request,
      fdf::Arena& arena, OnDisplayVsyncCompleter::Sync& completer) override;
  void OnCaptureComplete(fdf::Arena& arena, OnCaptureCompleteCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_display_engine::EngineListener> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  struct Expectation;

  std::mutex mutex_;
  std::vector<Expectation> expectations_ __TA_GUARDED(mutex_);
  size_t call_index_ __TA_GUARDED(mutex_) = 0;
  bool check_all_calls_replayed_called_ __TA_GUARDED(mutex_) = false;

  fdf::ServerBindingGroup<fuchsia_hardware_display_engine::EngineListener> bindings_;
};

}  // namespace display::testing

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_MOCK_FIDL_DISPLAY_ENGINE_LISTENER_H_
