// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/recognizers_v2/any_recognizer.h"

#include <fuchsia/ui/pointer/augment/cpp/fidl.h>

#include "src/ui/a11y/lib/gesture_manager/arena_v2/participation_token_interface.h"

namespace a11y::recognizers_v2 {

void AnyRecognizer::HandleEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {}

void AnyRecognizer::OnContestStarted(std::unique_ptr<ParticipationTokenInterface> token) {
  token->Accept();
}

std::string AnyRecognizer::DebugName() const { return "any"; }

}  // namespace a11y::recognizers_v2
