// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UI_SCENIC_CPP_TESTING_FAKE_VIEW_REF_FOCUSED_H_
#define LIB_UI_SCENIC_CPP_TESTING_FAKE_VIEW_REF_FOCUSED_H_

#include <fuchsia/ui/views/cpp/fidl.h>

#include <cstddef>

using ViewRefFocused = fuchsia::ui::views::ViewRefFocused;

namespace scenic {

class FakeViewRefFocused : public ViewRefFocused {
 public:
  using WatchCallback = ViewRefFocused::WatchCallback;

  void Watch(WatchCallback callback) override {
    callback_ = std::move(callback);
    ++times_watched_;
  }

  void ScheduleCallback(bool focused) {
    fuchsia::ui::views::FocusState focus_state;
    focus_state.set_focused(focused);
    callback_(std::move(focus_state));
  }

  size_t times_watched() const { return times_watched_; }

 private:
  size_t times_watched_ = 0;
  WatchCallback callback_;
};

}  // namespace scenic

#endif  // LIB_UI_SCENIC_CPP_TESTING_FAKE_VIEW_REF_FOCUSED_H_
