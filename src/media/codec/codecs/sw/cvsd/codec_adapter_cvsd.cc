// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "codec_adapter_cvsd.h"

#include <lib/async/cpp/task.h>

bool AreJbitsEqual(const CvsdParams& params) {
  uint32_t lastKBits = params.historic_bits;
  for (uint32_t i = 0; i < params.k - params.j + 1; i++) {
    auto tmp = lastKBits & params.equal_bit_mask;
    if (tmp == 0 || tmp == params.equal_bit_mask) {
      return true;
    }
    lastKBits = lastKBits >> 1;
  }
  return false;
}

// Calculates the new step size based on Bluetooth Core v5.3 section
// 9.2 CVSD CODEC, equations 16 and 17.
double GetNewStepSize(const CvsdParams& params) {
  // EQ 16 and EQ 17.
  // If J bits in the last K output bits are equal, step size is incremented
  // otherwise, it's decremented.
  double new_delta;
  if (AreJbitsEqual(params)) {
    new_delta =
        std::min(params.step_size + kStepDecaySizeMin, static_cast<double>(kStepDecaySizeMax));
  } else {
    new_delta = static_cast<int16_t>(
        std::max(params.step_size * kStepDecayNumerator / kStepDecayDenominator,
                 static_cast<double>(kStepDecaySizeMin)));
  }

  ZX_DEBUG_ASSERT(new_delta <= std::numeric_limits<int16_t>::max());
  ZX_DEBUG_ASSERT(new_delta >= std::numeric_limits<int16_t>::min());

  return new_delta;
}

// Calculates the new accumulator value based on Bluetooth Core v5.3 section
// 9.2 CVSD CODEC equations.
double GetNewAccumulator(const CvsdParams& params, const uint8_t& bit) {
  double new_accum = params.accumulator;
  new_accum += (bit) ? -params.step_size : params.step_size;

  // EQ 18. Limit the internal reference.
  if (new_accum >= 0) {
    new_accum = std::min(new_accum, static_cast<double>(kAccumulatorPosSaturation));
  } else {
    new_accum = std::max(new_accum, static_cast<double>(kAccumulatorNegSaturation));
  }
  // EQ 14. Multiply by the decay factor.
  new_accum = new_accum * kAccumDecayNumerator / kAccumDecayDenominator;

  ZX_DEBUG_ASSERT(new_accum <= std::numeric_limits<int16_t>::max());
  ZX_DEBUG_ASSERT(new_accum >= std::numeric_limits<int16_t>::min());

  return new_accum;
}

void UpdateCvsdParams(CvsdParams* params, uint8_t bit) {
  // Shift last value into historic bits buffer.
  params->historic_bits = ((params->historic_bits << 1) | bit) & params->historic_bit_mask;

  // Update the step size.
  params->step_size = GetNewStepSize(*params);

  // Update the internal accumulator.
  params->accumulator = GetNewAccumulator(*params, bit);
}

void InitCvsdParams(std::optional<CvsdParams>& codec_params) {
  // Number of equal bits (run of 1's or 0's), J, should be less than or equal to
  // the number of historic output bits we keep track of.
  codec_params.emplace(CvsdParams{
      .k = kK,
      .j = kJ,
      .equal_bit_mask = static_cast<uint32_t>(pow(2, kJ) - 1),
      .historic_bit_mask = static_cast<uint32_t>(pow(2, kK) - 1),
      .historic_bits = 0,
      .accumulator = 0,
      .step_size = 0,
  });
}
