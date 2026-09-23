// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_TESTS_UTILS_FLATLAND_CLIENT_WITH_EVENT_HANDLER_H_
#define SRC_UI_SCENIC_TESTS_UTILS_FLATLAND_CLIENT_WITH_EVENT_HANDLER_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/tests/utils/simple_watcher_client.h"

namespace integration_tests {

// Bundles a fidl::Client<Flatland> together with a fidl::AsyncEventHandler<Flatland>.
// Each of the different event types is handled by a separate, settable closure.
class FlatlandClientWithEventHandler
    : protected fidl::AsyncEventHandler<fuchsia_ui_composition::Flatland> {
 public:
  using OnFramePresentedEvent = fidl::Event<fuchsia_ui_composition::Flatland::OnFramePresented>;
  using OnFramePresentedHandler = fit::function<void(OnFramePresentedEvent&)>;
  using OnNextFrameBeginEvent = fidl::Event<fuchsia_ui_composition::Flatland::OnNextFrameBegin>;
  using OnNextFrameBeginHandler = fit::function<void(OnNextFrameBeginEvent&)>;
  using OnErrorEvent = fidl::Event<fuchsia_ui_composition::Flatland::OnError>;
  using OnErrorHandler = fit::function<void(OnErrorEvent&)>;

  // Not moveable, not copyable.
  FlatlandClientWithEventHandler(const FlatlandClientWithEventHandler& other) = delete;
  FlatlandClientWithEventHandler(FlatlandClientWithEventHandler&& other) = delete;
  FlatlandClientWithEventHandler& operator=(const FlatlandClientWithEventHandler& other) = delete;
  FlatlandClientWithEventHandler& operator=(FlatlandClientWithEventHandler&& other) = delete;

  FlatlandClientWithEventHandler(fidl::ClientEnd<fuchsia_ui_composition::Flatland> client_end,
                                 async_dispatcher_t* dispatcher)
      : dispatcher_(dispatcher),
        flatland_(std::move(client_end), dispatcher,
                  static_cast<fidl::AsyncEventHandler<fuchsia_ui_composition::Flatland>*>(this)) {
    FX_CHECK(dispatcher_);
  }

  ~FlatlandClientWithEventHandler() = default;

  // Allow conveniently calling though to Flatland methods.
  fidl::Client<fuchsia_ui_composition::Flatland>& operator->() { return flatland_; }
  fidl::Client<fuchsia_ui_composition::Flatland>& client() { return flatland_; }
  const fidl::Client<fuchsia_ui_composition::Flatland>& client() const { return flatland_; }
  bool is_valid() const { return flatland_.is_valid(); }
  bool is_bound() const { return is_bound_ && flatland_.is_valid(); }

  // Configure handling of Flatland::OnFramePresented event.
  void set_on_frame_presented(OnFramePresentedHandler handler) {
    // This assertion enforces the rule: you cannot set a handler if one is already set.
    FX_CHECK(!on_frame_presented_.has_value()) << "OnFramePresented handler is already set.";
    on_frame_presented_ = std::move(handler);
  }
  void reset_on_frame_presented() { on_frame_presented_.reset(); }

  // Configure handling of Flatland::OnNextFrameBegin event.
  void set_on_next_frame_begin(OnNextFrameBeginHandler handler) {
    FX_CHECK(!on_next_frame_begin_.has_value()) << "OnNextFrameBegin handler is already set.";
    on_next_frame_begin_ = std::move(handler);
  }
  void reset_on_next_frame_begin() { on_next_frame_begin_.reset(); }

  // Configure handling of Flatland::OnError event.
  void set_on_error(OnErrorHandler handler) {
    FX_CHECK(!on_error_.has_value()) << "OnError handler is already set.";
    on_error_ = std::move(handler);
  }
  void reset_on_error() { on_error_.reset(); }

  // Configure handling of channel closure, for any reason. Without a handler, a closure is only
  // logged.
  void set_on_close(OnCloseHandler handler) {
    FX_CHECK(!on_close_) << "on_close handler is already set.";
    on_close_ = std::move(handler);
  }

 protected:
  void on_fidl_error(fidl::UnbindInfo info) override {
    is_bound_ = false;
    if (on_close_) {
      on_close_(info);
    } else {
      FX_LOGS(WARNING) << "Flatland connection closed: " << info.FormatDescription();
    }
  }

  // fidl::AsyncEventHandler<fuchsia_ui_composition::Flatland>
  void OnFramePresented(OnFramePresentedEvent& event) override {
    if (on_frame_presented_) {
      (*on_frame_presented_)(event);
    }
  }

  // fidl::AsyncEventHandler<fuchsia_ui_composition::Flatland>
  void OnNextFrameBegin(OnNextFrameBeginEvent& event) override {
    if (on_next_frame_begin_) {
      (*on_next_frame_begin_)(event);
    }
  }

  // fidl::AsyncEventHandler<fuchsia_ui_composition::Flatland>
  void OnError(OnErrorEvent& event) override {
    if (on_error_) {
      (*on_error_)(event);
    }
  }

 private:
  async_dispatcher_t* dispatcher_;
  bool is_bound_ = true;

  std::optional<OnFramePresentedHandler> on_frame_presented_;
  std::optional<OnNextFrameBeginHandler> on_next_frame_begin_;
  std::optional<OnErrorHandler> on_error_;
  OnCloseHandler on_close_;

  // MUST be destructed first, therefore it is the last field.
  fidl::Client<fuchsia_ui_composition::Flatland> flatland_;
};

}  // namespace integration_tests

#endif  // SRC_UI_SCENIC_TESTS_UTILS_FLATLAND_CLIENT_WITH_EVENT_HANDLER_H_
