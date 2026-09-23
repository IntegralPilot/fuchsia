// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <debug.h>
#include <lib/suspend_wakeup_timer.h>

#include <fbl/alloc_checker.h>
#include <kernel/idle_power_thread.h>
#include <kernel/timer.h>

#ifdef __aarch64__
#include <dev/timer/armv7_mmio_timer.h>
#endif

class GenericSuspendWakeupTimer final : public SuspendWakeupTimer {
 public:
  GenericSuspendWakeupTimer(Callback callback) : SuspendWakeupTimer(ktl::move(callback)) {}
  ~GenericSuspendWakeupTimer() override = default;

  void EnsureStarted() override {
    Guard<SpinLock, IrqSave> timer_guard{&lock_};

    if (!started_ && resume_at_ != ZX_TIME_INFINITE) {
      auto handler = +[](Timer*, zx_instant_boot_t now, void* thiz) -> void {
        reinterpret_cast<GenericSuspendWakeupTimer*>(thiz)->DoCallback(now);
      };

      DEBUG_ASSERT(arch_curr_cpu_num() == BOOT_CPU_ID);
      generic_resume_timer_.SetOneshot(resume_at_, handler, this);
      started_ = true;
    }
  }

  void CancelTimer() override {
    Guard<SpinLock, IrqSave> timer_guard{&lock_};
    generic_resume_timer_.Cancel();
    ResetLocked();
  }

 protected:
  Timer generic_resume_timer_{ZX_CLOCK_BOOT};
};

#if __aarch64__
class Armv7MmioSuspendWakeupTimer : public SuspendWakeupTimer {
 public:
  Armv7MmioSuspendWakeupTimer(Armv7MmioTimer::Timer& timer, Callback callback)
      : SuspendWakeupTimer(ktl::move(callback)), timer_(timer) {
    // Make certain that our timer is canceled, then register our handler.
    ASSERT(timer_.CancelTimer() == ZX_OK);

    const zx_status_t status = timer_.SetHandler(
        [this]() {
          DoCallback(current_boot_time());
          return Armv7MmioTimer::IrqHandlerResult::RemainRegistered;
        },
        cpu_num_to_mask(BOOT_CPU_ID));

    ASSERT(status == ZX_OK);
  }

  ~Armv7MmioSuspendWakeupTimer() override = default;

  void EnsureStarted() override {
    Guard<SpinLock, IrqSave> timer_guard{&lock_};

    if (!started_ && resume_at_ != ZX_TIME_INFINITE) {
      DEBUG_ASSERT(!timer_.enabled());
      const zx_duration_boot_t resume_delta =
          ktl::max<zx_duration_t>(0, zx_time_sub_time(resume_at_, current_boot_time()));
      started_ = true;
      timer_.SetRelativeTimer(resume_delta);
    }
  }

  void CancelTimer() override {
    Guard<SpinLock, IrqSave> timer_guard{&lock_};
    timer_.CancelTimer();
    ResetLocked();
  }

 private:
  Armv7MmioTimer::Timer& timer_;
};
#endif

ktl::unique_ptr<SuspendWakeupTimer> SuspendWakeupTimer::Create(Callback callback) {
#if __aarch64__
  // Search through the driver for a v7 MMIO which can use the VCT reference.
  // Failing that, try to fall back on which can use PCT.
  constexpr ktl::array kTimerTypes = {
      Armv7MmioTimer::Type::VCT,
      Armv7MmioTimer::Type::PCT,
  };

  for (Armv7MmioTimer::Type type : kTimerTypes) {
    for (size_t i = 0; i < Armv7MmioTimer::kMaxTimers; ++i) {
      if (Armv7MmioTimer* timer_block = Armv7MmioTimer::Get(i); timer_block != nullptr) {
        Armv7MmioTimer::Timer& timer = (type == Armv7MmioTimer::Type::VCT)
                                           ? timer_block->vct_timer()
                                           : timer_block->pct_timer();

        if (timer.supported()) {
          dprintf(INFO, "Using ARMv7 %s timer #%zu for suspend-wakeup.\n",
                  (type == Armv7MmioTimer::Type::VCT) ? "VCT" : "PCT", i);

          fbl::AllocChecker ac;
          ktl::unique_ptr<SuspendWakeupTimer> ret =
              fbl::make_unique_checked<Armv7MmioSuspendWakeupTimer>(ac, timer, ktl::move(callback));
          ASSERT(ac.check());
          return ret;
        }
      }
    }
  }
#endif

  dprintf(INFO, "Using Generic timer for suspend-wakeup.\n");
  fbl::AllocChecker ac;
  ktl::unique_ptr<SuspendWakeupTimer> ret =
      fbl::make_unique_checked<GenericSuspendWakeupTimer>(ac, ktl::move(callback));
  ASSERT(ac.check());
  return ret;
}
