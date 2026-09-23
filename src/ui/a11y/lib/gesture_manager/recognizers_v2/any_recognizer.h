// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_ANY_RECOGNIZER_H_
#define SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_ANY_RECOGNIZER_H_

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>

#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"
#include "src/ui/a11y/lib/gesture_manager/arena_v2/recognizer_v2.h"

namespace a11y::recognizers_v2 {

// Recognizer that accepts any gesture. This can be used as a catch-all to make a gesture arena
// consume any gesture not handled by another recognizer.
class AnyRecognizer : public GestureRecognizerV2 {
 public:
  void HandleEvent(const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) override;
  void OnContestStarted(std::unique_ptr<ParticipationTokenInterface> token) override;
  std::string DebugName() const override;
};

}  // namespace a11y::recognizers_v2

#endif  // SRC_UI_A11Y_LIB_GESTURE_MANAGER_RECOGNIZERS_V2_ANY_RECOGNIZER_H_
