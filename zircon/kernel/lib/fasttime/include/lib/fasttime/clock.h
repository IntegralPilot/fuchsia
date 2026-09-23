// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_CLOCK_H_
#define ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_CLOCK_H_

#include <lib/affine/transform.h>
#include <lib/concurrent/seqlock.h>
#include <lib/fasttime/time.h>
#include <zircon/syscalls/clock.h>
#include <zircon/types.h>

#include <concepts>

class ClockDispatcher;

namespace fasttime {

// ClockTransformation structures in `libfasttime` need an adapter class
// injected into them in order to operate correctly in their environment.  The
// transformation structure will be used in either user mode (via the VDSO) or
// in the kernel, and there will be a few differences in their behavior
// depending on where they are executing.
//
// Both environments need to make use of libconcurrent's implementation of a
// SeqLock, however we will only need to use a subset of the SeqLock API as we
// never need to wait with a timeout on the lock.  As a result, we only need
// to supply an implementation of ArchYield in order to use SeqLocks.
//
// Actually reading or getting details for a clock requires access to either
// the monotonic or boot ticks timeline, depending on how the clock is
// configured.  Depending on the environment, we need to be able to do this
// one of three ways.
//
// 1) Using current_ticks/current_boot_ticks when operating in the Kernel.
// 2) Using libfasttime directly when operating in a VDSO and direct
//    observation of the underlying reference counter HW is permitted in
//    user-mode.
// 3) Using a syscall to fetch ticks when operating in a VDSO when directly
//    reading the underlying reference counter HW is not permitted.
//
// Users will provide an implementation of GetMonoTicks and GetBootTicks which
// is appropriate for their environment in order to provide the required
// access to the timelines.
template <typename T>
concept ClockTransformationAdapter = requires(T) {
  { T::ArchYield() } -> std::same_as<void>;
  { T::GetMonoTicks() } -> std::same_as<zx_instant_mono_ticks_t>;
  { T::GetBootTicks() } -> std::same_as<zx_instant_boot_ticks_t>;
};

template <ClockTransformationAdapter Adapter>
struct ClockTransformation {
 public:
  ClockTransformation(uint64_t options, zx_time_t backstop_time)
      : options_(options), backstop_time_(backstop_time) {}

  bool is_monotonic() const { return (options_ & ZX_CLOCK_OPT_MONOTONIC) != 0; }
  bool is_boot() const { return (options_ & ZX_CLOCK_OPT_BOOT) != 0; }
  bool is_continuous() const { return (options_ & ZX_CLOCK_OPT_CONTINUOUS) != 0; }
  bool is_mappable() const { return (options_ & ZX_CLOCK_OPT_MAPPABLE) != 0; }
  bool is_started() __TA_REQUIRES(seq_lock_) {
    // Note, we require that we hold the seq_lock_ exclusively here.  This
    // should ensure that there are no other threads writing to this memory
    // location concurrent with our read, meaning there is no formal data race
    // here.
    return params_.unsynchronized_get().reference_to_synthetic.numerator() != 0;
  }

  zx_status_t Read(zx_time_t* out_now) const;

  // Reads the current value of the clock, as well as the clock's backstop value.
  //
  // You can get the same info from GetDetails and some math. But GetDetails
  // does more work internally, and ends up being slower. In hot code
  // paths it shows up as perf regressions. If you can't afford to throw away
  // that extra work, and you need time and backstop only and no other clock
  // details, then this is a function for you.
  zx_status_t ReadWithBackstop(zx_time_t* out_now, zx_time_t* out_backstop_time) const;
  zx_status_t GetDetails(zx_clock_details_v1_t* out_details) const;

 private:
  friend class ::ClockDispatcher;

  zx_ticks_t GetCurrentTicks() const {
    return is_boot() ? Adapter::GetBootTicks() : Adapter::GetMonoTicks();
  }

  static zx_clock_transformation_t CopyTransform(const affine::Transform& src) {
    return {src.a_offset(), src.b_offset(), {src.numerator(), src.denominator()}};
  }

  struct Params {
    affine::Transform reference_to_synthetic{0, 0, {0, 1}};
    uint64_t error_bound = ZX_CLOCK_UNKNOWN_ERROR;
    zx_ticks_t last_value_update_ticks = 0;
    zx_ticks_t last_rate_adjust_update_ticks = 0;
    zx_ticks_t last_error_bounds_update_ticks = 0;
    int32_t cur_ppm_adj = 0;
    uint32_t _padding = 0;
  };

  // Make sure that our Params both a standard layout, and a unique representation.
  //
  // The standard layout requirement ensures that we don't have wonky stuff like
  // a vtable, or multiple inheritance going on (just a plain 'ole structure
  // please).  The unique representation requirement makes sure that we don't
  // have any non-explicit padding.  All of the field and their initial values
  // are defined above.
  //
  // Both of these things are important when we plan to share this data
  // structure between the kernel and user-mode, which we do.
  static_assert(std::is_standard_layout_v<Params>);
  static_assert(std::has_unique_object_representations_v<Params>);

  // Constants determined at construction time.  These never change and do not
  // need to be protected with the sequence lock.
  const uint64_t options_;
  const zx_time_t backstop_time_;

  // The transformation "payload" parameters, and the sequence lock which protects them.
  //
  // Note that the reference_ticks_to_synthetic transformation is kept separate from the
  // rest of the parameters.  While we need to observe all of the parameters
  // during a call to GetDetails, we only need to observe reference_ticks_to_synthetic
  // during Read, and keeping the parameters separate makes this a bit easier.
  using SeqLockType = ::concurrent::internal::SeqLock<Adapter, concurrent::SyncOpt::Fence>;
  SeqLockType seq_lock_;

  template <typename T>
  using Payload = concurrent::SeqLockPayload<T, decltype(seq_lock_)>;

  __TA_GUARDED(seq_lock_)
  Payload<affine::Transform> reference_ticks_to_synthetic_{0, 0, affine::Ratio{0, 1}};
  __TA_GUARDED(seq_lock_) Payload<Params> params_;
};

template <ClockTransformationAdapter Adapter>
inline zx_status_t ClockTransformation<Adapter>::Read(zx_time_t* out_now) const {
  int64_t now_ticks;
  affine::Transform ticks_to_synthetic;

  bool transaction_success;
  do {
    typename SeqLockType::ReadTransactionToken token = seq_lock_.BeginReadTransaction();

    reference_ticks_to_synthetic_.Read(ticks_to_synthetic);
    now_ticks = GetCurrentTicks();

    transaction_success = seq_lock_.EndReadTransaction(token);
  } while (!transaction_success);

  *out_now = ticks_to_synthetic.Apply(now_ticks);
  return ZX_OK;
}

template <ClockTransformationAdapter Adapter>
inline zx_status_t ClockTransformation<Adapter>::ReadWithBackstop(
    zx_time_t* out_now, zx_time_t* out_backstop_time) const {
  *out_backstop_time = backstop_time_;
  return Read(out_now);
}

template <ClockTransformationAdapter Adapter>
inline zx_status_t ClockTransformation<Adapter>::GetDetails(
    zx_clock_details_v1_t* out_details) const {
  int64_t now_ticks;
  affine::Transform ticks_to_synthetic;
  Params params;
  uint32_t generation_counter;

  bool transaction_success;
  do {
    typename SeqLockType::ReadTransactionToken token = seq_lock_.BeginReadTransaction();

    reference_ticks_to_synthetic_.Read(ticks_to_synthetic);
    params_.Read(params);
    generation_counter = token.seq_num();
    now_ticks = GetCurrentTicks();

    transaction_success = seq_lock_.EndReadTransaction(token);
  } while (!transaction_success);

  ZX_DEBUG_ASSERT(!(generation_counter & 0x1));
  out_details->generation_counter = generation_counter >> 1;
  out_details->reference_ticks_to_synthetic = CopyTransform(ticks_to_synthetic);
  out_details->reference_to_synthetic = CopyTransform(params.reference_to_synthetic);
  out_details->error_bound = params.error_bound;
  out_details->query_ticks = now_ticks;
  out_details->last_value_update_ticks = params.last_value_update_ticks;
  out_details->last_rate_adjust_update_ticks = params.last_rate_adjust_update_ticks;
  out_details->last_error_bounds_update_ticks = params.last_error_bounds_update_ticks;

  // Options and backstop_time are constant over the life of the clock.  We
  // don't need to latch them during the generation counter spin.
  out_details->options = options_;
  out_details->backstop_time = backstop_time_;

  return ZX_OK;
}

}  // namespace fasttime

#endif  // ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_CLOCK_H_
