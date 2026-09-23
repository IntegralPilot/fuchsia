// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_TIME_H_
#define LIB_FIDL_CPP_TIME_H_

#ifdef __Fuchsia__
#include <lib/zx/time.h>
#else
#include <zircon/types.h>
#endif

namespace fidl {

#ifdef __Fuchsia__

template <zx_clock_t kClockId>
using basic_time = zx::basic_time<kClockId>;

template <zx_clock_t kClockId>
using basic_ticks = zx::basic_ticks<kClockId>;

#else

template <zx_clock_t kClockId>
class basic_time final {
 public:
  constexpr basic_time() = default;

  explicit constexpr basic_time(zx_time_t value) : value_(value) {}

  static constexpr basic_time<kClockId> infinite() {
    return basic_time<kClockId>(ZX_TIME_INFINITE);
  }

  static constexpr basic_time<kClockId> infinite_past() {
    return basic_time<kClockId>(ZX_TIME_INFINITE_PAST);
  }

  constexpr zx_time_t get() const { return value_; }

  zx_time_t* get_address() { return &value_; }

  constexpr bool operator==(basic_time<kClockId> other) const { return value_ == other.value_; }
  constexpr bool operator!=(basic_time<kClockId> other) const { return value_ != other.value_; }
  constexpr bool operator<(basic_time<kClockId> other) const { return value_ < other.value_; }
  constexpr bool operator<=(basic_time<kClockId> other) const { return value_ <= other.value_; }
  constexpr bool operator>(basic_time<kClockId> other) const { return value_ > other.value_; }
  constexpr bool operator>=(basic_time<kClockId> other) const { return value_ >= other.value_; }

 private:
  zx_time_t value_ = 0;
};

template <zx_clock_t kClockId>
class basic_ticks final {
 public:
  constexpr basic_ticks() = default;

  explicit constexpr basic_ticks(zx_ticks_t value) : value_(value) {}

  // Acquires the number of ticks contained within this object.
  constexpr zx_ticks_t get() const { return value_; }

  static constexpr basic_ticks<kClockId> infinite() { return basic_ticks(ZX_TIME_INFINITE); }
  static constexpr basic_ticks<kClockId> infinite_past() {
    return basic_ticks(ZX_TIME_INFINITE_PAST);
  }

  constexpr bool operator==(basic_ticks<kClockId> other) const { return value_ == other.value_; }
  constexpr bool operator!=(basic_ticks<kClockId> other) const { return value_ != other.value_; }
  constexpr bool operator<(basic_ticks<kClockId> other) const { return value_ < other.value_; }
  constexpr bool operator<=(basic_ticks<kClockId> other) const { return value_ <= other.value_; }
  constexpr bool operator>(basic_ticks<kClockId> other) const { return value_ > other.value_; }
  constexpr bool operator>=(basic_ticks<kClockId> other) const { return value_ >= other.value_; }

 private:
  zx_ticks_t value_ = 0;
};

#endif  // __Fuchsia__

}  // namespace fidl

#endif  // LIB_FIDL_CPP_TIME_H_
