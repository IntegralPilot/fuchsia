// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_AFFINE_H_
#define ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_AFFINE_H_

#include <inttypes.h>
#include <zircon/assert.h>

#include <algorithm>

#include <ffl/fixed.h>

#include "time.h"

namespace sched {

// Affine performs simple affine transformations between monotonic and variable timelines for
// variable bandwidth scheduling.
template <size_t PrecisionBits = 10>
class Affine {
 public:
  using Slope = ffl::Fixed<int64_t, PrecisionBits>;

  static constexpr int64_t kPrecision = static_cast<int64_t>(Slope::Format::Power);
  static constexpr Slope kMaxSlope{1};
  static constexpr Slope kMinSlope{Slope::Epsilon()};

  Affine() = default;
  ~Affine() = default;

  Affine(const Affine&) = default;
  Affine& operator=(const Affine&) = default;

  constexpr Time MonotonicToVariable(Time monotonic) const {
    return Multiply(monotonic - monotonic_reference_time_) + variable_reference_time_;
  }
  constexpr Time VariableToMonotonic(Time variable) const {
    return Divide(variable - variable_reference_time_) + monotonic_reference_time_;
  }

  constexpr void MonotonicToVariableInPlace(Time& ref) const { ref = MonotonicToVariable(ref); }
  constexpr void VariableToMonotonicInPlace(Time& ref) const { ref = VariableToMonotonic(ref); }

  constexpr void ChangeSlopeAtMonotonicTime(Time monotonic, Slope slope) {
    ZX_DEBUG_ASSERT(slope >= kMinSlope && slope <= kMaxSlope);
    variable_reference_time_ = MonotonicToVariable(monotonic);
    monotonic_reference_time_ = monotonic;
    slope_ = slope;
  }

  constexpr void Snap(Time monotonic, Time variable) {
    monotonic_reference_time_ = std::max(monotonic, monotonic_reference_time_);
    variable_reference_time_ = std::max(variable, variable_reference_time_);
  }

  constexpr Time monotonic_reference_time() const { return monotonic_reference_time_; }
  constexpr Time variable_reference_time() const { return variable_reference_time_; }
  constexpr Slope slope() const { return slope_; }

 private:
  constexpr Time Multiply(Time delta) const {
    if (slope_ != kMaxSlope) {
      const Time shifted = delta / kPrecision;  // Arithmetic right shift.
      const Time scaled = shifted * slope_.raw_value();
      return scaled;
    }
    return delta;
  }
  constexpr Time Divide(Time delta) const {
    if (slope_ != kMaxSlope) {
      const Time scaled = delta / slope_.raw_value();
      const Time shifted = scaled * kPrecision;  // Arithmetic left shift.
      return shifted;
    }
    return delta;
  }

  Time monotonic_reference_time_{0};
  Time variable_reference_time_{0};
  Slope slope_{1};
};

}  // namespace sched

#endif  // ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_AFFINE_H_
