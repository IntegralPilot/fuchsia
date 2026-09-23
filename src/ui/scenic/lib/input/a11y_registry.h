// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_A11Y_REGISTRY_H_
#define SRC_UI_SCENIC_LIB_INPUT_A11Y_REGISTRY_H_

#include <fidl/fuchsia.ui.input.accessibility/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/component_context.h>

#include <utility>

namespace scenic_impl::input {

// Implementation of PointerEventRegistry API.
class A11yPointerEventRegistry
    : public fidl::Server<fuchsia_ui_input_accessibility::PointerEventRegistry>,
      public fidl::AsyncEventHandler<fuchsia_ui_input_accessibility::PointerEventListener> {
 public:
  using OnStreamHandledCallback =
      fit::function<void(uint32_t device_id, uint32_t pointer_id,
                         fuchsia_ui_input_accessibility::EventHandling handled)>;

  A11yPointerEventRegistry(async_dispatcher_t* input_dispatcher, fit::function<void()> on_register,
                           fit::function<void()> on_disconnect);

  void Bind(fidl::ServerEnd<fuchsia_ui_input_accessibility::PointerEventRegistry> request);

  // |fidl::Server<fuchsia_ui_input_accessibility::PointerEventRegistry>|
  void Register(RegisterRequest& request, RegisterCompleter::Sync& completer) override;

  bool RegisterListener(
      fidl::ClientEnd<fuchsia_ui_input_accessibility::PointerEventListener> listener);

  // Called for every OnStreamHandled event from the registered listener. A null
  // callback ignores the events.
  void set_on_stream_handled(OnStreamHandledCallback callback) {
    on_stream_handled_ = std::move(callback);
  }

  fidl::Client<fuchsia_ui_input_accessibility::PointerEventListener>&
  accessibility_pointer_event_listener() {
    return accessibility_pointer_event_listener_;
  }

 private:
  // |fidl::AsyncEventHandler<fuchsia_ui_input_accessibility::PointerEventListener>|
  void OnStreamHandled(
      fidl::Event<fuchsia_ui_input_accessibility::PointerEventListener::OnStreamHandled>& event)
      override;
  void on_fidl_error(fidl::UnbindInfo info) override;

  async_dispatcher_t* const input_dispatcher_;
  fidl::ServerBindingGroup<fuchsia_ui_input_accessibility::PointerEventRegistry>
      accessibility_pointer_event_registry_;
  // We honor the first accessibility listener to register. A call to Register()
  // above will fail if there is already a registered listener. The client is
  // reset from on_fidl_error() when the listener's channel closes, so that a
  // later listener can register.
  fidl::Client<fuchsia_ui_input_accessibility::PointerEventListener>
      accessibility_pointer_event_listener_;

  OnStreamHandledCallback on_stream_handled_;

  // Function called when a new listener successfully registers.
  fit::function<void()> on_register_;

  // Function called when an active listener disconnects.
  fit::function<void()> on_disconnect_;
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_A11Y_REGISTRY_H_
