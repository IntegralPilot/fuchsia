// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZX_TIME_H_
#define LIB_ZX_TIME_H_

#include <stdint.h>
#include <zircon/availability.h>
#include <zircon/compiler.h>
#ifdef __Fuchsia__
#include <zircon/syscalls.h>
#endif
#include <zircon/time.h>
#include <zircon/types.h>

#include <ctime>
#include <limits>

namespace zx {

class duration final {
 public:
  constexpr duration() = default;

  explicit constexpr duration(zx_duration_t value) : value_(value) {}

  explicit constexpr duration(std::timespec ts) : value_(zx_duration_from_timespec(ts)) {}

  static constexpr duration infinite() { return duration(ZX_TIME_INFINITE); }

  static constexpr duration infinite_past() { return duration(ZX_TIME_INFINITE_PAST); }

  constexpr zx_duration_t get() const { return value_; }

  constexpr duration operator-() const ZX_AVAILABLE_SINCE(9) {
    return duration(zx_duration_mul_int64(value_, -1));
  }

  constexpr duration operator+(duration other) const ZX_AVAILABLE_SINCE(7) {
    return duration(zx_duration_add_duration(value_, other.value_));
  }

  constexpr duration operator-(duration other) const ZX_AVAILABLE_SINCE(7) {
    return duration(zx_duration_sub_duration(value_, other.value_));
  }

  constexpr duration operator*(int64_t multiplier) const ZX_AVAILABLE_SINCE(7) {
    return duration(zx_duration_mul_int64(value_, multiplier));
  }

  constexpr duration operator/(int64_t divisor) const { return duration(value_ / divisor); }

  constexpr int64_t operator/(duration other) const { return value_ / other.value_; }

  constexpr duration operator%(int64_t divisor) const { return duration(value_ % divisor); }

  constexpr int64_t operator%(duration other) const { return value_ % other.value_; }

  constexpr duration& operator+=(duration other) ZX_AVAILABLE_SINCE(7) {
    value_ = zx_duration_add_duration(value_, other.value_);
    return *this;
  }

  constexpr duration& operator-=(duration other) ZX_AVAILABLE_SINCE(7) {
    value_ = zx_duration_sub_duration(value_, other.value_);
    return *this;
  }

  constexpr duration& operator*=(int64_t multiplier) ZX_AVAILABLE_SINCE(7) {
    value_ = zx_duration_mul_int64(value_, multiplier);
    return *this;
  }

  constexpr duration& operator/=(int64_t divisor) {
    value_ /= divisor;
    return *this;
  }

  constexpr duration& operator%=(int64_t divisor) {
    value_ %= divisor;
    return *this;
  }

  constexpr bool operator==(duration other) const { return value_ == other.value_; }
  constexpr bool operator!=(duration other) const { return value_ != other.value_; }
  constexpr bool operator<(duration other) const { return value_ < other.value_; }
  constexpr bool operator<=(duration other) const { return value_ <= other.value_; }
  constexpr bool operator>(duration other) const { return value_ > other.value_; }
  constexpr bool operator>=(duration other) const { return value_ >= other.value_; }

  constexpr int64_t to_nsecs() const { return value_; }

  constexpr int64_t to_usecs() const { return value_ / ZX_USEC(1); }

  constexpr int64_t to_msecs() const { return value_ / ZX_MSEC(1); }

  constexpr int64_t to_secs() const { return value_ / ZX_SEC(1); }

  constexpr int64_t to_mins() const { return value_ / ZX_MIN(1); }

  constexpr int64_t to_hours() const { return value_ / ZX_HOUR(1); }

  constexpr std::timespec to_timespec() const { return zx_timespec_from_duration(value_); }

 private:
  zx_duration_t value_ = 0;
} ZX_AVAILABLE_SINCE(7);

constexpr duration operator*(int64_t multiplier, duration d) ZX_AVAILABLE_SINCE(9) {
  return d * multiplier;
}

template <zx_clock_t kClockId>
class basic_ticks final {
 public:
  constexpr basic_ticks() = default;

  explicit constexpr basic_ticks(zx_ticks_t value) : value_(value) {}

  static basic_ticks<kClockId> now();

#ifdef __Fuchsia__
  // Returns the number of ticks contained within one second.
  static basic_ticks<kClockId> per_second() ZX_AVAILABLE_SINCE(7) {
    return basic_ticks(zx_ticks_per_second());
  }
#endif

  // Acquires the number of ticks contained within this object.
  constexpr zx_ticks_t get() const { return value_; }

  static constexpr basic_ticks<kClockId> infinite() { return basic_ticks(INFINITE); }

  static constexpr basic_ticks<kClockId> infinite_past() { return basic_ticks(INFINITE_PAST); }

  constexpr basic_ticks<kClockId> operator+(basic_ticks<kClockId> other) const {
    zx_ticks_t x = 0;

    if (unlikely(add_overflow(value_, other.value_, &x))) {
      if (x >= 0) {
        return infinite_past();
      } else {
        return infinite();
      }
    }

    return basic_ticks(x);
  }

  constexpr basic_ticks<kClockId> operator-(basic_ticks<kClockId> other) const {
    zx_ticks_t x = 0;

    if (unlikely(sub_overflow(value_, other.value_, &x))) {
      if (x >= 0) {
        return infinite_past();
      } else {
        return infinite();
      }
    }

    return basic_ticks(x);
  }

  constexpr basic_ticks<kClockId> operator*(uint64_t multiplier) const {
    zx_ticks_t x = 0;

    if (unlikely(mul_overflow(value_, multiplier, &x))) {
      if (value_ < 0) {
        return infinite_past();
      } else {
        return infinite();
      }
    }

    return basic_ticks<kClockId>(x);
  }

  constexpr basic_ticks<kClockId> operator/(uint64_t divisor) const {
    return basic_ticks(static_cast<int64_t>(static_cast<uint64_t>(value_) / divisor));
  }

  constexpr uint64_t operator/(basic_ticks<kClockId> other) const {
    return static_cast<uint64_t>(value_ / other.value_);
  }

  constexpr basic_ticks<kClockId> operator%(uint64_t divisor) const {
    return basic_ticks(static_cast<int64_t>(static_cast<uint64_t>(value_) % divisor));
  }

  constexpr uint64_t operator%(basic_ticks<kClockId> other) const {
    return static_cast<uint64_t>(value_ % other.value_);
  }

  constexpr basic_ticks<kClockId>& operator+=(basic_ticks<kClockId> other) {
    *this = *this + other;
    return *this;
  }

  constexpr basic_ticks<kClockId>& operator-=(basic_ticks<kClockId> other) {
    *this = *this - other;
    return *this;
  }

  constexpr basic_ticks<kClockId>& operator*=(uint64_t multiplier) {
    *this = *this * multiplier;
    return *this;
  }

  constexpr basic_ticks<kClockId>& operator/=(uint64_t divisor) {
    value_ /= divisor;
    return *this;
  }

  constexpr basic_ticks<kClockId>& operator%=(uint64_t divisor) {
    value_ %= divisor;
    return *this;
  }

  constexpr bool operator==(basic_ticks<kClockId> other) const { return value_ == other.value_; }
  constexpr bool operator!=(basic_ticks<kClockId> other) const { return value_ != other.value_; }
  constexpr bool operator<(basic_ticks<kClockId> other) const { return value_ < other.value_; }
  constexpr bool operator<=(basic_ticks<kClockId> other) const { return value_ <= other.value_; }
  constexpr bool operator>(basic_ticks<kClockId> other) const { return value_ > other.value_; }
  constexpr bool operator>=(basic_ticks<kClockId> other) const { return value_ >= other.value_; }

 private:
  static constexpr zx_ticks_t INFINITE = std::numeric_limits<zx_ticks_t>::max();
  static constexpr zx_ticks_t INFINITE_PAST = std::numeric_limits<zx_ticks_t>::min();

  zx_ticks_t value_ = 0;
} ZX_AVAILABLE_SINCE(7);

using ticks = basic_ticks<ZX_CLOCK_MONOTONIC>;
using ticks_boot = basic_ticks<ZX_CLOCK_BOOT>;
using ticks_monotonic = basic_ticks<ZX_CLOCK_MONOTONIC>;

template <zx_clock_t kClockId>
class basic_time final {
 public:
  constexpr basic_time() = default;

  explicit constexpr basic_time(zx_time_t value) : value_(value) {}

  explicit constexpr basic_time(std::timespec ts) : value_(zx_time_from_timespec(ts)) {}

  static constexpr basic_time<kClockId> infinite() {
    return basic_time<kClockId>(ZX_TIME_INFINITE);
  }

  static constexpr basic_time<kClockId> infinite_past() {
    return basic_time<kClockId>(ZX_TIME_INFINITE_PAST);
  }

  constexpr zx_time_t get() const { return value_; }

  zx_time_t* get_address() { return &value_; }

  constexpr duration operator-(basic_time<kClockId> other) const ZX_AVAILABLE_SINCE(7) {
    return duration(zx_time_sub_time(value_, other.value_));
  }

  constexpr basic_time<kClockId> operator+(duration delta) const ZX_AVAILABLE_SINCE(7) {
    return basic_time<kClockId>(zx_time_add_duration(value_, delta.get()));
  }

  constexpr basic_time<kClockId> operator-(duration delta) const ZX_AVAILABLE_SINCE(7) {
    return basic_time<kClockId>(zx_time_sub_duration(value_, delta.get()));
  }

  constexpr basic_time<kClockId>& operator+=(duration delta) ZX_AVAILABLE_SINCE(7) {
    value_ = zx_time_add_duration(value_, delta.get());
    return *this;
  }

  constexpr basic_time<kClockId>& operator-=(duration delta) ZX_AVAILABLE_SINCE(7) {
    value_ = zx_time_sub_duration(value_, delta.get());
    return *this;
  }

  constexpr bool operator==(basic_time<kClockId> other) const { return value_ == other.value_; }
  constexpr bool operator!=(basic_time<kClockId> other) const { return value_ != other.value_; }
  constexpr bool operator<(basic_time<kClockId> other) const { return value_ < other.value_; }
  constexpr bool operator<=(basic_time<kClockId> other) const { return value_ <= other.value_; }
  constexpr bool operator>(basic_time<kClockId> other) const { return value_ > other.value_; }
  constexpr bool operator>=(basic_time<kClockId> other) const { return value_ >= other.value_; }

  constexpr std::timespec to_timespec() const { return zx_timespec_from_time(value_); }

 private:
  zx_time_t value_ = 0;
} ZX_AVAILABLE_SINCE(7);

template <zx_clock_t kClockId>
constexpr basic_time<kClockId> operator+(duration delta, basic_time<kClockId> time) {
  return time + delta;
}

using time = basic_time<ZX_CLOCK_MONOTONIC>;
using time_boot = basic_time<ZX_CLOCK_BOOT>;
using time_monotonic = basic_time<ZX_CLOCK_MONOTONIC>;

constexpr inline duration nsec(int64_t n) { return duration(ZX_NSEC(n)); }

constexpr inline duration usec(int64_t n) { return duration(ZX_USEC(n)); }

constexpr inline duration msec(int64_t n) { return duration(ZX_MSEC(n)); }

constexpr inline duration sec(int64_t n) { return duration(ZX_SEC(n)); }

constexpr inline duration min(int64_t n) { return duration(ZX_MIN(n)); }

constexpr inline duration hour(int64_t n) { return duration(ZX_HOUR(n)); }

#ifdef __Fuchsia__
inline zx_status_t nanosleep(zx::time deadline) ZX_AVAILABLE_SINCE(7) {
  return zx_nanosleep(deadline.get());
}

inline time deadline_after(zx::duration nanoseconds) ZX_AVAILABLE_SINCE(7) {
  return time(zx_deadline_after(nanoseconds.get()));
}
#endif

}  // namespace zx

#endif  // LIB_ZX_TIME_H_
