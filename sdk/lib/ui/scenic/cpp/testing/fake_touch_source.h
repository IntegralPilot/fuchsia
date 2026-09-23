// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UI_SCENIC_CPP_TESTING_FAKE_TOUCH_SOURCE_H_
#define LIB_UI_SCENIC_CPP_TESTING_FAKE_TOUCH_SOURCE_H_

#include <fuchsia/ui/pointer/cpp/fidl.h>
#include <zircon/assert.h>

#include <optional>
#include <vector>

namespace scenic {

// A test stub to act as the protocol server. A test can control what is sent
// back by this server implementation, via the ScheduleCallback call.
class FakeTouchSource : public fuchsia::ui::pointer::TouchSource {
 public:
  // |fuchsia.ui.pointer.TouchSource|
  void Watch(std::vector<fuchsia::ui::pointer::TouchResponse> responses,
             TouchSource::WatchCallback callback) override {
    responses_ = std::move(responses);
    callback_ = std::move(callback);
  }

  // Have the server issue events to the client's hanging-get Watch call.
  void ScheduleCallback(std::vector<fuchsia::ui::pointer::TouchEvent> events) {
    ZX_ASSERT_MSG(callback_, "FakeTouchSource::ScheduleCallback require a valid WatchCallback");
    callback_(std::move(events));
  }

  // Allow the test to observe what the client uploaded on the next Watch call.
  std::optional<std::vector<fuchsia::ui::pointer::TouchResponse>> UploadedResponses() {
    auto responses = std::move(responses_);
    responses_.reset();
    return responses;
  }

 private:
  // |fuchsia.ui.pointer.TouchSource|
  void UpdateResponse(fuchsia::ui::pointer::TouchInteractionId ixn,
                      fuchsia::ui::pointer::TouchResponse response,
                      TouchSource::UpdateResponseCallback callback) override {}

  // Client uploads responses to server.
  std::optional<std::vector<fuchsia::ui::pointer::TouchResponse>> responses_;

  // Client-side logic to invoke on Watch() call's return. A test triggers it
  // with ScheduleCallback().
  TouchSource::WatchCallback callback_;
};

}  // namespace scenic

#endif  // LIB_UI_SCENIC_CPP_TESTING_FAKE_TOUCH_SOURCE_H_
