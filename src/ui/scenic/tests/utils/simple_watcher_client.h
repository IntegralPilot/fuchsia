// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_TESTS_UTILS_SIMPLE_WATCHER_CLIENT_H_
#define SRC_UI_SCENIC_TESTS_UTILS_SIMPLE_WATCHER_CLIENT_H_

#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>

#include <utility>

#include <zxtest/zxtest.h>

namespace integration_tests {

// Invoked when a client's channel closes, for any reason.
using OnCloseHandler = fit::function<void(fidl::UnbindInfo)>;

// Returns an OnCloseHandler that fails the current test. For clients whose
// channel is never expected to close.
inline OnCloseHandler FailOnClose(const char* what) {
  return [what](fidl::UnbindInfo info) {
    FX_LOGS(ERROR) << what << ": " << info.FormatDescription();
    FAIL();
  };
}

// Bundles a fidl::Client<Protocol> with a minimal fidl::AsyncEventHandler that
// tracks whether the channel is still open. Not usable with open protocols,
// which require a handle_unknown_event override.
template <typename Protocol>
class SimpleWatcherClient : public fidl::AsyncEventHandler<Protocol> {
 public:
  // Without |on_close|, a closure is only logged.
  SimpleWatcherClient(fidl::ClientEnd<Protocol> client_end, async_dispatcher_t* dispatcher,
                      OnCloseHandler on_close = nullptr)
      : on_close_(std::move(on_close)), client_(std::move(client_end), dispatcher, this) {}

  void on_fidl_error(fidl::UnbindInfo info) override {
    is_bound_ = false;
    if (on_close_) {
      on_close_(info);
    } else {
      FX_LOGS(WARNING) << "Connection closed: " << info.FormatDescription();
    }
  }

  bool is_bound() const { return is_bound_ && client_.is_valid(); }
  void Unbind() {
    client_ = {};
    is_bound_ = false;
  }
  fidl::Client<Protocol>& operator->() { return client_; }
  fidl::Client<Protocol>& client() { return client_; }

 private:
  bool is_bound_ = true;
  OnCloseHandler on_close_;
  // MUST be destructed first, therefore it is the last field.
  fidl::Client<Protocol> client_;
};

}  // namespace integration_tests

#endif  // SRC_UI_SCENIC_TESTS_UTILS_SIMPLE_WATCHER_CLIENT_H_
