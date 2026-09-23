// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_PLATFORM_TIMER_H_
#define ZIRCON_KERNEL_INCLUDE_PLATFORM_TIMER_H_

#include <assert.h>
#include <lib/arch/ticks.h>
#include <zircon/compiler.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <fbl/enum_bits.h>
#include <ktl/atomic.h>
#include <ktl/optional.h>
#include <ktl/type_traits.h>

namespace internal {

// A modifier that converts a raw ticks counter value to a point on the monotonic ticks timeline.
// This modifier has three different modes of operation.
// 1. If the value is ZX_TIME_INFINITE_PAST, then the modifier is being updated and all attempts
//    to read it should be retried.
// 2. If the value is positive (> 0), the monotonic clock is paused and this variable contains the
//    value of the clock when it was paused.
// 3. If the value is non-positive (<= 0), the monotonic clock is ticking and this variable contains
//    the offset between the monotonic ticks and the raw ticks counter.
inline ktl::atomic<zx_ticks_t> mono_ticks_modifier{0};

// This threshold detects when the initial ticks offset is set to a value that is "Very Close" to
// setting bit 63. This would be a problem because modifying bit 63 would change the sign of the
// offset value, which would then break our ability to use the sign as a sentinel that indicates
// how the mono_ticks_modifier value should be interpreted (see the comment around that variable
// for more information on how that works).
//
// "Very Close" in this case means a value that is within approximately 10 years of setting bit 63,
// assuming a clock rate of 10ghz.
inline constexpr uint64_t kTicksOopsThreshold =
    0x8000'0000'0000'0000ul -
    (static_cast<uint64_t>(365.2422 * 10 * 86400) *  // 10 years of seconds
     10'000'000'000);                                // 10GHz

// Offset between the raw ticks counter and the boot ticks timeline.
inline ktl::atomic<uint64_t> boot_ticks_offset{0};

}  // namespace internal

// Sets the initial ticks value using the underlying hardware register value. This value is used to
// ensure that both the boot and monotonic timelines start at 0.
//
// Must be called with interrupts disabled, before secondary CPUs are booted.
inline void timer_set_initial_ticks(uint64_t ticks) {
  // Verify that the offset is not "dangerously close" to setting bit 63. If it is, emit a KERNEL
  // OOPS.
  if (ticks > internal::kTicksOopsThreshold) {
    KERNEL_OOPS("initial ticks value %lu is very close to setting bit 63\n", ticks);
  }

  // Negate the value to yield a signed offset that later be added to an observed counter register
  // value.
  const zx_ticks_t offset = -static_cast<zx_ticks_t>(ticks);
  internal::mono_ticks_modifier.store(offset, ktl::memory_order_relaxed);
  internal::boot_ticks_offset.store(offset, ktl::memory_order_relaxed);

  // Because |boot_ticks_offset| is used to compute the current time, defer printing until after it
  // has been set to ensure the resulting log message timestamp makes sense.
  printf("timer: initial offset is %ld\n", offset);
}

// Converts a ticks value on the monotonic timeline to a raw hardware ticks value.
// Returns the raw ticks value if the monotonic clock is not paused, and nullopt if it is.
inline ktl::optional<zx_ticks_t> timer_convert_mono_to_raw_ticks(
    zx_instant_mono_ticks_t mono_ticks) {
  const zx_ticks_t modifier = internal::mono_ticks_modifier.load(ktl::memory_order_relaxed);
  if (modifier > 0) {
    return ktl::nullopt;
  }
  return ktl::optional<zx_ticks_t>(zx_ticks_sub_ticks(mono_ticks, modifier));
}

// Access the platform specific offset from the raw ticks timeline to the monotonic ticks
// timeline.  The only current legit uses for this function are when
// initializing the RO data for the VDSO, and when fixing up timer values during
// vmexit on ARM (see arch/arm64/hypervisor/vmexit.cc).
inline zx_ticks_t timer_get_mono_ticks_offset() {
  const zx_ticks_t offset = internal::mono_ticks_modifier.load(ktl::memory_order_relaxed);
  DEBUG_ASSERT(offset <= 0);
  return offset;
}

// Access the offset from the raw ticks timeline to the boot ticks timeline.
inline zx_ticks_t timer_get_boot_ticks_offset() {
  return internal::boot_ticks_offset.load(ktl::memory_order_relaxed);
}

// API to set/clear a hardware timer that is responsible for calling timer_tick() when it fires.
zx_status_t platform_set_oneshot_timer(zx_ticks_t deadline);
void platform_stop_timer();

// Shutdown the calling CPU's platform timer.
//
// Should be called after |platform_stop_timer|, but before taking the CPU offline.
//
// TODO(maniscalco): Provide a "resume" function so we can suspend/resume.
void platform_shutdown_timer();

// Suspend the platform timer for the calling CPU.
//
// It is an error to call this function with interrupts enabled.
zx_status_t platform_suspend_timer_curr_cpu();

// Resume the platform timer for the calling CPU.
//
// It is an error to call this function with interrupts enabled.
zx_status_t platform_resume_timer_curr_cpu();

void timer_tick();

// A bool indicating whether or not user mode has direct access to the registers
// which allow directly observing the tick counter or not.
bool platform_usermode_can_access_tick_registers();

// Current monotonic time in nanoseconds.
zx_instant_mono_t current_mono_time();

// Current boot time in nanoseconds.
zx_instant_boot_t current_boot_time();

// Monotonic and boot time representing the same instant in both timelines.
struct InstantMonoAndBootTime {
  zx_instant_mono_t mono_time;
  zx_instant_boot_t boot_time;
};

// Current monotonic and boot time in nanoseconds, derived from a coherent
// observation of the offsets and raw ticks.
InstantMonoAndBootTime current_mono_and_boot_time();

// Monotonic time and boot ticks representing same instant in both timelines.
// This is primarily useful for lining up samples of monotonic time with
// corresponding trace events, which use boot ticks, so that computed and
// visualized intervals are accurate.
struct InstantMonoTimeAndBootTicks {
  zx_instant_mono_t mono_time;
  zx_instant_boot_ticks_t boot_ticks;
};

// Current monotonic time in nanoseconds and boot ticks, derived from a coherent
// observation of the offsets and raw ticks.
InstantMonoTimeAndBootTicks current_mono_time_and_boot_ticks();

// High-precision timer ticks per second.
zx_ticks_t ticks_per_second();

namespace affine {
class Ratio;  // Fwd decl.
}  // namespace affine

// Setter/getter pair for the ratio which defines the relationship between the
// system's tick counter, and the current_time/clock_monotonic clock.  This gets
// set once by architecture specific platform code, after an appropriate ticks
// source has been selected and characterized.
void timer_set_ticks_to_time_ratio(const affine::Ratio& ticks_to_time);
const affine::Ratio& timer_get_ticks_to_time_ratio();

// Convert a sample taken early on to a proper zx_ticks_t, if possible.
// This returns 0 if early samples are not convertible.
zx_ticks_t platform_convert_early_ticks(arch::EarlyTicks sample);

// A special version of `current_ticks` which is explicitly synchronized with
// memory loads/stores in an architecture/timer specific way.  For example, when
// using TSC as the ticks reference on an x64 system, there is nothing
// explicitly preventing the actual read of the TSC finishing before or after
// previous/subsequent loads/stores (unless we are talking about a subsequent
// store which has a dependency on the TSC read).
//
// Most of the time, this does not matter, but occasionally it can be an issue.
// For example, some algorithms need to record a timestamp while inside of a
// critical section (either shared or exclusive).  While the implementation of
// the synchronization object typically uses atomics and explicit C++ memory
// order annotations to be sure that (for example) stores to some payload done
// by a thread with exclusive access to the payload "happen-before" any
// subsequent loads from threads with shared access to the payload, accesses to
// the ticks reference on a system is typically not constrained by such
// dependencies.
//
// Without special care, the time observed on the system timer can actually
// occur before/after any loads or stores which are part of the entry/exit
// into/out-of the critical section.
//
// In such situations, timer_current_mono_ticks_synchronized can be used instead
// to order to contain the timer observation.  Users pass (via template args) a
// set of flags which cause appropriate architecture specific barriers to be
// inserted before/after the ticks reference observation in order to contain it.
// Specifically, users can demand that the observation must take place:
//
//  + After any previous loads
//  + After any previous stores
//  + Before any subsequent loads
//  + Before any subsequent stores
//
// or any combination of the above.  Some timer/architecture combinations might
// require even more strict synchronization than was requested, but all should
// provide at least the minimum level of synchronization to satisfy the request.

enum class GetTicksSyncFlag : uint8_t {
  kNone = 0,
  kAfterPreviousLoads = (1 << 0),
  kAfterPreviousStores = (1 << 1),
  kBeforeSubsequentLoads = (1 << 2),
  kBeforeSubsequentStores = (1 << 3),
};

FBL_ENABLE_ENUM_BITS(GetTicksSyncFlag)

// Reads a platform-specific ticks counter.
//
// This function is required to return zero if it is invoked before the timer hardware is
// initialized. Not doing so could violate the monotonicity of both the monotonic and boot
// timelines.
//
// This raw counter value is almost certainly not what you want; callers generally want to use the
// monotonic ticks value provided by timer_current_mono_ticks. This ticks value is the raw counter
// adjusted by a constant used to make the ticks timeline start ticking from 0 when the system
// boots.
template <GetTicksSyncFlag Flags>
zx_ticks_t platform_current_raw_ticks_synchronized();
inline zx_ticks_t platform_current_raw_ticks() {
  return platform_current_raw_ticks_synchronized<GetTicksSyncFlag::kNone>();
}

template <GetTicksSyncFlag Flags>
inline zx_instant_mono_ticks_t timer_current_mono_ticks_synchronized() {
  while (true) {
    const zx_ticks_t obs1 = internal::mono_ticks_modifier.load(ktl::memory_order_relaxed);
    const zx_ticks_t raw_ticks = platform_current_raw_ticks_synchronized<Flags>();
    const zx_ticks_t obs2 = internal::mono_ticks_modifier.load(ktl::memory_order_relaxed);
    if (obs1 == obs2 && obs1 != ZX_TIME_INFINITE_PAST) {
      // If the observation is positive, it's the value of the paused monotonic clock, so return
      // the observation directly. Otherwise, it's an offset, so return the sum of the observation
      // and the raw ticks counter.
      if (unlikely(obs1 > 0)) {
        return obs1;
      }
      return raw_ticks + obs1;
    }
  }
}
inline zx_instant_mono_ticks_t timer_current_mono_ticks() {
  return timer_current_mono_ticks_synchronized<GetTicksSyncFlag::kAfterPreviousLoads |
                                               GetTicksSyncFlag::kBeforeSubsequentLoads>();
}

template <GetTicksSyncFlag Flags>
inline zx_instant_boot_ticks_t timer_current_boot_ticks_synchronized() {
  while (true) {
    const zx_ticks_t off1 = internal::boot_ticks_offset.load(ktl::memory_order_relaxed);
    const zx_ticks_t raw_ticks = platform_current_raw_ticks_synchronized<Flags>();
    const zx_ticks_t off2 = internal::boot_ticks_offset.load(ktl::memory_order_relaxed);
    if (off1 == off2) {
      return raw_ticks + off1;
    }
  }
}
inline zx_instant_boot_ticks_t timer_current_boot_ticks() {
  return timer_current_boot_ticks_synchronized<GetTicksSyncFlag::kNone>();
}

// CurrentTicksObservation contains a coherent read of the current ticks on both the monotonic and
// boot timeline.
struct CurrentTicksObservation {
  zx_instant_mono_ticks_t mono_now;
  zx_instant_boot_ticks_t boot_now;
};
// Performs a coherent read of both the mono and boot timelines and returns a tuple with the current
// value of both timelines.
inline CurrentTicksObservation timer_current_mono_and_boot_ticks() {
  while (true) {
    const zx_ticks_t boot_off1 = internal::boot_ticks_offset.load(ktl::memory_order_relaxed);
    const zx_ticks_t mono_obs1 = internal::mono_ticks_modifier.load(ktl::memory_order_relaxed);
    const zx_ticks_t raw_ticks =
        platform_current_raw_ticks_synchronized<GetTicksSyncFlag::kAfterPreviousLoads |
                                                GetTicksSyncFlag::kBeforeSubsequentLoads>();
    const zx_ticks_t boot_off2 = internal::boot_ticks_offset.load(ktl::memory_order_relaxed);
    const zx_ticks_t mono_obs2 = internal::mono_ticks_modifier.load(ktl::memory_order_relaxed);
    if (boot_off1 == boot_off2 && mono_obs1 == mono_obs2 && mono_obs1 != ZX_TIME_INFINITE_PAST) {
      // If our monotonic modifier observation was positive, the clock is paused and the observation
      // is the current monotonic ticks value.
      const zx_instant_mono_ticks_t mono_now = mono_obs1 > 0 ? mono_obs1 : raw_ticks + mono_obs1;
      const zx_instant_boot_ticks_t boot_now = raw_ticks + boot_off1;
      return {.mono_now = mono_now, .boot_now = boot_now};
    }
  }
}

// The current monotonic time in ticks.
inline zx_instant_mono_ticks_t current_mono_ticks() { return timer_current_mono_ticks(); }

// The current boot time in ticks.
inline zx_instant_boot_ticks_t current_boot_ticks() { return timer_current_boot_ticks(); }

// Pause/unpause the monotonic clock.
//
// These functions are safe to call concurrently with `timer_current_mono_ticks_synchronized`, as
// that function will retry its read of the current ticks if the value of the `mono_ticks_modifier`
// is ZX_TIME_INFINITE_PAST, which we set it to before our update, or if it notices that the value
// has changed during its read.
//
// It is NOT safe to call timer_pause_monotonic and timer_unpause_monotonic concurrently, nor is it
// safe to call this function concurrently with itself.
inline void timer_pause_monotonic() {
  // Read the current value of the modifier. We can do this without performing a sequential read
  // because this function is the only function that can modify the modifier after the timer has
  // been initialized.
  const zx_ticks_t modifier = internal::mono_ticks_modifier.load(ktl::memory_order_relaxed);
  DEBUG_ASSERT(modifier != ZX_TIME_INFINITE_PAST);
  DEBUG_ASSERT(modifier <= 0);

  // Set the modifier to ZX_TIME_INFINITE_PAST to signal to any concurrent calls to
  // timer_current_mono_ticks that the modifier is being updated and they should therefore retry
  // their read.
  internal::mono_ticks_modifier.store(ZX_TIME_INFINITE_PAST, ktl::memory_order_relaxed);

  // Compute the current monotonic time. To do so, we load the raw ticks with synchronization
  // barriers that will prevent it from being reordered above the previous load and store. This is
  // necessary to prevent this call from retrieving a raw ticks value from before we started the
  // pause operation. We don't need to specify `GetTicksSyncFlag::kBeforeSubsequentStores` because
  // the store of the mono_ticks_modifier below has a data dependency on this call.
  const zx_instant_mono_ticks_t raw_ticks =
      platform_current_raw_ticks_synchronized<GetTicksSyncFlag::kAfterPreviousLoads |
                                              GetTicksSyncFlag::kAfterPreviousStores>();

  // Finally, update the mono_ticks_modifier to the paused ticks value.
  const zx_instant_mono_ticks_t paused_ticks = raw_ticks + modifier;
  internal::mono_ticks_modifier.store(paused_ticks, ktl::memory_order_relaxed);
}
inline void timer_unpause_monotonic() {
  // First, load the existing modifier and assert that it is actually a paused value by checking
  // that it is positive.
  const zx_instant_mono_ticks_t paused_value =
      internal::mono_ticks_modifier.load(ktl::memory_order_relaxed);
  DEBUG_ASSERT(paused_value > 0);

  // Then, compute the new offset by subtracting the current raw ticks counter value from the
  // paused value. Assert the new offset is non-positive as expected.
  const zx_ticks_t raw_ticks = platform_current_raw_ticks();
  const zx_ticks_t new_offset = paused_value - raw_ticks;
  DEBUG_ASSERT(new_offset <= 0);

  // Finally, update the modifier value with the new offset.
  internal::mono_ticks_modifier.store(new_offset, ktl::memory_order_relaxed);
}

// Returns true iff the monotonic clock is paused.
//
// Intended to be used in asserts.  Does not synchronize-with anything.
inline bool timer_is_monotonic_paused() {
  return internal::mono_ticks_modifier.load(ktl::memory_order_relaxed) > 0;
}

#endif  // ZIRCON_KERNEL_INCLUDE_PLATFORM_TIMER_H_
