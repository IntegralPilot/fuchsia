// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "timeline_function.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/assert.h>

#include <limits>
#include <utility>

namespace media {

// static
// Translates a given reference value through a provided timeline function, producing a
// corresponding subject value. Returns kOverflow if result can't fit in an int64_t.
int64_t TimelineFunction::Apply(int64_t subject_time, int64_t reference_time, TimelineRate rate,
                                int64_t reference_input) {
  // Round down (toward negative infinity) when scaling. This preserves scaled distances between
  // positive and negative points on the timeline.
  // For example, suppose we call this twice:
  //
  //   1. reference_input - reference_time = 20, ratio = 1/8, scaled_value = 2.5
  //   2. reference_input - reference_time = -20, ratio = 1/8, scaled_value = -2.5
  //
  // If we truncate (round toward zero), the scaled values are 2 and -2, for a difference of 4,
  // while the true scaled difference should be 40*1/8 = 5. If we round down, the scaled values are
  // 2 and -3, for a (correct) difference of 5.
  //
  // Perform subtraction in 128-bit space to prevent signed integer overflow undefined behavior when
  // handling distant timestamps (e.g. INT64_MAX - (-10)).
  __int128_t reference_delta = static_cast<__int128_t>(reference_input) - reference_time;
  int64_t scaled_value = rate.Scale128(reference_delta, TimelineRate::RoundingMode::Floor);
  if (scaled_value == TimelineRate::kOverflow || scaled_value == TimelineRate::kUnderflow) {
    return scaled_value;
  }

  // Perform addition in 128-bit space and explicitly check bounds before casting down to int64_t.
  // Doing 64-bit addition first would trigger C++ UB on overflow, allowing compiler optimizations
  // to elide post-addition bounds checks.
  __int128_t result_value_128 = static_cast<__int128_t>(scaled_value) + subject_time;
  if (result_value_128 > TimelineRate::kOverflow) {
    return TimelineRate::kOverflow;
  }
  if (result_value_128 < TimelineRate::kUnderflow) {
    return TimelineRate::kUnderflow;
  }

  return static_cast<int64_t>(result_value_128);
}

// static
// Combine two given timeline functions, forming a new one. ASSERT upon overflow.
TimelineFunction TimelineFunction::Compose(const TimelineFunction& bc, const TimelineFunction& ab,
                                           bool exact) {
  // This composition approach may compromise range and accuracy (in some cases) for simplicity.
  // TODO(https://fxbug.dev/42082948): more accuracy here
  auto scaled_subject_time = bc.Apply(ab.subject_time());
  if (exact) {
    ZX_ASSERT(scaled_subject_time != TimelineRate::kOverflow);
    ZX_ASSERT(scaled_subject_time != TimelineRate::kUnderflow);
  }

  return TimelineFunction(scaled_subject_time, ab.reference_time(),
                          TimelineRate::Product(ab.rate(), bc.rate(), exact));
}

}  // namespace media
