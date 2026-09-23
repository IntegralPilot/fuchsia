// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/mock-fidl-display-engine-listener.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <zircon/assert.h>

#include <mutex>
#include <utility>

namespace display::testing {

// Exactly one of the members is non-null.
struct MockFidlDisplayEngineListener::Expectation {
  OnDisplayAddedChecker on_display_added_checker;
  OnDisplayRemovedChecker on_display_removed_checker;
  OnDisplayVsyncChecker on_display_vsync_checker;
  OnCaptureCompleteChecker on_capture_complete_checker;
};

MockFidlDisplayEngineListener::MockFidlDisplayEngineListener() = default;

MockFidlDisplayEngineListener::~MockFidlDisplayEngineListener() {
  ZX_ASSERT_MSG(check_all_calls_replayed_called_, "CheckAllCallsReplayed() not called on a mock");
}

void MockFidlDisplayEngineListener::ExpectOnDisplayAdded(OnDisplayAddedChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.on_display_added_checker = std::move(checker)});
}

void MockFidlDisplayEngineListener::ExpectOnDisplayRemoved(OnDisplayRemovedChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back(Expectation{.on_display_removed_checker = std::move(checker)});
}

void MockFidlDisplayEngineListener::ExpectOnDisplayVsync(OnDisplayVsyncChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back(Expectation{.on_display_vsync_checker = std::move(checker)});
}

void MockFidlDisplayEngineListener::ExpectOnCaptureComplete(OnCaptureCompleteChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back(Expectation{.on_capture_complete_checker = std::move(checker)});
}

void MockFidlDisplayEngineListener::CheckAllCallsReplayed() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(expectations_.size() == call_index_, "%zu expected calls were not received",
                expectations_.size() - call_index_);
  check_all_calls_replayed_called_ = true;
}

fidl::ProtocolHandler<fuchsia_hardware_display_engine::EngineListener>
MockFidlDisplayEngineListener::CreateHandler(fdf_dispatcher_t& dispatcher) {
  return bindings_.CreateHandler(this, &dispatcher, fidl::kIgnoreBindingClosure);
}

void MockFidlDisplayEngineListener::OnDisplayAdded(
    fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayAddedRequest* request,
    fdf::Arena& arena, OnDisplayAddedCompleter::Sync& completer) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.on_display_added_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.on_display_added_checker(request, arena, completer);
}

void MockFidlDisplayEngineListener::OnDisplayRemoved(
    fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayRemovedRequest* request,
    fdf::Arena& arena, OnDisplayRemovedCompleter::Sync& completer) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.on_display_removed_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.on_display_removed_checker(request, arena, completer);
}

void MockFidlDisplayEngineListener::OnDisplayVsync(
    fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayVsyncRequest* request,
    fdf::Arena& arena, OnDisplayVsyncCompleter::Sync& completer) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.on_display_vsync_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.on_display_vsync_checker(request, arena, completer);
}

void MockFidlDisplayEngineListener::OnCaptureComplete(fdf::Arena& arena,
                                                      OnCaptureCompleteCompleter::Sync& completer) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.on_capture_complete_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.on_capture_complete_checker(arena, completer);
}

void MockFidlDisplayEngineListener::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_display_engine::EngineListener> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ZX_PANIC("Received unknown FIDL method call");
}

}  // namespace display::testing
