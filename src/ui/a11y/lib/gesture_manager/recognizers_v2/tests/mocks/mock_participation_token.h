// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_TESTS_MOCKS_MOCK_PARTICIPATION_TOKEN_H_
#define SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_TESTS_MOCKS_MOCK_PARTICIPATION_TOKEN_H_

#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"

namespace accessibility_test {

class MockParticipationTokenHandle {
 public:
  enum class Status {
    kUndecided,
    kAccept,
    kReject,
  };

  std::unique_ptr<a11y::ParticipationTokenInterface> TakeToken();

  bool is_held() const { return held_; }
  Status status() const { return status_; }

 private:
  class Token;

  bool taken_ = false;

  bool held_ = false;
  Status status_ = Status::kUndecided;
};

}  // namespace accessibility_test

#endif  // SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_TESTS_MOCKS_MOCK_PARTICIPATION_TOKEN_H_
