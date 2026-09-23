// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_SPIN_TRACING_CONFIG_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_SPIN_TRACING_CONFIG_H_

#include <stdint.h>

#if SCHEDULER_LOCK_SPIN_TRACING_ENABLED
constexpr bool kSchedulerLockSpinTracingEnabled = true;
#else
constexpr bool kSchedulerLockSpinTracingEnabled = false;
#endif

#if SCHEDULER_LOCK_SPIN_TRACING_COMPRESSED
constexpr bool kSchedulerLockSpinTracingCompressed = true;
#else
constexpr bool kSchedulerLockSpinTracingCompressed = false;
#endif

static_assert(!kSchedulerLockSpinTracingCompressed || kSchedulerLockSpinTracingEnabled,
              "Error: Compress lock-spin trace records requested, but lock-spin tracing is not "
              "enabled.");

namespace spin_tracing {

namespace internal {

template <typename T, uint32_t kBits, uint32_t kShift>
struct Bitfield {
  static inline constexpr uint64_t kMask = ((uint64_t{1} << kBits) - 1);

  static inline constexpr uint64_t Encode(T val) {
    return (static_cast<uint64_t>(val) & kMask) << kShift;
  }

  static inline constexpr T Decode(uint64_t val) { return static_cast<T>((val >> kShift) & kMask); }

  static inline constexpr uint64_t Reset(uint64_t old_val, T new_val) {
    return (old_val & ~(kMask << kShift)) | ((static_cast<uint64_t>(new_val) & kMask) << kShift);
  }
};

}  // namespace internal

enum class LockType { kSpinlock = 0, kMutex = 1, kChainLock = 2, kRwLock = 3 };
enum class FinishType {
  kLockAcquired = 0,
  kBlocked = 1,
};

class EncodedLockId {
 public:
  using IdBits = internal::Bitfield<uint64_t, 49, 0>;            // bits [ 0 .. 48]
  using ClassNameBits = internal::Bitfield<uint16_t, 12, 49>;    // bits [49 .. 60]
  using LockTypeBits = internal::Bitfield<LockType, 2, 61>;      // bits [61 .. 62]
  using FinishTypeBits = internal::Bitfield<FinishType, 1, 63>;  // bits [63 .. 63]

  constexpr EncodedLockId(LockType type, uint64_t lock_id, uint16_t class_name_id)
      : value_{LockTypeBits::Encode(type) | IdBits::Encode(lock_id) |
               ClassNameBits::Encode(class_name_id)} {}

  static constexpr EncodedLockId Invalid() { return EncodedLockId{}; }

  void SetLockClassId(uint16_t class_name_id) {
    value_ = ClassNameBits::Reset(value_, class_name_id);
  }

  uint64_t FinishedValue(FinishType finish_type) {
    return value_ | FinishTypeBits::Encode(finish_type);
  }

  constexpr uint64_t value() const { return value_; }
  constexpr uint64_t id() const { return IdBits::Decode(value_); }
  constexpr uint16_t class_name() const { return ClassNameBits::Decode(value_); }
  constexpr LockType lock_type() const { return LockTypeBits::Decode(value_); }
  constexpr FinishType finish_type() const { return FinishTypeBits::Decode(value_); }

 private:
  constexpr EncodedLockId() = default;

  uint64_t value_{0};
};

}  // namespace spin_tracing

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_SPIN_TRACING_CONFIG_H_
