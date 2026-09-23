// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_TIMER_ARMV7_MMIO_TIMER_INCLUDE_DEV_TIMER_ARMV7_MMIO_TIMER_H_
#define ZIRCON_KERNEL_DEV_TIMER_ARMV7_MMIO_TIMER_INCLUDE_DEV_TIMER_ARMV7_MMIO_TIMER_H_

#include <lib/affine/ratio.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zx/result.h>
#include <stdint.h>

#include <dev/interrupt.h>
#include <dev/timer/armv7_mmio_timer_registers.h>
#include <hwreg/mmio.h>
#include <ktl/array.h>
#include <ktl/optional.h>
#include <ktl/unique_ptr.h>

// # Armv7MmioTimer
//
// Armv7MmioTimer and its associated inner class, Armv7MmioTimer::Timer provide
// an interface to the memory mapped timer used described in the v7 ARM ARM.
//
// Armv7MmioTimer represents a block of timer registers, while
// Armv7MmioTimer::Timer represents the specific PCT and VCT based timers within
// that block, when available.
//
// ## Usage.
//
// ### Finding a timer.
//
// After initialization, which takes place just before LK_INIT_LEVEL_PLATFORM,
// users may find a timer to use with the Armv7MmioTimer::Get method.  There are
// a maximum of Armv7MmioTimer::kMaxTimers timer blocks in the system.  When a
// given block index is not detected or is otherwise unusable, the
// Armv7MmioTimer::Get method will return nullptr.
//
// Each block may contain a PCT based timer, a VCT based timer, or both.  They
// may be accessed via the pct_timer() and vct_timer() methods of a timer block
// instance, and are usable if their `supported()` members return true.
//
// ### Configuring a handler.
//
// In order to receive notification that a timer has fired, users must register
// an Armv7MmioTimer::IrqHandler with the timer via the Timer::SetHandler
// method.  This is an instance of a fit::inline_function which takes no
// arguments and returns a Armv7MmioTimer::IrqHandlerResult.
//
// Only one handler can be registered at once, and attempts to register a second
// handler will return an error.  In order to change an existing handler for a
// Timer instance, users must first call ResetHandler.
//
// ### Using the timer.
//
// #### Programming.
//
// After configuring a handler, users may read the timer's current clock value
// via the `ticks()` method.  Additionally, the Timer's tick rate may be read
// from its parent timer block's `ticks_to_nsec()` method.  An absolute deadline
// may be programmed via the SetTimer method, which takes a deadline expressed
// in the timer's specific ticks timeline.  Alternatively, a relative timer may
// be set using the SetRelativeTimer helper method, which takes a deadline
// expressed in nanosecond units.
//
// #### Canceling.
//
// A programmed timer may be canceled at any time by calling the CancelTimer
// method.  While canceling a running timer is a race, the Timer instance
// guarantees that after CancelTimer has been called, any handlers will have
// been successfully canceled and will not run, or they will have already run to
// completion.  Calls to CancelTimer are idempotent.
//
// #### Handling a timer event.
//
// When a timer finally fires, a user's registered handler will be called.  This
// is done at hard IRQ time and with interrupts disabled.  Blocking is not
// allowed.  The timer always acts as a one-shot.  When the timer handler has
// been called, the timer is effectively canceled.  Users may safely reset their
// timer during their timer handler by calling either SetTimer or
// SetRelativeTimer. They are also free to cancel their timer using CancelTimer,
// however since timers are effectively already canceled at the start of the
// handler, this would only matter if they had already set the timer during the
// handler and suddenly wanted to cancel it.
//
// Users *may not* change their registered handler during their handler
// callback.  Any attempt to do so with produce an error.  That said, timer
// handlers may choose to stay registered after the handler has run by retuning
// IrqHandlerResult::RemainRegistered, or they have have their handler
// unregistered after completing by returning IrqHandlerResult::Unregister.  Care
// must be taken, however, as the destruction of the inline function will take
// place at IRQ time.  In order to safely unregister the handler at this time,
// the inline function must not hold and references to anything which would
// require blocking to destroy (such as a heap allocation).
//
class Armv7MmioTimer {
 public:
  class Timer;  // fwd decl
  enum class Type { PCT, VCT };
  enum class IrqHandlerResult { RemainRegistered, Unregister };
  using IrqHandler = fit::inline_function<IrqHandlerResult(), sizeof(void*)>;

  struct Irq {
    uint32_t num;
    uint32_t zbi_flags;
  };

  static inline constexpr size_t kMaxTimers = 8;

  static void Init(const zbi_dcfg_arm_generic_timer_mmio_driver_t& config);

  static Armv7MmioTimer* Get(size_t ndx) {
    return (ndx < timers_.size()) ? timers_[ndx].get() : nullptr;
  }

  // Console diags
  static int Dump(uint8_t timer_mask);
  static int ShowStatus(uint8_t timer_mask);

  Armv7MmioTimer(uint32_t frame_ndx, vaddr_t mmio, vaddr_t el0_mmio, Irq phys_irq, Irq virt_irq)
      : frame_ndx_(frame_ndx),
        mmio_{reinterpret_cast<volatile void*>(mmio)},
        el0_mmio_{reinterpret_cast<volatile void*>(el0_mmio)},
        pct_timer_{*this, Type::PCT, phys_irq},
        vct_timer_{*this, Type::VCT, virt_irq} {}

  ~Armv7MmioTimer() = default;

  affine::Ratio ticks_to_nsec() const { return ticks_to_nsec_; }

  Timer& pct_timer() { return pct_timer_; }
  Timer& vct_timer() { return vct_timer_; }

  const Timer& pct_timer() const { return pct_timer_; }
  const Timer& vct_timer() const { return vct_timer_; }

  class Timer {
   private:
    using CT = armv7_mmio_timer_registers::CNTPVRegs::CT;
    using CVAL = armv7_mmio_timer_registers::CNTPVRegs::CVAL;
    using TVAL = armv7_mmio_timer_registers::CNTPVRegs::TVAL;
    using CTL = armv7_mmio_timer_registers::CNTPVRegs::CTL;

   public:
    Timer(const Armv7MmioTimer& owner, Type type, Irq irq)
        : owner_(owner), type_(type), irq_(irq) {}

    void set_supported(bool value) { supported_ = value; }
    bool supported() const { return supported_; }

    bool enabled() const TA_EXCL(irq_lock_) {
      Guard<SpinLock, IrqSave> guard(&irq_lock_);
      return enabled_;
    }

    bool has_handler() const TA_EXCL(irq_lock_) {
      Guard<SpinLock, IrqSave> guard(&irq_lock_);
      return user_handler_ != nullptr;
    }

    uint32_t irq() const { return irq_.num; }
    Type type() const { return type_; }
    const char* type_name() const { return type_ == Type::PCT ? "PCT" : "VCT"; }

    uint64_t ticks() const { return CT::Get().ReadFrom(&counter_mmio_).reg_value(); }

    void Setup();

    zx_status_t SetHandler(IrqHandler handler, cpu_mask_t mask = cpu_num_to_mask(BOOT_CPU_ID))
        TA_EXCL(irq_lock_);
    zx_status_t ResetHandler() TA_EXCL(irq_lock_) { return SetHandler(IrqHandler{}); }
    zx_status_t CancelTimer() TA_EXCL(irq_lock_);
    zx_status_t SetTimer(uint64_t ticks_deadline) TA_EXCL(irq_lock_);
    zx_status_t SetRelativeTimer(zx_duration_t relative_timout) TA_EXCL(irq_lock_);

    zx::result<zx_duration_t> TimeUntilDeadline() const;

   private:
    void IrqHandlerThunk();
    static void IrqHandlerThunkHelper(void* cookie) {
      static_cast<Timer*>(cookie)->IrqHandlerThunk();
    }

    zx_status_t CancelTimerLocked() TA_REQ(irq_lock_);

    void DisableLocked() TA_REQ(irq_lock_) {
      CTL::Get().FromValue(0).set_ENABLE(0).set_IMASK(1).WriteTo(&timer_mmio_);
      enabled_ = false;
    }

    void EnableLocked() TA_REQ(irq_lock_) {
      CTL::Get().FromValue(0).set_ENABLE(1).set_IMASK(0).WriteTo(&timer_mmio_);
      enabled_ = true;
    }

    const Armv7MmioTimer& owner_;
    mutable hwreg::RegisterMmio counter_mmio_{nullptr};
    mutable hwreg::RegisterMmio timer_mmio_{nullptr};

    const Type type_;
    const Irq irq_;
    bool supported_{false};

    mutable DECLARE_SPINLOCK(Timer) irq_lock_;
    TA_GUARDED(irq_lock_)::Armv7MmioTimer::IrqHandler user_handler_ { nullptr };
    TA_GUARDED(irq_lock_) bool enabled_ { false };
    ktl::atomic<cpu_num_t> active_cpu_{INVALID_CPU};
  };

 private:
  void Setup();

  static inline hwreg::RegisterMmio mmio_ctl_{nullptr};
  static ktl::array<ktl::unique_ptr<Armv7MmioTimer>, kMaxTimers> timers_;

  const uint32_t frame_ndx_;
  mutable hwreg::RegisterMmio mmio_{nullptr};
  mutable hwreg::RegisterMmio el0_mmio_{nullptr};

  affine::Ratio ticks_to_nsec_;
  Timer pct_timer_;
  Timer vct_timer_;
};

// Can't be _defined_ / constructed until the type is complete, so `inline`
// can't just be in the declaration inside the class (still incomplete there).
inline ktl::array<ktl::unique_ptr<Armv7MmioTimer>, Armv7MmioTimer::kMaxTimers>
    Armv7MmioTimer::timers_;

#endif  // ZIRCON_KERNEL_DEV_TIMER_ARMV7_MMIO_TIMER_INCLUDE_DEV_TIMER_ARMV7_MMIO_TIMER_H_
