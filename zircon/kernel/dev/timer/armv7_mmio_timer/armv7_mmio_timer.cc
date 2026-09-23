// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/console.h>
#include <lib/root_resource_filter.h>
#include <lib/zbi-format/driver-config.h>

#include <arch/arm64/periphmap.h>
#include <dev/timer/armv7_mmio_timer.h>
#include <dev/timer/armv7_mmio_timer_registers.h>
#include <ktl/algorithm.h>
#include <ktl/array.h>
#include <ktl/unique_ptr.h>
#include <ktl/utility.h>

using namespace armv7_mmio_timer_registers;

void Armv7MmioTimer::Init(const zbi_dcfg_arm_generic_timer_mmio_driver_t& config) {
  // Add the top level control page and all of the EL1 timer register pages to
  // the root resource filter deny list.  We don't want user-mode to be able to
  // access these pages (although, it is OK if they can access the EL0 view).
  if (config.mmio_phys != 0) {
    root_resource_filter_add_deny_region(config.mmio_phys, 0x1000, ZX_RSRC_KIND_MMIO);
  }

  static_assert(Armv7MmioTimer::kMaxTimers ==
                ktl::size(zbi_dcfg_arm_generic_timer_mmio_driver_t{}.frames));

  for (uint32_t frame_number = 0; frame_number < Armv7MmioTimer::kMaxTimers; ++frame_number) {
    const zbi_dcfg_arm_generic_timer_mmio_frame_t& frame = config.frames[frame_number];

    if (frame.mmio_phys_el1 != 0) {
      root_resource_filter_add_deny_region(frame.mmio_phys_el1, 0x1000, ZX_RSRC_KIND_MMIO);
    }
    if (frame.mmio_phys_el0 != 0) {
      root_resource_filter_add_deny_region(frame.mmio_phys_el0, 0x1000, ZX_RSRC_KIND_MMIO);
    }
  }

  if (const vaddr_t virt_base = periph_paddr_to_vaddr(config.mmio_phys); virt_base != 0) {
    mmio_ctl_ = hwreg::RegisterMmio{reinterpret_cast<volatile void*>(virt_base)};
  } else {
    dprintf(CRITICAL, "Failed to translate ARMv7 Timer phys base @ 0x%08lx\n", config.mmio_phys);
    return;
  }

  const auto TIDR = CNTCTLBase::CNTTIDR::Get().ReadFrom(&mmio_ctl_);
  for (uint32_t frame_number = 0; frame_number < Armv7MmioTimer::kMaxTimers; ++frame_number) {
    // Don't attempt to initialize a timer if the ID register says it does not
    // exist.
    if (!TIDR.implemented(frame_number)) {
      continue;
    }

    if (timers_[frame_number] != nullptr) {
      dprintf(INFO, "ARMv7 Timer Frame %u has already been initialized\n", frame_number);
      continue;
    }

    const zbi_dcfg_arm_generic_timer_mmio_frame_t& frame = config.frames[frame_number];
    if (!frame.mmio_phys_el1) {
      continue;
    }

    const vaddr_t el1_mmio = periph_paddr_to_vaddr(frame.mmio_phys_el1);
    if (el1_mmio == 0) {
      dprintf(INFO, "Failed to translate EL1 timer base @ 0x%08lx for ARMv7 timer frame %u\n",
              frame.mmio_phys_el1, frame_number);
      continue;
    }

    vaddr_t el0_mmio{0};
    if (frame.mmio_phys_el0 != 0) {
      el0_mmio = periph_paddr_to_vaddr(frame.mmio_phys_el0);
      if (el0_mmio == 0) {
        dprintf(INFO, "Failed to translate EL0 timer base @ 0x%08lx for ARMv7 timer frame %u\n",
                frame.mmio_phys_el0, frame_number);
      } else {
        // Now that we have what appears to be a valid frame addresses for both
        // the EL1 and EL0 frames, unconditionally deny timer access to EL0.  We
        // don't want to accidentally give user mode access to a high resolution
        // timer if they are not suppose to have it, regardless of whether or
        // not we are going to skip the timer based on device tree
        // configuration, we still want to make sure that EL0 does not have
        // access.
        //
        // Note, it still should be pretty difficult for EL0 to access the timer
        // registers since we added them to the root resource filter deny list
        // at the start of Init, but it never hurts to add some extra
        // roadblocks.
        hwreg::RegisterMmio mmio{reinterpret_cast<volatile void*>(el1_mmio)};
        CNTBase::CNTEL0ACR::Get().FromValue(0).WriteTo(&mmio);
      }
    }

    // Finally, don't attempt to expose a timer which is not in the device-tree
    // "active frames" mask.
    //
    // Note: sometimes it is the case that there exist timers as reported by the
    // TIDR register which are not part of the active mask supplied by device
    // tree, even though the device tree _does_ provide both valid frame
    // addresses and interrupts.
    //
    // It is not immediately clear why device implementers might choose to do
    // this.  In theory, they might be trying to reserve these timers for use by
    // EL2/EL3, or perhaps for secure mode execution.  The main problem with
    // this theory is that those exception levels already have access to tools
    // that they can use to lock out EL0/EL1 access to the timers, in the form
    // of the CNTACR<n> and CNTNSAR registers.  If things like the secure
    // monitor wanted to reserve these timers for their own usage, why don't
    // they just deny access to EL0/EL1?
    //
    // Regardless, we skip them for now.
    //
    if (!(config.active_frames_mask & (1u << frame_number))) {
      dprintf(INFO,
              "Explicitly skipping ARMv7 Timer Frame %u.  DeviceTree declares it as disabled.\n",
              frame_number);
      continue;
    }

    fbl::AllocChecker ac;
    ktl::unique_ptr<Armv7MmioTimer> timer = ktl::make_unique<Armv7MmioTimer>(
        &ac, frame_number, el1_mmio, el0_mmio,
        Armv7MmioTimer::Irq{.num = frame.irq_phys, .zbi_flags = frame.irq_phys_flags},
        Armv7MmioTimer::Irq{.num = frame.irq_virt, .zbi_flags = frame.irq_virt_flags});
    if (!ac.check()) {
      dprintf(INFO, "Failed to allocated ARMv7 Timer for Frame %u\n", frame_number);
      continue;
    }

    timer->Setup();
    timers_[frame_number] = ktl::move(timer);
  }
}

void Armv7MmioTimer::Setup() {
  const auto TIDR = CNTCTLBase::CNTTIDR::Get().ReadFrom(&mmio_ctl_);
  const auto ACR = CNTCTLBase::CNTACR::Get(frame_ndx_).ReadFrom(&mmio_ctl_);

  uint32_t freq = ACR.RFRQ() ? CNTBase::CNTFRQ::Get().ReadFrom(&mmio_).reg_value() : 0;
  if (freq) {
    ticks_to_nsec_ = affine::Ratio{ZX_SEC(1), freq};
    ticks_to_nsec_.Reduce();
    pct_timer_.set_supported(ACR.RPCT() && ACR.RWPT() && (pct_timer_.irq() != 0));
    vct_timer_.set_supported(TIDR.virt_impl(frame_ndx_) && (vct_timer_.irq() != 0));
  }

  pct_timer_.Setup();
  vct_timer_.Setup();
}

void Armv7MmioTimer::Timer::Setup() {
  const uint32_t counter_offset =
      (type_ == Type::PCT) ? CNTBase::CNTPCT::kAddr : CNTBase::CNTVCT::kAddr;
  const uint32_t timer_offset =
      (type_ == Type::PCT) ? CNTBase::CNTP_CVAL::kAddr : CNTBase::CNTV_CVAL::kAddr;

  counter_mmio_ =
      hwreg::RegisterMmio{reinterpret_cast<volatile void*>(owner_.mmio_.base() + counter_offset)};
  timer_mmio_ =
      hwreg::RegisterMmio{reinterpret_cast<volatile void*>(owner_.mmio_.base() + timer_offset)};

  // unconditionally disable the timer and mask it at the timer register level.
  CTL::Get().FromValue(0).set_ENABLE(0).set_IMASK(1).WriteTo(&timer_mmio_);

  if (supported_) {
    DEBUG_ASSERT(irq() != 0);
    mask_interrupt(irq());

    const uint32_t mode_flags = irq_.zbi_flags & (ZBI_KERNEL_DRIVER_IRQ_FLAGS_EDGE_TRIGGERED |
                                                  ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED);
    const uint32_t polarity_flags = irq_.zbi_flags & (ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_LOW |
                                                      ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH);
    if ((ktl::popcount(mode_flags) == 1) || (ktl::popcount(polarity_flags) == 1)) {
      const interrupt_trigger_mode irq_mode =
          (mode_flags & ZBI_KERNEL_DRIVER_IRQ_FLAGS_EDGE_TRIGGERED) ? interrupt_trigger_mode::EDGE
                                                                    : interrupt_trigger_mode::LEVEL;
      const interrupt_polarity irq_polarity =
          (polarity_flags & ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_LOW) ? interrupt_polarity::LOW
                                                                      : interrupt_polarity::HIGH;

      configure_interrupt(irq(), irq_mode, irq_polarity);
      register_int_handler(irq(), interrupt_handler_t{this, &Timer::IrqHandlerThunkHelper});
      unmask_interrupt(irq());
    } else {
      supported_ = false;
      dprintf(INFO, "Bad IRQ flags for %s timer frame %u (flags 0x%08x)\n", type_name(),
              owner_.frame_ndx_, irq_.zbi_flags);
    }
  }
}

zx::result<zx_duration_t> Armv7MmioTimer::Timer::TimeUntilDeadline() const {
  if (!supported_) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  Guard<SpinLock, IrqSave> guard(&irq_lock_);
  if (!enabled_) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  const uint64_t deadline = CVAL::Get().ReadFrom(&timer_mmio_).reg_value();
  const uint64_t now = ticks();
  if (deadline <= now) {
    return zx::ok(zx_duration_t{0});
  }

  return zx::ok(static_cast<zx_duration_t>(owner_.ticks_to_nsec_.Scale(deadline - now)));
}

void Armv7MmioTimer::Timer::IrqHandlerThunk() {
  DEBUG_ASSERT(arch_ints_disabled());
  Guard<SpinLock, NoIrqSave> guard(&irq_lock_);

  // If someone canceled us after the handler fired, but before we made it to
  // this point, then the Cancel operation won the race.  Don't run the handler,
  // just get out.
  if (!enabled_) {
    return;
  }

  // Is our deadline in the future?  This is possible if someone reset the
  // deadline on another CPU after the interrupt fired, but before it made it
  // to here.  If this happens, the handler lost the race, and we should just
  // get out.
  const uint64_t deadline = CVAL::Get().ReadFrom(&timer_mmio_).reg_value();
  const uint64_t now = ticks();
  if (deadline >= now) {
    return;
  }

  // Looks like the timer fired while we were still enabled.  Disable the timer,
  // and dispatch the handler if we have one registered.
  DisableLocked();

  if (user_handler_ != nullptr) {
    // We have a handler.  Record that fact that we have an active IRQ handler
    // in flight by recording our CPU number in the `active_cpu_` member
    // variable, the run the handler.
    const cpu_num_t current_cpu = arch_curr_cpu_num();
    active_cpu_.store(current_cpu, ktl::memory_order_relaxed);

    // Note that we disable lock analysis here because we want to access the
    // `user_handler_`, which is typically protected by the `irq_lock_`.  It
    // is safe to access user_handler_ here _without_ the lock, because
    // `SetHandler` will not allow the user_handler_ to be changed  while
    // `active_cpu_` is something other than INVALID_CPU.
    IrqHandlerResult irq_result;
    guard.CallUnlocked([this, &irq_result]()
                           TA_NO_THREAD_SAFETY_ANALYSIS { irq_result = user_handler_(); });

    // Now that we are back inside of the lock, but before we mark ourselves as
    // no longer in flight, If the interrupt handler asked us to unregister, do
    // so now.
    if (irq_result == IrqHandlerResult::Unregister) {
      user_handler_ = IrqHandler{};
    }

    // We are back inside of the `irq_lock_` once again.  The IRQ handler is
    // finished, reset the `active_cpu_` back to INVALID_HANDLE.
    active_cpu_.store(INVALID_CPU, ktl::memory_order_release);
  }
}

zx_status_t Armv7MmioTimer::Timer::SetHandler(IrqHandler handler, cpu_mask_t mask) {
  InterruptDisableGuard irqd;

  // If we are resetting our handler, make sure to destroy the old handler
  // outside of the spinlock code by transferring it to old_handler first before
  // dropping the lock.
  IrqHandler old_handler{};
  const cpu_num_t current_cpu = arch_curr_cpu_num();
  while (true) {
    Guard<SpinLock, NoIrqSave> guard(&irq_lock_);

    // Users are not allowed to replace and existing handler with a new existing handler.  They must
    // explicitly reset the existing handler first.
    if ((user_handler_ != nullptr) && (handler != nullptr)) {
      return ZX_ERR_BAD_STATE;
    }

    // Is there an IRQ handler in flight?  If it is running on a different CPU,
    // drop our lock and wait for it to finish.  If it is running on the same
    // CPU as us, the it is in the process of calling is.  It is not legal to
    // reset the IRQ handler from within the IRQ handler itself.
    const cpu_num_t active_cpu = active_cpu_.load(ktl::memory_order_acquire);
    if (active_cpu != INVALID_CPU) {
      if (active_cpu != current_cpu) {
        guard.Release();
        arch::Yield();
        continue;
      }
      return ZX_ERR_BAD_STATE;
    }

    DisableLocked();

    if (const zx_status_t status = set_interrupt_affinity(irq(), mask); status != ZX_OK) {
      return status;
    }

    old_handler = ktl::move(user_handler_);
    user_handler_ = ktl::move(handler);

    return ZX_OK;
  }
}

zx_status_t Armv7MmioTimer::Timer::CancelTimer() {
  if (!supported_) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Make sure that interrupts remain disabled, even after we drop the lock to
  // sync with any in-flight IRQ handler.
  InterruptDisableGuard irqd;
  {
    // Lock and make sure the timer is disabled.
    Guard<SpinLock, IrqSave> guard(&irq_lock_);
    DisableLocked();
  }

  // Now that we have dropped the lock, sync with any IrqHandler which may
  // currently be in flight. There is a handler in flight if the `active_cpu_`
  // member is not INVALID_CPU.  Interrupt are currently disabled, so if the
  // `active_cpu_` member is valid, and equal to our current cpu, then this is a
  // call to Cancel being made from the IrqHandler itself, and we can
  // immediately return.  Otherwise we wait until we see the active CPU achieve
  // INVALID at least one.
  const cpu_num_t current_cpu = arch_curr_cpu_num();
  while (true) {
    const cpu_num_t active_cpu = active_cpu_.load(ktl::memory_order_acquire);
    if ((active_cpu == current_cpu) || (active_cpu == INVALID_CPU)) {
      break;
    }
    arch::Yield();
  }

  return ZX_OK;
}

zx_status_t Armv7MmioTimer::Timer::SetTimer(uint64_t ticks_deadline) {
  if (!supported_) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  InterruptDisableGuard irqd;
  const cpu_num_t current_cpu = arch_curr_cpu_num();

  while (true) {
    Guard<SpinLock, NoIrqSave> guard(&irq_lock_);
    // If there is an IRQ handler in flight, and it is not running on this CPU
    // (meaning it is calling us), wait until it completes before setting the
    // timer.
    const cpu_num_t active_cpu = active_cpu_.load(ktl::memory_order_acquire);
    if ((active_cpu != current_cpu) && (active_cpu != INVALID_CPU)) {
      guard.Release();
      arch::Yield();
      continue;
    }

    DisableLocked();
    CVAL::Get().FromValue(ticks_deadline).WriteTo(&timer_mmio_);
    EnableLocked();
    break;
  }

  return ZX_OK;
}

zx_status_t Armv7MmioTimer::Timer::SetRelativeTimer(zx_duration_t relative_timeout) {
  return SetTimer(ticks() + owner_.ticks_to_nsec_.Inverse().Scale(relative_timeout));
}
