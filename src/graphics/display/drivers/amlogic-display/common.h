// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_COMMON_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_COMMON_H_

#include <hwreg/mmio.h>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"

// Sets the bit field [field_begin_bit .. field_begin_bit + field_size_bits - 1]
// of the `reg_value` to the 32-bit `field_value`.
//
// The arguments must satisfy the following constraints:
// - `field_begin_bit` must be >= 0 and <= 31.
// - `field_size_bits` must be > 0.
// - `field_begin_bit + field_size_bits - 1` must be <= 31.
// - `field_value` must be a valid `field_size_bits`-bit number, i.e. it must be
//   >= 0 and <= 2 ^ `field_size_bits` - 1.
//
// TODO(https://fxbug.dev/42081461): Replace direct register reads / writes using the
// register helper functions with register classes (such as
// `hwreg::RegisterBase`).
constexpr uint32_t SetFieldValue32(uint32_t reg_value, int field_begin_bit, int field_size_bits,
                                   uint32_t field_value) {
  ZX_DEBUG_ASSERT(field_begin_bit >= 0);
  ZX_DEBUG_ASSERT(field_begin_bit <= 31);
  ZX_DEBUG_ASSERT(field_size_bits > 0);
  ZX_DEBUG_ASSERT(field_begin_bit + field_size_bits - 1 <= 31);
  const uint32_t field_mask_unshifted =
      std::numeric_limits<uint32_t>::max() >> (32 - field_size_bits);
  ZX_DEBUG_ASSERT(field_value <= field_mask_unshifted);
  const uint32_t field_mask_shifted = field_mask_unshifted << field_begin_bit;
  const uint32_t reg_value_with_field_cleared = reg_value & ~field_mask_shifted;
  return reg_value_with_field_cleared | (field_value << field_begin_bit);
}

// Gets the value of the bit field [field_begin_bit .. field_begin_bit +
// field_size_bits - 1] from the 32-bit `reg_value`.
//
// The arguments must satisfy the following constraints:
// - `field_begin_bit` must be >= 0 and <= 31.
// - `field_size_bits` must be > 0.
// - `field_begin_bit + field_size_bits - 1` must be <= 31.
//
// The returned value is guaranteed to be a valid `field_size_bits`-bit number,
// i.e. it is >= 0 and <= 2 ^ `field_size_bits` - 1.
//
// TODO(https://fxbug.dev/42081461): Replace direct register reads / writes using the
// register helper functions with register classes (such as
// `hwreg::RegisterBase`).
constexpr uint32_t GetFieldValue32(uint32_t reg_value, int field_begin_bit, int field_size_bits) {
  ZX_DEBUG_ASSERT(field_begin_bit >= 0);
  ZX_DEBUG_ASSERT(field_begin_bit <= 31);
  ZX_DEBUG_ASSERT(field_size_bits > 0);
  ZX_DEBUG_ASSERT(field_begin_bit + field_size_bits - 1 <= 31);
  const uint32_t field_mask_unshifted =
      std::numeric_limits<uint32_t>::max() >> (32 - field_size_bits);
  return (reg_value >> field_begin_bit) & field_mask_unshifted;
}

enum CaptureState {
  CAPTURE_RESET = 0,
  CAPTURE_IDLE = 1,
  CAPTURE_ACTIVE = 2,
  CAPTURE_ERROR = 3,
};

constexpr display::DisplayId kPanelDisplayId(1);

constexpr bool kBootloaderDisplayEnabled = true;

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_COMMON_H_
