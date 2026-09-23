// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/interrupts.h"

#include <lib/async/cpp/task.h>
#include <lib/device-protocol/pci.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/result.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>

#include <bitset>

#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>

#include "src/graphics/display/drivers/intel-display/ddi.h"
#include "src/graphics/display/drivers/intel-display/pci-ids.h"
#include "src/graphics/display/drivers/intel-display/registers-ddi.h"
#include "src/graphics/display/drivers/intel-display/registers-pipe.h"
#include "src/graphics/display/drivers/intel-display/registers.h"

namespace intel_display {

namespace {

struct HotplugDetectionResult {
  static constexpr size_t kMaxAllowedDdis = 32;
  std::bitset<kMaxAllowedDdis> detected;
  std::bitset<kMaxAllowedDdis> long_pulse;
};

HotplugDetectionResult DetectHotplugSkylake(fdf::MmioBuffer* mmio_space) {
  HotplugDetectionResult result;

  auto sde_int_identity =
      registers::SdeInterruptBase::Get(registers ::SdeInterruptBase::kSdeIntIdentity)
          .ReadFrom(mmio_space);
  auto hp_ctrl1 = registers::SouthHotplugCtrl ::Get(DdiId::DDI_A).ReadFrom(mmio_space);
  auto hp_ctrl2 = registers::SouthHotplugCtrl ::Get(DdiId::DDI_E).ReadFrom(mmio_space);
  for (auto ddi : DdiIds<registers::Platform::kKabyLake>()) {
    auto hp_ctrl = ddi < DdiId::DDI_E ? hp_ctrl1 : hp_ctrl2;
    result.detected[ddi] =
        sde_int_identity.skl_ddi_bit(ddi).get() &
        (hp_ctrl.hpd_long_pulse(ddi).get() || hp_ctrl.hpd_short_pulse(ddi).get());
    result.long_pulse[ddi] = hp_ctrl.hpd_long_pulse(ddi).get();
  }
  // Write back the register to clear the bits
  hp_ctrl1.WriteTo(mmio_space);
  hp_ctrl2.WriteTo(mmio_space);
  sde_int_identity.WriteTo(mmio_space);

  return result;
}

HotplugDetectionResult DetectHotplugTigerLake(fdf::MmioBuffer* mmio_space) {
  HotplugDetectionResult result;

  auto sde_int_identity =
      registers::SdeInterruptBase::Get(registers ::SdeInterruptBase::kSdeIntIdentity)
          .ReadFrom(mmio_space);
  auto hpd_int_identity =
      registers::HpdInterruptBase::Get(registers::HpdInterruptBase::kHpdIntIdentity)
          .ReadFrom(mmio_space);

  auto pch_ddi_ctrl = registers::IclSouthHotplugCtrl::Get(DdiId::DDI_A).ReadFrom(mmio_space);
  auto pch_tc_ctrl = registers::IclSouthHotplugCtrl::Get(DdiId::DDI_TC_1).ReadFrom(mmio_space);

  auto tbt_ctrl = registers::TbtHotplugCtrl::Get().ReadFrom(mmio_space);
  auto tc_ctrl = registers::TcHotplugCtrl::Get().ReadFrom(mmio_space);

  for (auto ddi : DdiIds<registers::Platform::kTigerLake>()) {
    switch (ddi) {
      case DdiId::DDI_A:
      case DdiId::DDI_B:
      case DdiId::DDI_C: {
        result.detected[ddi] =
            sde_int_identity.icl_ddi_bit(ddi).get() &
            (pch_ddi_ctrl.hpd_long_pulse(ddi).get() || pch_ddi_ctrl.hpd_short_pulse(ddi).get());
        result.long_pulse[ddi] = pch_ddi_ctrl.hpd_long_pulse(ddi).get();
      } break;
      case DdiId::DDI_TC_1:
      case DdiId::DDI_TC_2:
      case DdiId::DDI_TC_3:
      case DdiId::DDI_TC_4:
      case DdiId::DDI_TC_5:
      case DdiId::DDI_TC_6: {
        bool sde_detected = sde_int_identity.icl_ddi_bit(ddi).get();
        bool tbt_detected = hpd_int_identity.tbt_hotplug(ddi).get();
        bool tc_detected = hpd_int_identity.tc_hotplug(ddi).get();
        result.detected[ddi] = tbt_detected || tc_detected || sde_detected;
        result.long_pulse[ddi] = (tbt_detected && tbt_ctrl.hpd_long_pulse(ddi).get()) ||
                                 (tc_detected && tc_ctrl.hpd_long_pulse(ddi).get()) ||
                                 (sde_detected && pch_tc_ctrl.hpd_long_pulse(ddi).get());
      } break;
    }
  }

  // Write back the register to clear the bits
  pch_ddi_ctrl.WriteTo(mmio_space);
  pch_tc_ctrl.WriteTo(mmio_space);
  tbt_ctrl.WriteTo(mmio_space);
  tc_ctrl.WriteTo(mmio_space);
  sde_int_identity.WriteTo(mmio_space);
  hpd_int_identity.WriteTo(mmio_space);

  return result;
}

void EnableHotplugInterruptsSkylake(fdf::MmioBuffer* mmio_space) {
  auto pch_fuses = registers::PchDisplayFuses::Get().ReadFrom(mmio_space);

  for (const auto ddi : DdiIds<registers::Platform::kKabyLake>()) {
    bool enabled = false;
    switch (ddi) {
      case DdiId::DDI_A:
      case DdiId::DDI_E:
        enabled = true;
        break;
      case DdiId::DDI_B:
        enabled = pch_fuses.port_b_present();
        break;
      case DdiId::DDI_C:
        enabled = pch_fuses.port_c_present();
        break;
      case DdiId::DDI_D:
        enabled = pch_fuses.port_d_present();
        break;
      case DdiId::DDI_TC_3:
      case DdiId::DDI_TC_4:
      case DdiId::DDI_TC_5:
      case DdiId::DDI_TC_6:
        ZX_DEBUG_ASSERT_MSG(false, "Unsupported DDI (%d)", ddi);
        break;
    }

    auto hp_ctrl = registers::SouthHotplugCtrl::Get(ddi).ReadFrom(mmio_space);
    hp_ctrl.hpd_enable(ddi).set(enabled);
    hp_ctrl.WriteTo(mmio_space);

    auto mask = registers::SdeInterruptBase::Get(registers::SdeInterruptBase::kSdeIntMask)
                    .ReadFrom(mmio_space);
    mask.skl_ddi_bit(ddi).set(!enabled);
    mask.WriteTo(mmio_space);

    auto enable = registers::SdeInterruptBase::Get(registers::SdeInterruptBase::kSdeIntEnable)
                      .ReadFrom(mmio_space);
    enable.skl_ddi_bit(ddi).set(enabled);
    enable.WriteTo(mmio_space);
  }
}

void EnableHotplugInterruptsTigerLake(fdf::MmioBuffer* mmio_space) {
  constexpr zx_off_t kSHPD_FILTER_CNT = 0xc4038;
  constexpr uint32_t kSHPD_FILTER_CNT_500_ADJ = 0x001d9;
  mmio_space->Write32(kSHPD_FILTER_CNT_500_ADJ, kSHPD_FILTER_CNT);

  for (const auto ddi : DdiIds<registers::Platform::kTigerLake>()) {
    switch (ddi) {
      case DdiId::DDI_TC_1:
      case DdiId::DDI_TC_2:
      case DdiId::DDI_TC_3:
      case DdiId::DDI_TC_4:
      case DdiId::DDI_TC_5:
      case DdiId::DDI_TC_6: {
        auto hp_ctrl = registers::TcHotplugCtrl::Get().ReadFrom(mmio_space);
        hp_ctrl.hpd_enable(ddi).set(1);
        hp_ctrl.WriteTo(mmio_space);

        auto mask = registers::HpdInterruptBase::Get(registers::HpdInterruptBase::kHpdIntMask)
                        .ReadFrom(mmio_space);
        mask.set_reg_value(0);
        mask.WriteTo(mmio_space);

        auto enable = registers::HpdInterruptBase::Get(registers::HpdInterruptBase::kHpdIntEnable)
                          .ReadFrom(mmio_space);
        enable.tc_hotplug(ddi).set(1);
        enable.tbt_hotplug(ddi).set(1);
        enable.WriteTo(mmio_space);
      }
        __FALLTHROUGH;
      case DdiId::DDI_A:
      case DdiId::DDI_B:
      case DdiId::DDI_C: {
        auto hp_ctrl = registers::IclSouthHotplugCtrl::Get(ddi).ReadFrom(mmio_space);
        hp_ctrl.hpd_enable(ddi).set(1);
        hp_ctrl.WriteTo(mmio_space);

        auto mask = registers::SdeInterruptBase::Get(registers::SdeInterruptBase::kSdeIntMask)
                        .ReadFrom(mmio_space);
        mask.set_reg_value(0);
        mask.WriteTo(mmio_space);
        mask.ReadFrom(mmio_space);

        auto enable = registers::SdeInterruptBase::Get(registers::SdeInterruptBase::kSdeIntEnable)
                          .ReadFrom(mmio_space);
        enable.icl_ddi_bit(ddi).set(1);
        enable.WriteTo(mmio_space);
      } break;
    }
  }
}

}  // namespace

Interrupts::Interrupts() { mtx_init(&lock_, mtx_plain); }

Interrupts::~Interrupts() { Destroy(); }

void Interrupts::Destroy() {
  if (irq_handler_dispatcher_.get() != nullptr) {
    irq_handler_dispatcher_.ShutdownAsync();
    irq_handler_dispatcher_shutdown_completed_.Wait();
  }

  if (irq_.is_valid()) {
    irq_.destroy();
    irq_.reset();
  }
}

void Interrupts::InterruptHandler(async_dispatcher_t* dispatcher, async::IrqBase* irq,
                                  zx_status_t status, const zx_packet_interrupt_t* interrupt) {
  if (status == ZX_ERR_CANCELED) {
    fdf::info("Vsync interrupt wait is cancelled.");
    return;
  }
  if (status != ZX_OK) {
    fdf::error("Vsync interrupt wait failed: {}", zx::make_result(status));
    // A failed async interrupt wait doesn't remove the interrupt from the
    // async loop, so we have to manually cancel it.
    irq->Cancel();
    return;
  }

  // We implement the steps in the section "Shared Functions" > "Interrupts" >
  // "Interrupt Service Routine" section of Intel's display engine docs.
  //
  // Tiger Lake: IHD-OS-TGL-Vol 12-1.22-Rev2.0 pages 199-200
  // Kaby Lake: IHD-OS-KBL-Vol 12-1.17 pages 142-143
  // Skylake: IHD-OS-SKL-Vol 12-05.16 pages 139-140

  auto graphics_primary_interrupts = registers::GraphicsPrimaryInterrupt::Get().FromValue(0);
  if (is_tgl(device_id_)) {
    graphics_primary_interrupts.ReadFrom(mmio_space_)
        .set_interrupts_enabled(false)
        .WriteTo(mmio_space_);
  }

  auto display_interrupts = registers::DisplayInterruptControl::Get().ReadFrom(mmio_space_);
  display_interrupts.set_interrupts_enabled(false);
  display_interrupts.WriteTo(mmio_space_);

  const bool pch_display_hotplug_pending = display_interrupts.pch_engine_pending();
  const bool display_hotplug_pending =
      is_tgl(device_id_) && display_interrupts.display_hot_plug_pending_tiger_lake();

  if (pch_display_hotplug_pending || display_hotplug_pending) {
    auto detect_result = is_tgl(device_id_) ? DetectHotplugTigerLake(mmio_space_)
                                            : DetectHotplugSkylake(mmio_space_);
    for (auto ddi : GetDdiIds(device_id_)) {
      if (detect_result.detected[ddi]) {
        fdf::trace("Detected hot plug interrupt on ddi {}", ddi);
        hotplug_callback_(ddi, detect_result.long_pulse[ddi]);
      }
    }
  }

  // TODO(https://fxbug.dev/42060657): Check for Pipe D interrupts here when we support
  //                         pipe and transcoder D.
  zx::time_monotonic timestamp(interrupt->timestamp);
  if (display_interrupts.pipe_c_pending()) {
    HandlePipeInterrupt(PipeId::PIPE_C, timestamp);
  }
  if (display_interrupts.pipe_b_pending()) {
    HandlePipeInterrupt(PipeId::PIPE_B, timestamp);
  }
  if (display_interrupts.pipe_a_pending()) {
    HandlePipeInterrupt(PipeId::PIPE_A, timestamp);
  }

  {
    // Dispatch GT interrupts to the GPU driver.
    fbl::AutoLock lock(&lock_);
    if (gpu_interrupt_callback_.callback) {
      if (is_tgl(device_id_)) {
        if (graphics_primary_interrupts.gt1_interrupt_pending() ||
            graphics_primary_interrupts.gt0_interrupt_pending()) {
          // Mask isn't used
          gpu_interrupt_callback_.callback(gpu_interrupt_callback_.ctx, 0, timestamp.get());
        }
      } else {
        if (display_interrupts.reg_value() & gpu_interrupt_mask_) {
          gpu_interrupt_callback_.callback(gpu_interrupt_callback_.ctx,
                                           display_interrupts.reg_value(), timestamp.get());
        }
      }
    }
  }

  display_interrupts.set_interrupts_enabled(true).WriteTo(mmio_space_);

  if (is_tgl(device_id_)) {
    graphics_primary_interrupts.set_interrupts_enabled(true).WriteTo(mmio_space_);
  }

  // For interrupts bound to ports (including those bound to async loops), the
  // interrupt must be re-armed using zx_interrupt_ack() for each incoming
  // interrupt request. This is best done after the interrupt has been fully
  // processed.
  zx::unowned_interrupt(irq->object())->ack();
}

void Interrupts::HandlePipeInterrupt(PipeId pipe_id, zx::time_monotonic timestamp) {
  registers::PipeRegs regs(pipe_id);
  auto interrupt_identity =
      regs.PipeInterrupt(registers::PipeRegs::InterruptRegister::kIdentity).ReadFrom(mmio_space_);

  // Interrupt Identity Registers (IIR) are R/WC (Read/Write Clear), meaning
  // that indicator bits are cleared by writing 1s to them. Writing the value we
  // just read declares that we've handled all the interrupts reported there.
  interrupt_identity.WriteTo(mmio_space_);

  if (interrupt_identity.underrun()) {
    fdf::warn("Transcoder underrun on pipe {}", pipe_id);
  }
  if (interrupt_identity.vsync()) {
    pipe_vsync_callback_(pipe_id, timestamp);
  }
}

void Interrupts::EnablePipeInterrupts(PipeId pipe_id, bool enable) {
  registers::PipeRegs regs(pipe_id);
  auto interrupt_mask =
      regs.PipeInterrupt(registers::PipeRegs::InterruptRegister::kMask).FromValue(0);
  interrupt_mask.set_underrun(!enable).set_vsync(!enable).WriteTo(mmio_space_);

  auto interrupt_enable =
      regs.PipeInterrupt(registers::PipeRegs::InterruptRegister::kEnable).FromValue(0);
  interrupt_enable.set_underrun(enable).set_vsync(enable).WriteTo(mmio_space_);
}

zx_status_t Interrupts::SetGpuInterruptCallback(
    const intel_gpu_core_interrupt_t& gpu_interrupt_callback, uint32_t gpu_interrupt_mask) {
  fbl::AutoLock lock(&lock_);
  if (gpu_interrupt_callback.callback != nullptr && gpu_interrupt_callback_.callback != nullptr) {
    return ZX_ERR_ALREADY_BOUND;
  }
  gpu_interrupt_callback_ = gpu_interrupt_callback;
  gpu_interrupt_mask_ = gpu_interrupt_mask;
  return ZX_OK;
}

zx_status_t Interrupts::Init(PipeVsyncCallback pipe_vsync_callback,
                             HotplugCallback hotplug_callback, const ddk::Pci& pci,
                             fdf::MmioBuffer* mmio_space, uint16_t device_id) {
  ZX_DEBUG_ASSERT(pipe_vsync_callback);
  ZX_DEBUG_ASSERT(hotplug_callback);
  ZX_DEBUG_ASSERT(mmio_space);
  ZX_DEBUG_ASSERT(!irq_.is_valid());

  pipe_vsync_callback_ = std::move(pipe_vsync_callback);
  hotplug_callback_ = std::move(hotplug_callback);
  mmio_space_ = mmio_space;
  device_id_ = device_id;

  // Interrupt propagation will be re-enabled in ::FinishInit()
  fdf::trace("Disabling graphics and display interrupt propagation");

  if (is_tgl(device_id_)) {
    auto graphics_primary_interrupts =
        registers::GraphicsPrimaryInterrupt::Get().ReadFrom(mmio_space);
    graphics_primary_interrupts.set_interrupts_enabled(false).WriteTo(mmio_space_);
  }

  auto interrupt_ctrl = registers::DisplayInterruptControl::Get().ReadFrom(mmio_space);
  interrupt_ctrl.set_interrupts_enabled(false).WriteTo(mmio_space);

  // Assume that PCI will enable bus mastering as required for MSI interrupts.
  zx_status_t status = pci.ConfigureInterruptMode(1, &irq_mode_);
  if (status != ZX_OK) {
    fdf::error("Failed to configure irq mode ({})", status);
    return ZX_ERR_INTERNAL;
  }

  status = pci.MapInterrupt(0, &irq_);
  if (status != ZX_OK) {
    fdf::error("Failed to map interrupt ({})", status);
    return status;
  }
  irq_handler_.set_object(irq_.get());

  const char* kRoleName = "fuchsia.graphics.display.drivers.intel-display.interrupt";
  zx::result<fdf::SynchronizedDispatcher> create_dispatcher_result =
      fdf::SynchronizedDispatcher::Create(
          fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "intel-display-irq-thread",
          /*shutdown_handler=*/
          [this](fdf_dispatcher_t*) { irq_handler_dispatcher_shutdown_completed_.Signal(); },
          kRoleName);
  if (create_dispatcher_result.is_error()) {
    fdf::error("Failed to create vsync Dispatcher: {}", create_dispatcher_result);
    return create_dispatcher_result.status_value();
  }
  irq_handler_dispatcher_ = std::move(create_dispatcher_result).value();

  status = irq_handler_.Begin(irq_handler_dispatcher_.async_dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to begin IRQ handler wait: {}", zx::make_result(status));
    return status;
  }

  Resume();
  return ZX_OK;
}

void Interrupts::FinishInit() {
  fdf::trace("Interrupts re-enabled");

  auto display_interrupts = registers::DisplayInterruptControl::Get().ReadFrom(mmio_space_);
  display_interrupts.set_interrupts_enabled(true).WriteTo(mmio_space_);

  if (is_tgl(device_id_)) {
    auto graphics_primary_interrupts =
        registers::GraphicsPrimaryInterrupt::Get().ReadFrom(mmio_space_);
    graphics_primary_interrupts.set_interrupts_enabled(true).WriteTo(mmio_space_);

    graphics_primary_interrupts.ReadFrom(mmio_space_);  // posting read
  }
}

void Interrupts::Resume() {
  if (is_tgl(device_id_)) {
    EnableHotplugInterruptsTigerLake(mmio_space_);
  } else {
    EnableHotplugInterruptsSkylake(mmio_space_);
  }
}

}  // namespace intel_display
