// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/tests/mocks/mock_participation_token.h"

#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

namespace accessibility_test {

class MockParticipationTokenHandle::Token : public a11y::ParticipationTokenInterface {
 public:
  explicit Token(MockParticipationTokenHandle* handle) : handle_(handle) {
    handle_->taken_ = true;

    handle_->held_ = true;
    handle_->status_ = Status::kUndecided;
  }

  ~Token() override {
    handle_->taken_ = false;

    handle_->held_ = false;
    if (handle_->status_ == Status::kUndecided) {
      handle_->status_ = Status::kReject;
    }
  }

  void Accept() override {
    EXPECT_NE(handle_->status_, Status::kReject);

    handle_->status_ = Status::kAccept;
  }

  void Reject() override {
    EXPECT_NE(handle_->status_, Status::kAccept);

    handle_->held_ = false;
    handle_->status_ = Status::kReject;
  }

 private:
  MockParticipationTokenHandle* const handle_;
};

std::unique_ptr<a11y::ParticipationTokenInterface> MockParticipationTokenHandle::TakeToken() {
  FX_CHECK(!taken_);

  return std::make_unique<Token>(this);
}

}  // namespace accessibility_test
