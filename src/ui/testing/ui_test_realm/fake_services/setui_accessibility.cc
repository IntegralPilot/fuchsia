// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/settings/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/cpp/component_context.h>
#include <stdlib.h>

#include <iostream>
#include <utility>

class SetUIAccessibility : public fuchsia::settings::Accessibility {
 public:
  SetUIAccessibility() = default;

  fidl::InterfaceRequestHandler<fuchsia::settings::Accessibility> GetHandler() {
    return bindings_.GetHandler(this);
  }

  // |fuchsia.settings.Accessibility|
  void Watch(WatchCallback callback) override {
    // Watch() will response immediately for the first call, otherwise Watch()
    // will hang until AccessibilitySettings is updated. Because this fake
    // setui service does not support Set(), just not call the callback.
    if (!is_first_watch_call_) {
      return;
    }

    is_first_watch_call_ = false;

    // just return default settings.
    fuchsia::settings::AccessibilitySettings settings;
    callback(std::move(settings));
  }

  // |fuchsia.settings.Accessibility|
  void Set(fuchsia::settings::AccessibilitySettings settings, SetCallback callback) override {
    // Do nothing
  }

 private:
  bool is_first_watch_call_ = true;
  fidl::BindingSet<fuchsia::settings::Accessibility> bindings_;
};

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();

  auto set_ui_accessibility = std::make_unique<SetUIAccessibility>();
  context->outgoing()->AddPublicService(set_ui_accessibility->GetHandler());

  loop.Run();
  return EXIT_SUCCESS;
}
