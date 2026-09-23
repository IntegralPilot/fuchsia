// Copyright 2015 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_BIN_VIRTUAL_KEYBOARD_MANAGER_APP_H_
#define SRC_UI_BIN_VIRTUAL_KEYBOARD_MANAGER_APP_H_

#include <fuchsia/input/virtualkeyboard/cpp/fidl.h>
#include <fuchsia/ui/views/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/cpp/component_context.h>

#include <limits>
#include <memory>
#include <unordered_map>
#include <vector>

#include "src/lib/fxl/macros.h"
#include "src/ui/bin/virtual_keyboard_manager/focus_dispatcher.h"
#include "src/ui/bin/virtual_keyboard_manager/virtual_keyboard_coordinator.h"
#include "src/ui/bin/virtual_keyboard_manager/virtual_keyboard_manager.h"

namespace virtual_keyboard_manager {

// Class for serving various input and graphics related APIs.
class App {
 public:
  App(sys::ComponentContext* component_context, fit::closure quit_callback);
  ~App() = default;

 private:
  // Exits the loop, terminating the RootPresenter process.
  void Exit() { quit_callback_(); }

  const fit::closure quit_callback_;

  // Coordinates virtual keyboard state changes between
  // `fuchsia.input.virtualkeyboard.Controller`s and the
  // `fuchsia.input.virtualkeyboard.Manager`.
  FidlBoundVirtualKeyboardCoordinator virtual_keyboard_coordinator_;

  // Used to dispatch the focus change messages to interested downstream clients.
  FocusDispatcher focus_dispatcher_;

  FXL_DISALLOW_COPY_AND_ASSIGN(App);
};

}  // namespace virtual_keyboard_manager

#endif  // SRC_UI_BIN_VIRTUAL_KEYBOARD_MANAGER_APP_H_
