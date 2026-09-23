// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/a11y_registry.h"

#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

namespace scenic_impl::input {

A11yPointerEventRegistry::A11yPointerEventRegistry(async_dispatcher_t* input_dispatcher,
                                                   fit::function<void()> on_register,
                                                   fit::function<void()> on_disconnect)
    : input_dispatcher_(input_dispatcher),
      on_register_(std::move(on_register)),
      on_disconnect_(std::move(on_disconnect)) {
  FX_DCHECK(on_register_);
  FX_DCHECK(on_disconnect_);
}

void A11yPointerEventRegistry::Bind(
    fidl::ServerEnd<fuchsia_ui_input_accessibility::PointerEventRegistry> request) {
  utils::CheckIsOnInputThread();
  accessibility_pointer_event_registry_.AddBinding(input_dispatcher_, std::move(request), this,
                                                   fidl::kIgnoreBindingClosure);
}

bool A11yPointerEventRegistry::RegisterListener(
    fidl::ClientEnd<fuchsia_ui_input_accessibility::PointerEventListener> listener) {
  if (accessibility_pointer_event_listener_.is_valid()) {
    // An accessibility listener is already registered.
    return false;
  }
  accessibility_pointer_event_listener_.Bind(std::move(listener), input_dispatcher_, this);
  on_register_();
  return true;
}

void A11yPointerEventRegistry::Register(RegisterRequest& request,
                                        RegisterCompleter::Sync& completer) {
  completer.Reply(RegisterListener(std::move(request.pointer_event_listener())));
}

void A11yPointerEventRegistry::OnStreamHandled(
    fidl::Event<fuchsia_ui_input_accessibility::PointerEventListener::OnStreamHandled>& event) {
  if (on_stream_handled_) {
    on_stream_handled_(event.device_id(), event.pointer_id(), event.handled());
  }
}

void A11yPointerEventRegistry::on_fidl_error(fidl::UnbindInfo info) {
  // The bindings release every reference to the client's internals before
  // calling this hook, so replacing the client here is safe. Afterwards
  // is_valid() is false and RegisterListener() accepts a new listener.
  accessibility_pointer_event_listener_ = {};
  on_disconnect_();
}

}  // namespace scenic_impl::input
