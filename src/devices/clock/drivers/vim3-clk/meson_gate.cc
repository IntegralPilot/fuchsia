// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "meson_gate.h"

#include <zircon/assert.h>

namespace vim3_clock {

void MesonGate::Enable() {
  vote_count_++;

  if (vote_count_ == 1) {
    EnableHw();
  }
}

void MesonGate::Disable() {
  // It's an error to disable a clock that's never been enabled.
  ZX_ASSERT_MSG(vote_count_ > 0, "Attempted to Disable a clock that's never been enabled, id = %u",
                id_);

  vote_count_--;

  if (vote_count_ == 0) {
    DisableHw();
  }
}

void MesonGate::EnableHw() { mmio_.SetBits32(mask_, offset_); }

void MesonGate::DisableHw() { mmio_.ClearBits32(mask_, offset_); }

}  // namespace vim3_clock
