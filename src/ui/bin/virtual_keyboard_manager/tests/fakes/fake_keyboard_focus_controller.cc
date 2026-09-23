// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/bin/virtual_keyboard_manager/tests/fakes/fake_keyboard_focus_controller.h"

#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>

namespace virtual_keyboard_manager {
namespace testing {

using fuchsia::ui::keyboard::focus::Controller;
using fuchsia::ui::views::ViewRef;

FakeKeyboardFocusController::FakeKeyboardFocusController() {}

FakeKeyboardFocusController::FakeKeyboardFocusController(
    sys::testing::ComponentContextProvider& context_provider) {
  context_provider.service_directory_provider()->AddService<Controller>(bindings_.GetHandler(this));
}

fidl::InterfaceRequestHandler<Controller> FakeKeyboardFocusController::GetHandler(
    async_dispatcher_t* dispatcher) {
  return [this, dispatcher](fidl::InterfaceRequest<Controller> request) {
    bindings_.AddBinding(this, std::move(request), dispatcher);
  };
}

void FakeKeyboardFocusController::SetOnNotify(
    std::function<void(const ViewRef& view_ref)> callback) {
  on_notify_callback_ = callback;
}

void FakeKeyboardFocusController::Notify(ViewRef view_ref, NotifyCallback callback) {
  num_calls_++;
  if (on_notify_callback_) {
    on_notify_callback_(view_ref);
  }
  if (defer_notify_response_) {
    deferred_callback_ = std::move(callback);
  } else {
    callback();
  }
}

}  // namespace testing
}  // namespace virtual_keyboard_manager
