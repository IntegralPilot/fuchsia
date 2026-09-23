// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <lib/arch/arm64/apple-aic.h>
#include <lib/arch/arm64/system.h>
#include <lib/mmio-ptr/mmio-ptr.h>
#include <lib/root_resource_filter.h>
#include <stdio.h>
#include <zircon/errors.h>

#include <initializer_list>

#include <arch/arm64.h>
#include <arch/arm64/interrupt.h>
#include <dev/interrupt/apple_aic3.h>
#include <kernel/lockdep.h>
#include <kernel/mp.h>
#include <pdev/interrupt.h>
#include <vm/vm_aspace.h>

namespace {
using arch::apple::Aic3Layout;
using arch::apple::Aic3State;
constexpr uint32_t kPhysTimer = arch::apple::kAicPhysTimer;
constexpr uint32_t kVirtTimer = arch::apple::kAicVirtTimer;
constexpr uint32_t kExternalBase = arch::apple::kAicExternalBase;
constexpr uint32_t kMaxExternalIrqs = arch::apple::kAicMaxExternalIrqs;
constexpr uint32_t kTimerEnable = 1u << 0;
constexpr uint32_t kTimerMask = 1u << 1;
constexpr uint32_t kTimerPending = 1u << 2;
static_assert(kMaxExternalIrqs + kExternalBase <= MAX_INTERRUPTS);
static_assert(SMP_MAX_CPUS == 1);
DECLARE_SINGLETON_SPINLOCK(aic_lock);
zbi_dcfg_apple_aic3_t config;
vaddr_t base;
Aic3Layout layout{};
Aic3State state{layout};
uint32_t pending_ipis TA_GUARDED(aic_lock::Get()) = 0;

uint32_t Read(uint32_t offset) {
  return MmioRead32(reinterpret_cast<MMIO_PTR volatile uint32_t*>(base + offset));
}
void Write(uint32_t offset, uint32_t value) {
  MmioWrite32(value, reinterpret_cast<MMIO_PTR volatile uint32_t*>(base + offset));
  // Complete masking before restoring DAIF or returning from the exception.
  __dsb(ARM_MB_SY);
}
struct AicIo {
  void Write(uint32_t offset, uint32_t value) { ::Write(offset, value); }
} io;

bool IsTimer(uint32_t vector) { return vector == kPhysTimer || vector == kVirtTimer; }
bool Valid(uint32_t vector, uint32_t) {
  return IsTimer(vector) || (vector >= kExternalBase && vector - kExternalBase < layout.num_irqs);
}
uint64_t TimerControl(uint32_t vector) {
  uint64_t value;
  if (vector == kPhysTimer) {
    __asm__ volatile("mrs %0, cntp_ctl_el0" : "=r"(value));
  } else {
    __asm__ volatile("mrs %0, cntv_ctl_el0" : "=r"(value));
  }
  return value;
}
void SetTimerControl(uint32_t vector, uint64_t value) {
  if (vector == kPhysTimer) {
    __asm__ volatile("msr cntp_ctl_el0, %0; isb" ::"r"(value) : "memory");
  } else {
    __asm__ volatile("msr cntv_ctl_el0, %0; isb" ::"r"(value) : "memory");
  }
}
zx_status_t SetMask(uint32_t vector, bool mask) {
  if (!Valid(vector, 0)) {
    return ZX_ERR_INVALID_ARGS;
  }
  Guard<SpinLock, IrqSave> guard{aic_lock::Get()};
  if (IsTimer(vector)) {
    auto ctl = TimerControl(vector);
    SetTimerControl(vector, mask ? ctl | kTimerMask : ctl & ~uint64_t{kTimerMask});
  } else {
    const uint32_t irq = vector - kExternalBase;
    state.SetMask(io, irq, mask);
  }
  return ZX_OK;
}
zx_status_t Mask(uint32_t vector) { return SetMask(vector, true); }
zx_status_t Unmask(uint32_t vector) { return SetMask(vector, false); }
zx_status_t Deactivate(uint32_t vector) { return Valid(vector, 0) ? ZX_OK : ZX_ERR_INVALID_ARGS; }
zx_status_t Configure(uint32_t vector, interrupt_trigger_mode mode, interrupt_polarity polarity) {
  if (!Valid(vector, 0)) {
    return ZX_ERR_INVALID_ARGS;
  }
  return mode == interrupt_trigger_mode::LEVEL && polarity == interrupt_polarity::HIGH
             ? ZX_OK
             : ZX_ERR_NOT_SUPPORTED;
}
zx_status_t GetConfig(uint32_t vector, interrupt_trigger_mode* mode, interrupt_polarity* polarity) {
  if (!Valid(vector, 0)) {
    return ZX_ERR_INVALID_ARGS;
  }
  *mode = interrupt_trigger_mode::LEVEL;
  *polarity = interrupt_polarity::HIGH;
  return ZX_OK;
}
zx_status_t SetAffinity(uint32_t vector, cpu_mask_t mask) {
  if (!Valid(vector, 0)) {
    return ZX_ERR_INVALID_ARGS;
  }
  return mask == 1 ? ZX_OK : ZX_ERR_NOT_SUPPORTED;
}
void Handle(iframe_t*) {
  const uint32_t event = Read(config.event_offset);
  if (!event) {
    return;
  }
  const auto irq = layout.DecodeEvent(event);
  ASSERT_MSG(irq, "AIC3 unexpected event %#x", event);
  const bool handled = pdev_invoke_int_if_present(*irq + kExternalBase);
  Guard<SpinLock, IrqSave> guard{aic_lock::Get()};
  state.Complete(io, *irq, handled);
}
void Shutdown() {
  for (uint32_t vector = 0; vector < layout.num_irqs; ++vector) {
    Mask(vector + kExternalBase);
  }
  Mask(kPhysTimer);
  Mask(kVirtTimer);
}
const pdev_interrupt_ops ops = {
    .mask = Mask,
    .unmask = Unmask,
    .deactivate = Deactivate,
    .configure = Configure,
    .get_config = GetConfig,
    .set_affinity = SetAffinity,
    .is_valid = Valid,
    .get_base_vector = []() -> uint32_t { return 0; },
    .get_max_vector = []() -> uint32_t { return layout.num_irqs + kExternalBase; },
    .remap = [](uint32_t vector) { return vector; },
    .send_ipi = [](cpu_mask_t target, mp_ipi ipi) -> zx_status_t {
      if (target & cpu_num_to_mask(BOOT_CPU_ID)) {
        Guard<SpinLock, IrqSave> guard{aic_lock::Get()};
        pending_ipis |= uint32_t{1} << static_cast<uint32_t>(ipi);
        // Even a single-CPU kernel needs asynchronous reschedule IPIs.
        __dsb(ARM_MB_SY);
        arch::AppleIpiLocalRequest::Write(arch::AppleIpiLocalRequest::Get().FromValue(0).set_target(
            arch::ArmMpidrEl1::Read().aff0()));
        __isb(ARM_MB_SY);
      }
      return ZX_OK;
    },
    .init_percpu_early = []() {},
    .init_percpu =
        []() {
          ASSERT(arch_curr_cpu_num() == 0);
          mp_set_curr_cpu_online(true);
        },
    .handle_irq = Handle,
    .shutdown = Shutdown,
    .shutdown_cpu = Shutdown,
    .suspend_cpu = []() -> zx_status_t { return ZX_ERR_NOT_SUPPORTED; },
    .resume_cpu = []() -> zx_status_t { return ZX_ERR_NOT_SUPPORTED; },
    .msi_is_supported = []() { return false; },
    .msi_supports_masking = []() { return false; },
    .msi_mask_unmask = [](const msi_block_t*, uint, bool) { PANIC("AIC3 MSI not supported"); },
    .msi_alloc_block = [](uint, bool, bool, msi_block_t*) -> zx_status_t {
      return ZX_ERR_NOT_SUPPORTED;
    },
    .msi_free_block = [](msi_block_t*) { PANIC("AIC3 MSI not supported"); },
    .msi_register_handler = [](const msi_block_t*, uint,
                               interrupt_handler_t) { PANIC("AIC3 MSI not supported"); },
};
}  // namespace

void AppleAic3HandleFiq() {
  bool pending = false;
  if (arch::AppleIpiStatus::Read().pending()) {
    pending = true;
    uint32_t ipis;
    {
      Guard<SpinLock, IrqSave> guard{aic_lock::Get()};
      arch::AppleIpiStatus::Write(arch::AppleIpiStatus::Get().FromValue(0).set_pending(true));
      __isb(ARM_MB_SY);
      ipis = pending_ipis;
      pending_ipis = 0;
    }
    if (ipis & (uint32_t{1} << static_cast<uint32_t>(mp_ipi::HALT))) {
      while (true) {
        __wfi();
      }
    }
    if (ipis & (uint32_t{1} << static_cast<uint32_t>(mp_ipi::GENERIC))) {
      mp_mbx_generic_irq();
    }
    if (ipis & (uint32_t{1} << static_cast<uint32_t>(mp_ipi::RESCHEDULE))) {
      mp_mbx_reschedule_irq();
    }
    if (ipis & (uint32_t{1} << static_cast<uint32_t>(mp_ipi::INTERRUPT))) {
      mp_mbx_interrupt_irq();
    }
  }
  for (uint32_t vector : {kPhysTimer, kVirtTimer}) {
    if ((TimerControl(vector) & (kTimerEnable | kTimerMask | kTimerPending)) ==
        (kTimerEnable | kTimerPending)) {
      pending = true;
      if (!pdev_invoke_int_if_present(vector)) {
        Mask(vector);
      }
    }
  }
  ASSERT_MSG(pending, "Unexpected FIQ: physical CTL=%#lx virtual CTL=%#lx",
             TimerControl(kPhysTimer), TimerControl(kVirtTimer));
}

void AppleAic3InitEarly() {
  ASSERT_MSG(arch_curr_cpu_num() == 0, "AIC3 requires a single boot CPU");
}

void AppleAic3InitPostVm(const zbi_dcfg_apple_aic3_t& payload) {
  ASSERT_MSG(Aic3Layout::ValidMmio(payload.mmio_phys, payload.mmio_size, payload.event_offset),
             "AIC3 invalid MMIO configuration");
  config = payload;
  void* ptr;
  zx_status_t status = VmAspace::kernel_aspace()->AllocPhysical(
      "apple-aic3", config.mmio_size, &ptr, PAGE_SIZE_SHIFT, config.mmio_phys, 0,
      ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_UNCACHED_DEVICE);
  ASSERT(status == ZX_OK);
  base = reinterpret_cast<vaddr_t>(ptr);
  root_resource_filter_add_deny_region(config.mmio_phys, config.mmio_size, ZX_RSRC_KIND_MMIO);
  const auto discovered =
      Aic3Layout::Create(Read(Aic3Layout::kInfo), Read(Aic3Layout::kCapacity), config.mmio_size);
  ASSERT_MSG(discovered, "AIC3 unsupported register layout");
  layout = *discovered;
  state = Aic3State(layout);
  Shutdown();
  Write(Aic3Layout::kConfig, Read(Aic3Layout::kConfig) | Aic3Layout::kEnable);
  pdev_register_interrupts(&ops);
  printf("AIC3: %u external IRQs, timer FIQ vectors %u/%u\n", layout.num_irqs, kPhysTimer,
         kVirtTimer);
}
