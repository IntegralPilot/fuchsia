// Copyright 2015 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/bin/virtual_keyboard_manager/app.h"

#include <fuchsia/ui/keyboard/focus/cpp/fidl.h>
#include <lib/fidl/cpp/clone.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/event.h>
#include <zircon/status.h>

#include <algorithm>
#include <cstdlib>
#include <string>

namespace virtual_keyboard_manager {

App::App(sys::ComponentContext* component_context, fit::closure quit_callback)
    : quit_callback_(std::move(quit_callback)),
      virtual_keyboard_coordinator_(component_context),
      focus_dispatcher_(component_context->svc(), virtual_keyboard_coordinator_.GetWeakPtr()) {
  FX_DCHECK(component_context);
}

}  // namespace virtual_keyboard_manager
