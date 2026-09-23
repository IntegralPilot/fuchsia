// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UI_SCENIC_CPP_TESTING_FAKE_FOCUSER_H_
#define LIB_UI_SCENIC_CPP_TESTING_FAKE_FOCUSER_H_

#include <fuchsia/ui/views/cpp/fidl.h>
#include <fuchsia/ui/views/cpp/fidl_test_base.h>
#include <lib/syslog/cpp/macros.h>

#include <string>

using Focuser = fuchsia::ui::views::Focuser;

namespace scenic {

class FakeFocuser : public fuchsia::ui::views::testing::Focuser_TestBase {
 public:
  bool request_focus_called() const { return request_focus_called_; }

  void fail_request_focus(bool fail_request = true) { fail_request_focus_ = fail_request; }

 private:
  void RequestFocus(fuchsia::ui::views::ViewRef view_ref, RequestFocusCallback callback) override {
    request_focus_called_ = true;
    auto result = fail_request_focus_
                      ? fuchsia::ui::views::Focuser_RequestFocus_Result::WithErr(
                            fuchsia::ui::views::Error::DENIED)
                      : fuchsia::ui::views::Focuser_RequestFocus_Result::WithResponse(
                            fuchsia::ui::views::Focuser_RequestFocus_Response());
    callback(std::move(result));
  }

  void NotImplemented_(const std::string& name) override {
    FX_LOGS(ERROR) << "FakeFocuser does not implement " << name;
  }

  bool request_focus_called_ = false;
  bool fail_request_focus_ = false;
};

}  // namespace scenic

#endif  // LIB_UI_SCENIC_CPP_TESTING_FAKE_FOCUSER_H_
